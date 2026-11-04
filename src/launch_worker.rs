//! Durable preparation effects. Native provider admission happens only after release.
use crate::{
    events::{append, get, put},
    proc::{self, Opts},
    store::{iso, now_ms, State},
    worktree_setup as tracker,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
type Result<T> = std::result::Result<T, String>;
struct Job {
    tid: String,
    rid: Option<String>,
    plan: Value,
}
fn load(db: &Connection, id: &str) -> Result<Job> {
    let (tid, rid, body): (String, Option<String>, String) = db
        .query_row(
            "SELECT thread_id,run_id,request FROM thread_launches WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    Ok(Job {
        tid,
        rid,
        plan: serde_json::from_str(&body).unwrap_or(Value::Null),
    })
}
fn cancelled(s: &State, id: &str) -> bool {
    let db = s.db.lock().unwrap();
    db.query_row("SELECT cancelled!=0 OR (uncancellable=0 AND (NOT EXISTS(SELECT 1 FROM records t WHERE t.kind='thread' AND t.id=l.thread_id AND json_extract(t.body,'$.deletedAt') IS NULL AND json_extract(t.body,'$.archivedAt') IS NULL) OR (run_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM records r WHERE r.kind='runs' AND r.id=l.run_id AND json_extract(r.body,'$.status')='preparing')))) FROM thread_launches l WHERE id=?",[id],|r|r.get::<_,bool>(0)).unwrap_or(true)
}
fn check(cancel: &tokio::sync::watch::Receiver<bool>) -> Result<()> {
    if *cancel.borrow() {
        Err("Worktree setup cancelled.".into())
    } else {
        Ok(())
    }
}
async fn g(
    root: &Path,
    args: &[&str],
    opts: Opts,
    cancel: &tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
    check(cancel)?;
    let out = proc::git_controlled(root, args, opts, cancel.clone())
        .await
        .map_err(|e| format!("Git preparation failed: {e:?}"))?;
    if out.ok() {
        Ok(out.text().trim().into())
    } else {
        Err(format!(
            "Git preparation failed: {}",
            tracker::clamp(&out.stderr, 1000)
        ))
    }
}
pub(crate) async fn run(s: &Arc<State>, id: &str) -> Result<()> {
    let deferred_cleanup =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT completed!=0 AND cleanup_pending!=0 FROM thread_launches WHERE id=?",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?;
    if deferred_cleanup {
        cleanup(s, id, true).await?;
        s.db.lock()
            .unwrap()
            .execute(
                "UPDATE thread_launches SET cleanup_pending=0 WHERE id=?",
                [id],
            )
            .map_err(|e| e.to_string())?;
        s.notify();
        return Ok(());
    }
    let cleanup: Option<String> =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT error FROM thread_launches WHERE id=? AND phase='cleanup'",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
    if let Some(error) = cleanup {
        return Err(error);
    }
    let phase: String =
        s.db.lock()
            .unwrap()
            .query_row("SELECT phase FROM thread_launches WHERE id=?", [id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
    if phase != "admitted" {
        return Err("Interrupted workspace preparation; retry explicitly after cleanup.".into());
    }
    let mut changes = s.changed.subscribe();
    let (send, cancel) = tokio::sync::watch::channel(false);
    let mut naming = None;
    let mut work = Box::pin(prepare(s, id, cancel, &mut naming));
    let result = loop {
        tokio::select! {result=&mut work=>break result,r=changes.changed()=>{
            if r.is_err() || cancelled(s,id){send.send_replace(true);}
        }}
    };
    drop(work);
    if cancelled(s, id) {
        if let Some(task) = naming {
            let _ = task.await;
        }
    }
    result
}

async fn prepare(
    s: &Arc<State>,
    id: &str,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    naming: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    let workspace = s.worktree_gate.read().await;
    let job = load(&s.db.lock().unwrap(), id)?;
    if cancelled(s, id) {
        return Err("Worktree setup cancelled.".into());
    }
    s.db.lock()
        .unwrap()
        .execute(
            "UPDATE thread_launches SET phase='preparing' WHERE id=?",
            [id],
        )
        .map_err(|e| e.to_string())?;
    let project = get(
        &s.db.lock().unwrap(),
        "project",
        job.plan["projectId"].as_str().ok_or("Project missing")?,
    )?;
    if !project["deletedAt"].is_null() {
        return Err("Project was deleted".into());
    }
    let root = tokio::fs::canonicalize(
        project["workspaceRoot"]
            .as_str()
            .ok_or("Workspace root missing")?,
    )
    .await
    .map_err(|e| e.to_string())?;
    s.db.lock()
        .unwrap()
        .execute(
            "UPDATE thread_launches SET root=?1 WHERE id=?2",
            params![root.to_string_lossy(), id],
        )
        .map_err(|e| e.to_string())?;
    let strategy = &job.plan["workspaceStrategy"];
    let mut path = None;
    let mut branch = strategy["branch"].as_str().map(str::to_owned);
    let existing = get(&s.db.lock().unwrap(), "thread", &job.tid)?;
    if existing["worktreePath"] != job.plan["expectedWorktreePath"]
        || existing["branch"] != job.plan["expectedBranch"]
    {
        return Err("The thread workspace changed before preparation".into());
    }
    let reused = job.plan["reusedWorktree"].is_object();
    match strategy["type"].as_str() {
        Some("root") => {}
        Some("existing_worktree") => {
            let work = tokio::fs::canonicalize(
                strategy["worktreePath"]
                    .as_str()
                    .ok_or("worktreePath required")?,
            )
            .await
            .map_err(|e| e.to_string())?;
            if !work.is_dir() {
                return Err("The workspace is not a directory".into());
            }
            path = Some(work);
        }
        Some("worktree") => {
            progress(s, id, &job, "worktree")?;
            let _repo = proc::lock_repo(&root).await;
            check(&cancel)?;
            let base = strategy["baseRef"].as_str().ok_or("baseRef required")?;
            if base.contains('\0') || (base.starts_with('-') && base != "-") {
                return Err("Invalid baseRef".into());
            }
            let settings =
                crate::source_control::project_settings(s, project["id"].as_str().unwrap_or(""));
            let start_from_origin = strategy["startFromOrigin"]
                .as_bool()
                .unwrap_or(settings["newWorktreesStartFromOrigin"] == true);
            tracker::stage(
                s,
                id,
                "fetch",
                if start_from_origin {
                    "running"
                } else {
                    "skipped"
                },
                None,
            )?;
            let remote = if start_from_origin {
                tokio::select! {r=crate::worktree_handoff::remote_base(&root,base)=>r.map_err(|e|e.to_string())?,_=cancel.changed()=>return Err("Worktree setup cancelled.".into())}
            } else {
                None
            };
            tracker::stage(
                s,
                id,
                "fetch",
                if remote.is_some() { "done" } else { "skipped" },
                None,
            )?;
            let fallback = if base == "-" { "@{-1}" } else { base };
            let reference = remote.as_ref().map(|r| r.0.as_str()).unwrap_or(fallback);
            // Git's :/message syntax already names a commit; appending a type
            // suffix changes the search text instead of verifying the type.
            let commit_ref = if reference.starts_with(':') {
                reference.to_owned()
            } else {
                format!("{reference}^{{commit}}")
            };
            let commit = g(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &commit_ref,
                ],
                Opts::READ,
                &cancel,
            )
            .await?;
            let mut name = branch
                .clone()
                .unwrap_or(format!("t3/{}", &crate::store::random_id()?[..8]));
            if crate::launch_names::temporary(&name)
                && proc::git_controlled(
                    &root,
                    &["show-ref", "--verify", "--quiet", "refs/heads/t3"],
                    Opts::READ,
                    cancel.clone(),
                )
                .await
                .is_ok_and(|v| v.ok())
            {
                name = name.replacen("t3/", "t3-", 1);
            }
            if name.starts_with('-') {
                return Err("Invalid worktree branch".into());
            }
            g(
                &root,
                &["check-ref-format", "--branch", &name],
                Opts::READ,
                &cancel,
            )
            .await?;
            let exists = proc::git_controlled(
                &root,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{name}"),
                ],
                Opts::READ,
                cancel.clone(),
            )
            .await
            .map_err(|e| format!("Git preparation failed: {e:?}"))?;
            if exists.ok() {
                return Err("Worktree branch already exists".into());
            }
            let destination = crate::worktree_handoff::managed_path(s, &root, &name, &settings)
                .await
                .map_err(|e| e.to_string())?;
            let checkout = destination.to_str().ok_or("Invalid worktree path")?;
            s.db.lock()
                .unwrap()
                .execute(
                    "UPDATE thread_launches SET path=?1,branch=?2 WHERE id=?3",
                    params![checkout, name, id],
                )
                .map_err(|e| e.to_string())?;
            tracker::stage(s, id, "checkout", "running", None)?;
            g(
                &root,
                &[
                    "update-ref",
                    "--create-reflog",
                    "-m",
                    &format!("o3 launch {id}"),
                    &format!("refs/heads/{name}"),
                    &commit,
                    &"0".repeat(commit.len()),
                ],
                Opts::WRITE,
                &cancel,
            )
            .await?;
            s.db.lock()
                .unwrap()
                .execute("UPDATE thread_launches SET owned=1 WHERE id=?", [id])
                .map_err(|e| e.to_string())?;
            g(
                &root,
                &["worktree", "add", "--", checkout, &name],
                Opts {
                    timeout: Duration::ZERO,
                    ..Opts::WRITE
                },
                &cancel,
            )
            .await?;
            let base_ref = remote.as_ref().map(|r| r.1.as_str()).unwrap_or(base);
            if let Some((short, full)) = crate::worktree_handoff::comparison_base(&root, base_ref)
                .await
                .map_err(|e| e.to_string())?
            {
                g(
                    &root,
                    &[
                        "config",
                        "--local",
                        &format!("branch.{name}.t3-merge-base"),
                        &short,
                    ],
                    Opts::WRITE,
                    &cancel,
                )
                .await?;
                if full.starts_with("refs/remotes/") {
                    g(
                        &root,
                        &[
                            "config",
                            "--local",
                            &format!("branch.{name}.t3-merge-base-ref"),
                            &full,
                        ],
                        Opts::WRITE,
                        &cancel,
                    )
                    .await?;
                }
            }
            if destination.join(".gitmodules").is_file() {
                let mode = settings["worktreeSubmodules"]
                    .as_str()
                    .unwrap_or("recursive");
                if mode != "none" {
                    tracker::stage(
                        s,
                        id,
                        "checkout",
                        "running",
                        Some("Initializing submodules"),
                    )?;
                    let mut args = vec!["submodule", "update", "--init"];
                    if mode != "top-level" {
                        args.push("--recursive");
                    }
                    g(
                        &destination,
                        &args,
                        Opts {
                            timeout: Duration::ZERO,
                            ..Opts::WRITE
                        },
                        &cancel,
                    )
                    .await?;
                }
            }
            tracker::stage(s, id, "checkout", "done", None)?;
            path = Some(destination);
            branch = Some(name);
        }
        _ => return Err("Invalid workspace strategy".into()),
    }
    check(&cancel)?;
    let mut detaching = false;
    {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut t = get(&tx, "thread", &job.tid)?;
        let p = get(&tx, "project", job.plan["projectId"].as_str().unwrap_or(""))?;
        if !t["deletedAt"].is_null()
            || !t["archivedAt"].is_null()
            || t["projectId"] != project["id"]
            || p["workspaceRoot"] != project["workspaceRoot"]
            || !p["deletedAt"].is_null()
        {
            return Err("The launch target changed during preparation".into());
        }
        if t["worktreePath"] != existing["worktreePath"] || t["branch"] != existing["branch"] {
            return Err("The thread workspace changed during preparation".into());
        }
        if let Some(rid) = &job.rid {
            if get(&tx, "runs", rid)?["status"] != "preparing" {
                return Err("Worktree setup cancelled.".into());
            }
        }
        if !reused {
            if job.rid.is_none() {
                detaching = crate::workspace_transition::plan(
                    &tx,
                    &t,
                    &json!({"commandId":format!("{id}:workspace"),"worktreePath":path,"branch":branch,"expectedWorktreePath":existing["worktreePath"],"expectedBranch":existing["branch"]}),
                    &iso(now_ms()),
                )?;
            }
            let owned = strategy["type"] == "worktree"
                || (t["ownedWorktree"] == true && t["worktreePath"] == json!(path));
            t["worktreePath"] = json!(path);
            t["branch"] = json!(branch);
            t["ownedWorktree"] = json!(owned);
            t["updatedAt"] = json!(iso(now_ms()));
            put(&tx, "thread", &job.tid, &job.tid, &t)?;
            append(&tx, &job.tid, "thread.metadata-updated", t.clone(), &t)?;
        }
        tx.execute(
            "UPDATE thread_launches SET phase='bound',bound=1 WHERE id=?",
            [id],
        )
        .map_err(|e| e.to_string())?;
        tracker::update(&tx, id, |v| {
            v["worktreePath"] = json!(path);
            v["branch"] = json!(branch);
        })?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    drop(workspace);
    if detaching {
        s.executions.cancel(&job.tid);
    }
    s.notify();
    if !reused && job.plan["initialMessage"].is_object() {
        if let (Some(path), Some(branch)) = (&path, &branch) {
            if crate::launch_names::temporary(branch) {
                *naming = Some(crate::launch_names::start(
                    s.clone(),
                    crate::launch_names::Naming {
                        id: id.into(),
                        tid: job.tid.clone(),
                        root: root.clone(),
                        path: path.clone(),
                        old: branch.clone(),
                        message: job.plan["initialMessage"].clone(),
                        project: job.plan["projectId"].as_str().unwrap_or("").into(),
                    },
                    cancel.clone(),
                ));
            }
        }
    }
    let mut observer = None;
    let mut synchronous = false;
    if let Some(path) = &path {
        progress(s, id, &job, "setup")?;
        let settings = crate::settings::read(s)?;
        let t = get(&s.db.lock().unwrap(), "thread", &job.tid)?;
        let fenced = t
            .get("linkOrigin")
            .is_some_and(|v| !v.is_null() && v["access"] != "full-access");
        if let Some(script) =
            crate::worktree_handoff::setup_script(&settings, &project).filter(|_| !fenced)
        {
            check(&cancel)?;
            tracker::stage(s, id, "setup-script", "running", None)?;
            let terminal = format!("setup-{}", script["id"].as_str().unwrap_or("project"));
            s.db.lock()
                .unwrap()
                .execute(
                    "UPDATE thread_launches SET script_started=1,terminal_id=?1 WHERE id=?2",
                    params![terminal, id],
                )
                .map_err(|e| e.to_string())?;
            tracker::update(
                &s.db.lock().unwrap(),
                id,
                |v| v["setupScript"] = json!({"name":script["name"],"command":script["command"],"terminalId":terminal}),
            )?;
            let input = json!({"threadId":job.tid,"terminalId":terminal,"cwd":path,"worktreePath":path,"_setupTrackingId":id,"providerInstanceId":t["providerInstanceId"],"env":{"T3CODE_PROJECT_ROOT":root,"T3CODE_WORKTREE_PATH":path,"COLORTERM":"","NO_COLOR":"1","FORCE_COLOR":"0"}});
            let command = script["command"]
                .as_str()
                .ok_or("Setup command missing")?
                .to_owned();
            if strategy["type"] == "worktree" || reused {
                observer = Some(
                    crate::terminal::observed_project_script(s, &input, command)
                        .await
                        .map_err(|e| e.to_string())?,
                );
                synchronous = script["async"] == false;
            } else {
                crate::terminal::project_script(s, &input, command)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        } else {
            tracker::stage(s, id, "setup-script", "skipped", None)?;
        }
    } else {
        tracker::stage(s, id, "setup-script", "skipped", None)?;
    }
    if synchronous {
        if let Some(observe) = observer.as_mut() {
            observe_setup(s, id, observe, &mut cancel, true).await?;
        }
        observer = None;
    }
    check(&cancel)?;
    tracker::stage(s, id, "agent", "running", None)?;
    release(s, id, &job)?;
    tracker::stage(s, id, "agent", "done", None)?;
    if let Some(mut observe) = observer {
        let _ = observe_setup(s, id, &mut observe, &mut cancel, false).await;
    }
    {
        let db = s.db.lock().unwrap();
        tracker::finish(&db, id, "done", None)?;
        db.execute(
            "UPDATE thread_launches SET completed=1,phase='done' WHERE id=?",
            [id],
        )
        .map_err(|e| e.to_string())?;
    }
    retain(s);
    s.notify();
    Ok(())
}
async fn observe_setup(
    s: &State,
    id: &str,
    observer: &mut crate::terminal::ScriptObserver,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    sync: bool,
) -> Result<()> {
    let result = tokio::select! {r=observer.wait(|line|{let _=tracker::tail(s,id,&line);})=>r,_=cancel.changed()=>return Err("Worktree setup cancelled.".into())};
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            tracker::stage(s, id, "setup-script", "failed", Some(&error))?;
            return if sync { Err(error) } else { Ok(()) };
        }
    };
    tracker::stage(
        s,
        id,
        "setup-script",
        if result == Some(0) { "done" } else { "failed" },
        Some(&format!(
            "exited with {}",
            result
                .map(|c| c.to_string())
                .unwrap_or("no exit code".into())
        )),
    )?;
    if sync && result != Some(0) {
        return Err(format!(
            "Setup script exited with {}.",
            result
                .map(|c| c.to_string())
                .unwrap_or("no exit code".into())
        ));
    }
    Ok(())
}
fn progress(s: &State, _id: &str, job: &Job, phase: &str) -> Result<()> {
    if let Some(rid) = &job.rid {
        let db = s.db.lock().unwrap();
        let t = get(&db, "thread", &job.tid)?;
        let item_id = format!("workspace-preparation:{rid}");
        let mut item = get(&db, "turnItems", &item_id)?;
        item["title"] = json!(if phase == "worktree" {
            "Preparing worktree"
        } else {
            "Starting setup script"
        });
        item["updatedAt"] = json!(iso(now_ms()));
        crate::worktree_setup::save_item(&db, &job.tid, &t, item)?;
    }
    s.notify();
    Ok(())
}
fn release(s: &Arc<State>, id: &str, job: &Job) -> Result<()> {
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let t = get(&tx, "thread", &job.tid)?;
    if !t["archivedAt"].is_null() || !t["deletedAt"].is_null() {
        return Err("Worktree setup cancelled.".into());
    }
    if tx
        .query_row(
            "SELECT cancelled!=0 FROM thread_launches WHERE id=?",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?
    {
        return Err("Worktree setup cancelled.".into());
    }
    let mut text = None;
    if let Some(rid) = &job.rid {
        let mut run = get(&tx, "runs", rid)?;
        if run["status"] != "preparing" {
            return Err("Worktree setup cancelled.".into());
        }
        crate::thread_launch::enabled(
            s,
            t["projectId"].as_str().unwrap_or(""),
            &run["providerInstanceId"],
        )
        .map_err(crate::provider_policy::display_error)?;
        if s.provider.lock().unwrap().is_none() {
            return Err("No provider adapter configured for prepared run".into());
        }
        let at = iso(now_ms());
        let mut item = get(&tx, "turnItems", &format!("workspace-preparation:{rid}"))?;
        item["status"] = json!("completed");
        item["title"] = json!("Workspace ready");
        item["output"] = json!("Workspace preparation completed.");
        item["exitCode"] = json!(0);
        item["completedAt"] = json!(at);
        item["updatedAt"] = json!(at);
        crate::worktree_setup::save_item(&tx, &job.tid, &t, item)?;
        run["status"] = json!("starting");
        run["executionRequested"] = json!(true);
        put(&tx, "runs", rid, &job.tid, &run)?;
        append(&tx, &job.tid, "run.updated", run.clone(), &t)?;
        text = Some(
            get(
                &tx,
                "messages",
                run["userMessageId"].as_str().ok_or("Message missing")?,
            )?["text"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
        );
    }
    tx.execute(
        "UPDATE thread_launches SET uncancellable=1,released=1 WHERE id=?",
        [id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    if let (Some(rid), Some(text)) = (&job.rid, text) {
        crate::provider::start(s.clone(), job.tid.clone(), rid.clone(), text);
    }
    Ok(())
}
fn retain(s: &Arc<State>) {
    let weak = Arc::downgrade(s);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if let Some(s) = weak.upgrade() {
            s.notify();
        }
    });
}
pub(crate) async fn fail(s: &Arc<State>, id: &str, error: &str) -> Result<()> {
    if s.db
        .lock()
        .unwrap()
        .query_row(
            "SELECT completed!=0 FROM thread_launches WHERE id=?",
            [id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?
    {
        return Err(error.into());
    }
    let job = load(&s.db.lock().unwrap(), id)?;
    let cancellation = cancelled(s, id) || error == "Worktree setup cancelled.";
    s.db.lock()
        .unwrap()
        .execute(
            "UPDATE thread_launches SET phase='cleanup',error=?1 WHERE id=?2",
            params![tracker::clamp(error, 1000), id],
        )
        .map_err(|e| e.to_string())?;
    // Drain the owned terminal before deleting any checkout or allowing a retry.
    let terminal: Option<String> =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT terminal_id FROM thread_launches WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
    let interrupted = error.starts_with("Server restarted")
        || error.starts_with("Interrupted workspace preparation");
    if let Some(terminal) = terminal.filter(|_| cancellation || interrupted) {
        crate::terminal::call(
            s,
            "terminal.close",
            &json!({"threadId":job.tid,"terminalId":terminal}),
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    let cleanup_error = cleanup(s, id, cancellation).await.err();
    if let Some(error) = &cleanup_error {
        let _ = tracker::stage(s, id, "checkout", "warning", Some(error));
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    fail_projection(&tx, &job, error)?;
    if tracker::finish(
        &tx,
        id,
        if cancellation { "cancelled" } else { "failed" },
        if cancellation { None } else { Some(error) },
    )
    .is_err()
    {
        tx.execute("UPDATE thread_launches SET snapshot=NULL WHERE id=?", [id])
            .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE thread_launches SET completed=1,error=?1,phase=?2,cleanup_pending=?4,retry_at=?5 WHERE id=?3",
        params![
            tracker::clamp(error, 1000),
            if cancellation { "cancelled" } else { "failed" },
            id,
            cleanup_error.is_some(),
            now_ms()+1000,
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    if cleanup_error.is_some() {
        let weak = Arc::downgrade(s);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some(s) = weak.upgrade() {
                s.notify();
            }
        });
    }
    retain(s);
    s.notify();
    Ok(())
}
fn fail_projection(db: &Connection, job: &Job, error: &str) -> Result<()> {
    let Some(rid) = &job.rid else {
        return Ok(());
    };
    let t = get(db, "thread", &job.tid)?;
    let mut run = get(db, "runs", rid)?;
    if run["status"] != "preparing" {
        return Ok(());
    }
    let at = iso(now_ms());
    run["status"] = json!("failed");
    run["completedAt"] = json!(at);
    run["executionRequested"] = json!(false);
    run["error"] = json!(tracker::clamp(error, 1000));
    put(db, "runs", rid, &job.tid, &run)?;
    append(db, &job.tid, "run.updated", run.clone(), &t)?;
    crate::projection::finish(db, &job.tid, rid, &t, &at, "failed")?;
    let item_id = format!("workspace-preparation:{rid}");
    let mut item = get(db, "turnItems", &item_id)?;
    item["status"] = json!("failed");
    item["title"] = json!("Workspace preparation failed");
    item["output"] = json!(tracker::clamp(error, 1000));
    item["exitCode"] = json!(1);
    item["completedAt"] = json!(at);
    item["updatedAt"] = json!(at);
    crate::worktree_setup::save_item(db, &job.tid, &t, item)?;
    let failure = json!({"id":format!("workspace-preparation-failure:{rid}"),"threadId":job.tid,"runId":rid,"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",[&job.tid],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"status":"failed","title":"Workspace preparation failed","startedAt":at,"completedAt":at,"updatedAt":at,"type":"error","failure":{"code":"workspace_preparation_failed","message":tracker::clamp(error,1000),"class":"validation_error","retryable":false}});
    crate::worktree_setup::save_item(db, &job.tid, &t, failure)?;
    crate::thread_commands::hold_queue(db, &job.tid, &t)?;
    Ok(())
}
async fn cleanup(s: &State, id: &str, cancellation: bool) -> Result<()> {
    let _workspace = s.worktree_gate.read().await;
    let (root, path, branch, owned, bound, released): (
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        bool,
        bool,
    ) =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT root,path,branch,owned,bound,released FROM thread_launches WHERE id=?",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
    if released || (!cancellation && bound) {
        return Ok(());
    }
    let (Some(root), Some(path)) = (root, path) else {
        return Ok(());
    };
    let job = load(&s.db.lock().unwrap(), id)?;
    let _repo = proc::lock_repo(Path::new(&root)).await;
    let rename: Option<String> =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT rename_to FROM thread_launches WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
    let mut branch = branch;
    if let Some(next) = rename {
        if Path::new(&path).is_dir() {
            let actual = proc::git(
                Path::new(&path),
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                Opts::READ,
            )
            .await
            .map_err(|e| format!("Rename recovery failed: {e:?}"))?;
            if actual.ok() && actual.text().trim() == next {
                branch = Some(next);
            }
        }
    }

    if !owned {
        let Some(branch) = branch.as_deref() else {
            return Ok(());
        };
        let proof = proc::git(
            Path::new(&root),
            &[
                "reflog",
                "show",
                "-1",
                "--format=%gs",
                &format!("refs/heads/{branch}"),
            ],
            Opts::READ,
        )
        .await
        .map_err(|e| format!("Ownership verification failed: {e:?}"))?;
        if !proof.ok() || proof.text().trim() != format!("o3 launch {id}") {
            return Ok(());
        }
    }
    let shared:bool=s.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='thread' AND id!=?1 AND json_extract(body,'$.worktreePath')=?2 AND json_extract(body,'$.deletedAt') IS NULL)",params![job.tid,path],|r|r.get(0)).map_err(|e|e.to_string())?;
    if shared {
        s.db.lock()
            .unwrap()
            .execute("UPDATE thread_launches SET owned=0 WHERE id=?", [id])
            .map_err(|e| e.to_string())?;
        tracker::stage(
            s,
            id,
            "checkout",
            "warning",
            Some("Created checkout retained because another thread now uses it"),
        )?;
        return Ok(());
    }
    if Path::new(&path).exists() {
        let out = proc::git(
            Path::new(&root),
            &["worktree", "remove", "--force", "--", &path],
            Opts::WRITE,
        )
        .await
        .map_err(|e| format!("Cleanup failed: {e:?}"))?;
        if !out.ok() {
            return Err(format!("Checkout cleanup failed: {}", out.stderr));
        }
    }
    if let Some(branch) = branch {
        let exists = proc::git(
            Path::new(&root),
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
            Opts::READ,
        )
        .await
        .map_err(|e| format!("Branch cleanup verification failed: {e:?}"))?;
        if exists.code != Some(1) {
            if !exists.ok() {
                return Err(format!(
                    "Branch cleanup verification failed: {}",
                    exists.stderr
                ));
            }
            let out = proc::git(
                Path::new(&root),
                &["branch", "-D", "--", &branch],
                Opts::WRITE,
            )
            .await
            .map_err(|e| format!("Branch cleanup failed: {e:?}"))?;
            if !out.ok() {
                return Err(format!("Branch cleanup failed: {}", out.stderr));
            }
        }
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut t = get(&tx, "thread", &job.tid)?;
    if t["worktreePath"] == path {
        t["worktreePath"] = Value::Null;
        t["branch"] = Value::Null;
        t["ownedWorktree"] = json!(false);
        put(&tx, "thread", &job.tid, &job.tid, &t)?;
        append(&tx, &job.tid, "thread.metadata-updated", t.clone(), &t)?;
    }
    tx.execute(
        "UPDATE thread_launches SET owned=0,path=NULL,branch=NULL WHERE id=?",
        [id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}
