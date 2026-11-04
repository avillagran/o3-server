//! Calling-thread worktree handoff, adapted from nightly4151's MIT WorktreeMcpService.
use crate::{
    events,
    mcp_access::Authority,
    proc::{self, Opts},
    store::{iso, now_ms, random_id, State},
};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
type Reply = Result<Value, Value>;
fn failure(code: &str, message: impl AsRef<str>) -> Value {
    json!({"_tag":"WorktreeMcpFailure","code":code,"message":message.as_ref()})
}
fn operation(message: impl std::fmt::Display) -> Value {
    failure("operation_failed", message.to_string())
}
struct Guard {
    state: Arc<State>,
    thread: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.state
            .worktree_handoffs
            .lock()
            .unwrap()
            .remove(&self.thread);
    }
}

pub(crate) async fn call(s: Arc<State>, a: Authority, p: Value) -> Reply {
    let tid = a
        .native_thread()
        .ok_or_else(|| {
            failure(
                "thread_credential_required",
                "Worktree handoff needs an agent running inside a T3 thread.",
            )
        })?
        .to_owned();
    {
        let db = s.reader.lock().unwrap();
        a.check(
            &db,
            &json!({"type":"thread.metadata.update","threadId":tid}),
        )
        .map_err(crate::mcp_access::command_error)?;
    }
    if !s.worktree_handoffs.lock().unwrap().insert(tid.clone()) {
        return Err(failure(
            "handoff_in_progress",
            "A worktree handoff is already in progress for this thread.",
        ));
    }
    let guard = Guard {
        state: s.clone(),
        thread: tid.clone(),
    };
    // A workspace detach can end the calling process or HTTP request. Keep creation,
    // binding, continuation and setup together until the operation has settled.
    tokio::spawn(async move {
        let _guard = guard;
        perform(s, a, &tid, p).await
    })
    .await
    .map_err(|_| operation("Worktree handoff did not finish."))?
}
fn load(s: &State, tid: &str) -> Result<(Value, Value), Value> {
    let db = s.reader.lock().unwrap();
    let t = events::get(&db, "thread", tid)
        .ok()
        .filter(|t| t["deletedAt"].is_null())
        .ok_or_else(|| failure("thread_not_found", "The calling thread was not found."))?;
    if !t["worktreePath"].is_null() {
        return Err(failure(
            "already_in_worktree",
            format!(
                "This thread is already attached to worktree '{}'.",
                t["worktreePath"].as_str().unwrap_or("")
            ),
        ));
    }
    if !t["archivedAt"].is_null() {
        return Err(failure(
            "invalid_request",
            "An archived thread cannot be handed off.",
        ));
    }
    let project = events::get(&db, "project", t["projectId"].as_str().unwrap_or(""))
        .ok()
        .filter(|p| p["deletedAt"].is_null())
        .ok_or_else(|| {
            failure(
                "project_not_found",
                "The calling thread's project was not found.",
            )
        })?;
    Ok((t, project))
}
async fn output(root: &Path, args: &[&str], opts: Opts) -> Result<proc::Out, Value> {
    proc::git(root, args, opts)
        .await
        .map_err(|_| operation("The git operation could not finish."))
}
async fn git(root: &Path, args: &[&str], opts: Opts) -> Result<String, Value> {
    let out = output(root, args, opts).await?;
    if out.ok() {
        Ok(out.text().trim().into())
    } else {
        Err(operation("The git operation failed."))
    }
}
async fn remote_base(root: &Path, base: &str) -> Result<Option<(String, String)>, Value> {
    let remotes = git(root, &["remote"], Opts::READ).await?;
    let mut remotes: Vec<_> = remotes.lines().collect();
    remotes.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let local = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(upstream:remotename)\t%(upstream:remoteref)",
            &format!("refs/heads/{base}"),
        ],
        Opts::READ,
    )
    .await?;
    let tracked = local
        .lines()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .find(|p| p.first().copied() == Some(format!("refs/heads/{base}").as_str()));
    let target = if let Some(p) = tracked {
        p.get(1)
            .filter(|r| remotes.contains(r))
            .zip(p.get(2).and_then(|b| b.strip_prefix("refs/heads/")))
            .map(|(r, b)| (*r, b))
    } else {
        remotes
            .iter()
            .find_map(|r| base.strip_prefix(&format!("{r}/")).map(|b| (*r, b)))
    }
    .or_else(|| remotes.contains(&"origin").then_some(("origin", base)));
    let Some((remote, branch)) = target else {
        return Ok(None);
    };
    if !output(
        root,
        &["check-ref-format", &format!("refs/heads/{branch}")],
        Opts::READ,
    )
    .await?
    .ok()
    {
        return Ok(None);
    }
    let refname = format!("refs/remotes/{remote}/{branch}");
    let out = output(
        root,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--",
            remote,
            &format!("+refs/heads/{branch}:{refname}"),
        ],
        Opts::NETWORK,
    )
    .await?;
    if !out.ok() {
        if out.stderr.contains("couldn't find remote ref") || out.stderr.contains("no such ref") {
            return Ok(None);
        }
        return Err(operation("Unable to fetch the remote worktree base."));
    }
    let commit = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ],
        Opts::READ,
    )
    .await?;
    Ok(Some((commit, refname)))
}
async fn managed_path(
    s: &State,
    root: &Path,
    branch: &str,
    settings: &Value,
) -> Result<PathBuf, Value> {
    let setting = settings["worktreesDirectory"].as_str().unwrap_or("");
    let directory = if setting.is_empty() {
        s.dir.join("worktrees")
    } else if setting == "~" || setting.starts_with("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(setting.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(setting)
    };
    if !directory.is_absolute() || directory.parent().is_none() {
        return Err(failure(
            "invalid_request",
            "The worktree directory must be absolute and cannot be a filesystem root.",
        ));
    }
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|_| operation("Unable to create the managed worktree directory."))?;
    let directory = tokio::fs::canonicalize(&directory)
        .await
        .map_err(|_| operation("Unable to resolve the managed worktree directory."))?;
    if directory.parent().is_none() {
        return Err(failure(
            "invalid_request",
            "The worktree directory resolves to a filesystem root.",
        ));
    }
    let name = branch.replace('/', "-");
    let name = if name.encode_utf16().count() > 40 {
        let mut units = 0;
        let prefix: String = name
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 23
            })
            .collect();
        format!(
            "{prefix}-{}",
            &format!("{:x}", Sha256::digest(branch.as_bytes()))[..16]
        )
    } else {
        name
    };
    let path =
        directory
            .join(root.file_name().ok_or_else(|| {
                failure("invalid_request", "The project folder must have a name.")
            })?)
            .join(name);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .map_err(|_| operation("Unable to create the managed project folder."))?;
    if tokio::fs::symlink_metadata(&path).await.is_ok() {
        return Err(failure(
            "invalid_request",
            "The managed worktree destination already exists.",
        ));
    }
    Ok(path)
}
async fn rollback(s: &State, root: &Path, path: &Path, branch: &str) -> Result<(), Value> {
    let used:bool=s.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='thread' AND json_extract(body,'$.deletedAt') IS NULL AND json_extract(body,'$.worktreePath')=?)",[path.to_string_lossy().as_ref()],|r|r.get(0)).map_err(|_|operation("Unable to verify rollback ownership."))?;
    if used {
        return Err(operation(
            "The created worktree was retained because another thread now uses it.",
        ));
    }
    let listed = git(root, &["worktree", "list", "--porcelain"], Opts::READ).await?;
    let owned = listed.split("\n\n").any(|entry| {
        entry
            .lines()
            .any(|l| l == format!("worktree {}", path.display()))
            && entry
                .lines()
                .any(|l| l == format!("branch refs/heads/{branch}"))
    });
    if owned {
        git(
            root,
            &[
                "worktree",
                "remove",
                "--force",
                "--",
                path.to_str().unwrap(),
            ],
            Opts::WRITE,
        )
        .await?;
    }
    git(root, &["branch", "-D", "--", branch], Opts::WRITE).await?;
    Ok(())
}

fn bind(
    s: &Arc<State>,
    a: &Authority,
    project: &Value,
    path: &Path,
    branch: &str,
    id: &str,
    p: &Value,
) -> Reply {
    let tid = a.native_thread().ok_or_else(|| {
        failure(
            "thread_credential_required",
            "Worktree handoff needs a calling thread.",
        )
    })?;
    let _gate = s
        .worktree_gate
        .try_read()
        .map_err(|_| operation("Worktree cleanup is in progress; retry handoff."))?;
    let mut db = s.db.lock().unwrap();
    if s.rollback_pending
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(operation(
            "Rollback recovery is required before worktree handoff.",
        ));
    }
    let tx = db
        .transaction()
        .map_err(|_| operation("Unable to start the binding transaction."))?;
    let command = json!({"type":"thread.metadata.update","commandId":id,"threadId":tid,"worktreePath":path,"branch":branch,"expectedWorktreePath":null});
    let mut t = events::get(&tx, "thread", tid).map_err(operation)?;
    if !t["deletedAt"].is_null() {
        return Err(failure(
            "thread_not_found",
            "The thread was deleted while the worktree was being created.",
        ));
    }
    if !t["worktreePath"].is_null() {
        return Err(failure(
            "already_in_worktree",
            "The thread was attached to another worktree while git was preparing this one.",
        ));
    }
    if !t["archivedAt"].is_null() || t["projectId"] != project["id"] {
        return Err(failure(
            "invalid_request",
            "The thread was archived or moved while the worktree was being created.",
        ));
    }
    let current =
        events::get(&tx, "project", project["id"].as_str().unwrap_or("")).map_err(operation)?;
    if !current["deletedAt"].is_null() || current["workspaceRoot"] != project["workspaceRoot"] {
        return Err(failure(
            "project_not_found",
            "The project changed while the worktree was being created.",
        ));
    }
    a.check(&tx, &command)
        .map_err(crate::mcp_access::command_error)?;
    let at = iso(now_ms());
    crate::workspace_transition::plan(&tx, &t, &command, &at).map_err(operation)?;
    t["worktreePath"] = json!(path);
    t["branch"] = json!(branch);
    t["ownedWorktree"] = json!(true);
    t["updatedAt"] = json!(at);
    events::put(&tx, "thread", tid, tid, &t).map_err(operation)?;
    let seq =
        events::append(&tx, tid, "thread.metadata-updated", t.clone(), &t).map_err(operation)?;
    let continuation = if let Some(prompt) = p["continuationPrompt"].as_str() {
        // Source queues immediately after binding. A savepoint preserves its typed
        // partial-failure result while making successful admission receipt-atomic.
        tx.execute_batch("SAVEPOINT handoff_continuation")
            .map_err(operation)?;
        let message = json!({"type":"message.dispatch","commandId":format!("{id}:continue"),"threadId":tid,"messageId":format!("{id}:message"),"text":prompt,"attachments":[],"dispatchMode":{"type":"queue_after_active"}});
        match events::stage_message(s, &tx, &message, &mut t, &at, true) {
            Ok(_) => {
                tx.execute_batch("RELEASE handoff_continuation")
                    .map_err(operation)?;
                json!({"status":"scheduled","delivery":"queued"})
            }
            Err(error) => {
                tx.execute_batch("ROLLBACK TO handoff_continuation; RELEASE handoff_continuation")
                    .map_err(operation)?;
                json!({"status":"failed","detail":error})
            }
        }
    } else {
        json!({"status":"skipped"})
    };
    tx.execute(
        "INSERT INTO receipts VALUES(?,?)",
        params![id, json!({"sequence":seq}).to_string()],
    )
    .map_err(|_| operation("Unable to commit the worktree binding receipt."))?;
    tx.commit()
        .map_err(|_| operation("Unable to commit the worktree binding."))?;
    drop(db);
    s.executions.cancel(tid);
    s.notify();
    Ok(continuation)
}
fn setup_script(settings: &Value, project: &Value) -> Option<Value> {
    let id = project["id"].as_str().unwrap_or("");
    let scripts =
        if let Some(v) = settings["projectSettingsOverrides"][id].get("defaultProjectScripts") {
            v
        } else if settings["projectSettingsFolded"] == true {
            &settings["defaultProjectScripts"]
        } else {
            match settings["projectScriptOverrides"].get(id) {
                Some(Value::Null) => &settings["defaultProjectScripts"],
                Some(v) => v,
                None if project["scripts"].as_array().is_some_and(|s| !s.is_empty()) => {
                    &project["scripts"]
                }
                None => &settings["defaultProjectScripts"],
            }
        };
    scripts
        .as_array()?
        .iter()
        .find(|s| s["runOnWorktreeCreate"] == true)
        .cloned()
}
async fn perform(s: Arc<State>, a: Authority, tid: &str, p: Value) -> Reply {
    let (_, project) = load(&s, tid)?;
    let root = PathBuf::from(
        project["workspaceRoot"]
            .as_str()
            .ok_or_else(|| failure("invalid_request", "The project has no workspace root."))?,
    );
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| failure("invalid_request", "The project workspace is missing."))?;
    let _repo = proc::lock_repo(&root).await;
    if !output(&root, &["rev-parse", "--git-dir"], Opts::READ)
        .await?
        .ok()
    {
        return Err(failure(
            "invalid_request",
            "The project workspace is not a git repository.",
        ));
    }
    let branch = p["branch"].as_str().unwrap();
    if branch.starts_with('-')
        || !output(&root, &["check-ref-format", "--branch", branch], Opts::READ)
            .await?
            .ok()
    {
        return Err(failure(
            "invalid_request",
            "Choose a valid new branch name.",
        ));
    }
    if output(
        &root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
        Opts::READ,
    )
    .await?
    .ok()
    {
        let listed = git(&root, &["worktree", "list", "--porcelain"], Opts::READ).await?;
        let checkout = listed
            .split("\n\n")
            .find(|e| {
                e.lines()
                    .any(|l| l == format!("branch refs/heads/{branch}"))
            })
            .and_then(|e| e.lines().find_map(|l| l.strip_prefix("worktree ")));
        return Err(failure("invalid_request",format!("Branch '{branch}' already exists{}. Choose another branch, or delete the existing branch and its worktree first.",checkout.map(|p|format!(" and is checked out at '{p}'")).unwrap_or_default())));
    }
    let base = match p["baseRef"].as_str() {
        Some(base) => base.to_owned(),
        None => {
            let out = output(
                &root,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                Opts::READ,
            )
            .await?;
            if !out.ok() {
                return Err(failure("invalid_request","Cannot determine the current branch (detached HEAD?). Pass baseRef explicitly."));
            }
            out.text().trim().into()
        }
    };
    if base.starts_with('-') || base.contains('\0') {
        return Err(failure("invalid_request", "Invalid baseRef."));
    }
    let settings = crate::settings::read(&s).map_err(operation)?;
    let default = settings["projectSettingsOverrides"][project["id"].as_str().unwrap_or("")]
        .get("newWorktreesStartFromOrigin")
        .unwrap_or(&settings["newWorktreesStartFromOrigin"])
        == true;
    let remote = if p["startFromOrigin"].as_bool().unwrap_or(default) {
        remote_base(&root, &base).await?
    } else {
        None
    };
    let reference = remote.as_ref().map(|r| r.0.as_str()).unwrap_or(&base);
    let commit = git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
        Opts::READ,
    )
    .await?;
    let path = managed_path(&s, &root, branch, &settings).await?;
    let checkout = path
        .to_str()
        .ok_or_else(|| operation("Invalid worktree path."))?;
    let configured_workers =
        output(&root, &["config", "--get", "checkout.workers"], Opts::READ).await?;
    let workers = if configured_workers.ok() {
        configured_workers.text().trim().to_owned()
    } else {
        "0".into()
    };
    git(
        &root,
        &["branch", "--no-track", "--", branch, &commit],
        Opts::WRITE,
    )
    .await?;
    let created = git(
        &root,
        &[
            "-c",
            &format!("checkout.workers={workers}"),
            "worktree",
            "add",
            "--",
            checkout,
            branch,
        ],
        Opts {
            timeout: Duration::ZERO,
            ..Opts::WRITE
        },
    )
    .await;
    let result = async {
        created?;
        let base_ref = remote.as_ref().map(|r| r.1.as_str()).unwrap_or(&base);
        if let Some((name, full)) = comparison_base(&root, base_ref).await? {
            let _ = git(
                &root,
                &[
                    "config",
                    "--local",
                    &format!("branch.{branch}.t3-merge-base"),
                    &name,
                ],
                Opts::WRITE,
            )
            .await;
            if full.starts_with("refs/remotes/") {
                let _ = git(
                    &root,
                    &[
                        "config",
                        "--local",
                        &format!("branch.{branch}.t3-merge-base-ref"),
                        &full,
                    ],
                    Opts::WRITE,
                )
                .await;
            }
        }
        if path.join(".gitmodules").is_file() {
            let effective = crate::source_control::effective(&s, &root);
            let file = tokio::fs::read(path.join("t3.json"))
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let mode = effective["worktreeSubmodules"]
                .as_str()
                .or_else(|| file.as_ref().and_then(|f| f["worktreeSubmodules"].as_str()))
                .unwrap_or("recursive");
            if mode != "none" {
                let args = if mode == "top-level" {
                    vec!["submodule", "update", "--init"]
                } else {
                    vec!["submodule", "update", "--init", "--recursive"]
                };
                if git(
                    &path,
                    &args,
                    Opts {
                        timeout: Duration::ZERO,
                        ..Opts::WRITE
                    },
                )
                .await
                .is_err()
                {
                    eprintln!("Worktree submodule checkout was incomplete.");
                }
            }
        }
        let id = format!("mcp:worktree-handoff:{}", random_id().map_err(operation)?);
        let (state, auth, proj, work, branch, p) = (
            s.clone(),
            a.clone(),
            project.clone(),
            path.clone(),
            branch.to_owned(),
            p.clone(),
        );
        tokio::task::spawn_blocking(move || bind(&state, &auth, &proj, &work, &branch, &id, &p))
            .await
            .map_err(|_| operation("The binding transaction did not finish."))?
    }
    .await;
    let continuation = match result {
        Ok(v) => v,
        Err(error) => {
            if let Err(cleanup) = rollback(&s, &root, &path, branch).await {
                return Err(operation(format!(
                    "{} {}",
                    error["message"].as_str().unwrap_or("Handoff failed."),
                    cleanup["message"].as_str().unwrap_or("Rollback failed.")
                )));
            }
            return Err(error);
        }
    };
    drop(_repo);
    let setup = run_setup(&s, tid, &p, &project, &root, &path).await;
    Ok(
        json!({"worktreePath":path,"branch":branch,"baseRef":base,"startedFromOrigin":remote.is_some(),"setupScript":setup,"continuation":continuation,"note":if continuation["status"]=="scheduled"{"Handoff recorded. Changing the workspace ends this live session; the queued prompt resumes the conversation in the worktree next turn. The worktree remains after thread deletion."}else{"Handoff recorded. Changing the workspace ends this live session; the next message resumes the conversation in the worktree. Pass continuationPrompt to resume automatically. The worktree remains after thread deletion."}}),
    )
}

async fn run_setup(
    s: &Arc<State>,
    tid: &str,
    p: &Value,
    project: &Value,
    root: &Path,
    path: &Path,
) -> Value {
    if p["runSetupScript"] == false {
        return json!({"status":"skipped"});
    }
    let thread = match events::get(&s.reader.lock().unwrap(), "thread", tid) {
        Ok(t) => t,
        Err(error) => return json!({"status":"failed","detail":error}),
    };
    let fenced = thread
        .get("linkOrigin")
        .is_some_and(|v| !v.is_null() && v["access"] != "full-access");
    if fenced {
        return json!({"status":"skipped"});
    }
    let settings = match crate::settings::read(s) {
        Ok(v) => v,
        Err(error) => return json!({"status":"failed","detail":error}),
    };
    if let Some(script) = setup_script(&settings, project) {
        let terminal = format!("setup-{}", script["id"].as_str().unwrap_or("project"));
        let input = json!({"threadId":tid,"terminalId":terminal,"cwd":path,"worktreePath":path,"env":{"T3CODE_PROJECT_ROOT":root,"T3CODE_WORKTREE_PATH":path,"COLORTERM":"","NO_COLOR":"1","FORCE_COLOR":"0"}});
        match crate::terminal::project_script(
            s,
            &input,
            script["command"].as_str().unwrap_or("").into(),
        )
        .await
        {
            Ok(()) => json!({"status":"started","scriptName":script["name"],"terminalId":terminal}),
            Err(error) => json!({"status":"failed","detail":error.to_string()}),
        }
    } else {
        json!({"status":"no-script"})
    }
}

async fn comparison_base(root: &Path, reference: &str) -> Result<Option<(String, String)>, Value> {
    let out = output(
        root,
        &[
            "rev-parse",
            "--symbolic-full-name",
            "--verify",
            "--end-of-options",
            reference,
        ],
        Opts::READ,
    )
    .await?;
    if !out.ok() {
        return Ok(None);
    }
    let full = out.text().trim().to_owned();
    if let Some(name) = full.strip_prefix("refs/heads/") {
        return Ok(Some((name.into(), full)));
    }
    if let Some(remote_ref) = full.strip_prefix("refs/remotes/") {
        let remotes = git(root, &["remote"], Opts::READ).await?;
        let mut remotes: Vec<_> = remotes.lines().collect();
        remotes.sort_by_key(|r| std::cmp::Reverse(r.len()));
        if let Some(name) = remotes
            .iter()
            .find_map(|r| remote_ref.strip_prefix(&format!("{r}/")))
        {
            return Ok(Some((name.into(), full)));
        }
    }
    Ok(None)
}
