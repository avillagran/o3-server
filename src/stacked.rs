//! `git.runStackedAction`: branch → commit → push → pull request as one progress stream.
//! Everything runs as argv subprocesses under the repository lock; the stream carries
//! `action_started`, `phase_started` and exactly one `action_finished` or `action_failed`.
use crate::{
    paths,
    proc::{self, Opts},
    vcs::{self, git_fail, ok, out, simple_fail, toplevel},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Notify};

pub enum Item {
    Event(Value),
    End(Result<(), Value>),
}

/// Producer side of the action stream: a bounded queue plus a wake-up for the socket loop.
#[derive(Clone)]
pub struct Tx {
    pub queue: Sender<Item>,
    pub wake: Arc<Notify>,
}
impl Tx {
    async fn send(&self, item: Item) -> bool {
        let sent = self.queue.send(item).await.is_ok();
        self.wake.notify_one();
        sent
    }
}

const ACTIONS: &[&str] = &[
    "commit",
    "push",
    "create_pr",
    "commit_push",
    "commit_push_pr",
];

fn phases(action: &str, feature_branch: bool) -> Vec<&'static str> {
    let mut p = Vec::new();
    if feature_branch {
        p.push("branch");
    }
    if action.contains("commit") {
        p.push("commit");
    }
    if action.contains("push") {
        p.push("push");
    }
    if action.contains("pr") {
        p.push("pr");
    }
    p
}

/// Validates the request before any stream starts.
pub fn validate(p: &Value) -> Result<(), Value> {
    let bad = |m: &str| json!({"_tag":"GitManagerError","operation":"git.runStackedAction","cwd":p["cwd"],"detail":m,"message":m});
    if !p["actionId"].as_str().is_some_and(|a| !a.trim().is_empty()) {
        return Err(bad("actionId is required"));
    }
    if !p["action"].as_str().is_some_and(|a| ACTIONS.contains(&a)) {
        return Err(bad("action is not supported"));
    }
    if p["commitMessage"]
        .as_str()
        .is_some_and(|m| m.chars().count() > 10_000)
    {
        return Err(bad("commitMessage is longer than 10000 characters"));
    }
    if let Some(files) = p.get("filePaths") {
        if !files.as_array().is_some_and(|a| {
            !a.is_empty()
                && a.iter().all(|f| {
                    f.as_str()
                        .is_some_and(|f| !f.is_empty() && !f.contains('\0'))
                })
        }) {
            return Err(bad("filePaths must hold at least one path"));
        }
    }
    if vcs::existing_dir(p).is_none() {
        return Err(simple_fail(
            "git.runStackedAction",
            Path::new(p["cwd"].as_str().unwrap_or("")),
            "The working directory does not exist.",
            Some("not_a_repository"),
        ));
    }
    Ok(())
}

struct Run {
    state: Arc<crate::store::State>,
    p: Value,
    tx: Tx,
    root: PathBuf,
}
impl Run {
    async fn event(&self, mut e: Value) -> bool {
        e["actionId"] = self.p["actionId"].clone();
        e["cwd"] = self.p["cwd"].clone();
        e["action"] = self.p["action"].clone();
        self.tx.send(Item::Event(e)).await
    }
}

fn slug(text: &str) -> String {
    let s: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "changes".into()
    } else {
        s.chars().take(40).collect()
    }
}

pub async fn run(state: Arc<crate::store::State>, p: Value, tx: Tx) {
    let result = tokio::select! {
        result = drive(state,p, tx.clone()) => result,
        _ = tx.queue.closed() => return,
    };
    let _ = tx.send(Item::End(result)).await;
}

async fn drive(state: Arc<crate::store::State>, p: Value, tx: Tx) -> Result<(), Value> {
    let op = "git.runStackedAction";
    let cwd = vcs::existing_dir(&p).ok_or_else(|| {
        simple_fail(
            op,
            Path::new(""),
            "The working directory does not exist.",
            None,
        )
    })?;
    let action = p["action"].as_str().unwrap_or("").to_owned();
    let feature = p["featureBranch"] == true;
    let Some(root) = toplevel(&cwd).await else {
        let e = simple_fail(
            op,
            &cwd,
            "The working directory is not inside a Git repository.",
            Some("not_a_repository"),
        );
        let run = Run {
            state,
            p,
            tx,
            root: PathBuf::new(),
        };
        run.event(json!({"kind":"action_failed","phase":null,"message":"The working directory is not inside a Git repository."})).await;
        return Err(e);
    };
    let run = Run {
        state,
        p,
        tx,
        root: root.clone(),
    };
    let _guard = proc::lock_repo(&root).await;
    run.event(json!({"kind":"action_started","phases":phases(&action, feature)}))
        .await;
    let mut phase = "branch";
    match steps(&run, &action, feature, &mut phase).await {
        Ok(result) => {
            run.event(json!({"kind":"action_finished","result":result}))
                .await;
            Ok(())
        }
        Err((message, err)) => {
            run.event(json!({"kind":"action_failed","phase":phase,"message":message}))
                .await;
            Err(err)
        }
    }
}

type StepErr = (String, Value);
fn step_err(e: Value) -> StepErr {
    (
        e["detail"]
            .as_str()
            .or(e["cause"].as_str())
            .unwrap_or("The git command failed.")
            .to_owned(),
        e,
    )
}
fn fail(op: &str, cwd: &Path, message: &str, reason: Option<&str>) -> StepErr {
    (message.to_owned(), simple_fail(op, cwd, message, reason))
}

async fn steps(
    run: &Run,
    action: &str,
    feature: bool,
    phase: &mut &'static str,
) -> Result<Value, StepErr> {
    let op = "git.runStackedAction";
    let root = &run.root;
    let do_commit = action.contains("commit");
    let do_push = action.contains("push");
    let do_pr = action.contains("pr");
    let mut result = json!({"action":action,"branch":{"status":"skipped_not_requested"},"commit":{"status":"skipped_not_requested"},
        "push":{"status":"skipped_not_requested"},"pr":{"status":"skipped_not_requested"}});

    let message = run.p["commitMessage"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    if feature {
        *phase = "branch";
        run.event(
            json!({"kind":"phase_started","phase":"branch","label":"Creating feature branch"}),
        )
        .await;
        let settings = crate::source_control::effective(&run.state, root);
        let mut branch_text = message.clone().unwrap_or_else(|| "changes".into());
        if settings["branchNamingMode"] != "static" {
            branch_text = crate::source_control::generate(&run.state, root, "branch", &branch_text)
                .await
                .unwrap_or(branch_text);
        }
        let prefix = settings["branchNamePrefix"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("t3");
        let name = format!(
            "{}/{}-{}",
            slug(prefix),
            slug(&branch_text),
            &crate::store::random_id().unwrap_or_default()[..6]
        );
        ok(op, root, &["switch", "-c", &name], Opts::WRITE)
            .await
            .map_err(step_err)?;
        result["branch"] = json!({"status":"created","name":name});
    }

    if do_commit {
        *phase = "commit";
        run.event(json!({"kind":"phase_started","phase":"commit","label":"Committing changes"}))
            .await;
        // Paths named by the client must stay inside the repository.
        let specs: Option<Vec<String>> = match run.p["filePaths"].as_array() {
            Some(files) => {
                let mut v = Vec::new();
                for f in files.iter().filter_map(Value::as_str) {
                    let abs = if Path::new(f).is_absolute() {
                        PathBuf::from(f)
                    } else {
                        root.join(f)
                    };
                    let rel = paths::normalize(&abs)
                        .strip_prefix(root)
                        .ok()
                        .map(|r| r.to_string_lossy().into_owned())
                        .filter(|r| !r.is_empty());
                    match rel {
                        Some(r) => v.push(format!(":(top,literal){r}")),
                        None => {
                            return Err(fail(
                                op,
                                root,
                                &format!("File path '{f}' is outside the repository."),
                                None,
                            ))
                        }
                    }
                }
                Some(v)
            }
            None => None,
        };
        match &specs {
            Some(s) => {
                let mut a = vec!["add", "-A", "--"];
                a.extend(s.iter().map(String::as_str));
                ok(op, root, &a, Opts::WRITE).await.map_err(step_err)?;
            }
            None => {
                ok(op, root, &["add", "-A"], Opts::WRITE)
                    .await
                    .map_err(step_err)?;
            }
        }
        let mut diff_args = vec!["diff", "--cached", "--quiet"];
        if let Some(s) = &specs {
            diff_args.push("--");
            diff_args.extend(s.iter().map(String::as_str));
        }
        let staged = out(op, root, &diff_args, Opts::READ)
            .await
            .map_err(step_err)?;
        if staged.code == Some(0) {
            result["commit"] = json!({"status":"skipped_no_changes"});
        } else {
            let subject = match &message {
                Some(m) => m.clone(),
                None => {
                    let mut names_args = vec!["diff", "--cached", "--name-only"];
                    if let Some(s) = &specs {
                        names_args.push("--");
                        names_args.extend(s.iter().map(String::as_str));
                    }
                    let names = out(op, root, &names_args, Opts::READ)
                        .await
                        .map_err(step_err)?
                        .text();
                    let files: Vec<&str> = names.lines().collect();
                    let fallback = match files.as_slice() {
                        [one] => format!("Update {one}"),
                        many => format!("Update {} files", many.len()),
                    };
                    let mut args = vec!["diff", "--cached"];
                    if let Some(specs) = &specs {
                        args.push("--");
                        args.extend(specs.iter().map(String::as_str));
                    }
                    let diff = proc::git(
                        root,
                        &args,
                        Opts {
                            cap: 16000,
                            allow_truncate: true,
                            ..Opts::READ
                        },
                    )
                    .await
                    .ok()
                    .map(|o| o.text())
                    .unwrap_or_default();
                    crate::title::check_settings(
                        &run.state,
                        &crate::source_control::effective(&run.state, root),
                        "sourceControlWriterModelSelection",
                    )
                    .map_err(|message| fail(op, root, &message, None))?;
                    crate::source_control::generate(&run.state, root, "commit", &diff)
                        .await
                        .unwrap_or(fallback)
                }
            };
            let mut a = vec!["commit", "-m", subject.as_str()];
            if let Some(s) = &specs {
                a.push("--only");
                a.push("--");
                a.extend(s.iter().map(String::as_str));
            }
            ok(op, root, &a, Opts::WRITE).await.map_err(step_err)?;
            let sha = out(op, root, &["rev-parse", "HEAD"], Opts::READ)
                .await
                .map_err(step_err)?
                .text()
                .trim()
                .to_owned();
            result["commit"] = json!({"status":"created","commitSha":sha,"subject":subject.lines().next().unwrap_or("")});
        }
    }

    let mut branch: Option<String> = None;
    if do_push || do_pr {
        let b = out(op, root, &["branch", "--show-current"], Opts::READ)
            .await
            .map_err(step_err)?
            .text()
            .trim()
            .to_owned();
        if b.is_empty() {
            return Err(fail(
                op,
                root,
                "Cannot push or open a pull request from a detached HEAD.",
                None,
            ));
        }
        branch = Some(b);
    }

    if do_push {
        *phase = "push";
        run.event(json!({"kind":"phase_started","phase":"push","label":"Pushing to remote"}))
            .await;
        let b = branch.clone().unwrap_or_default();
        let upstream = out(
            op,
            root,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
            Opts::READ,
        )
        .await
        .map_err(step_err)?;
        if upstream.ok() {
            let up = upstream.text().trim().to_owned();
            let ahead = out(
                op,
                root,
                &["rev-list", "--count", "@{upstream}..HEAD"],
                Opts::READ,
            )
            .await
            .map_err(step_err)?
            .text()
            .trim()
            .parse::<u64>()
            .unwrap_or(1);
            if ahead == 0 {
                result["push"] =
                    json!({"status":"skipped_up_to_date","branch":b,"upstreamBranch":up});
            } else {
                ok(op, root, &["push"], Opts::NETWORK)
                    .await
                    .map_err(step_err)?;
                result["push"] = json!({"status":"pushed","branch":b,"upstreamBranch":up});
            }
        } else {
            let remotes = out(op, root, &["remote"], Opts::READ)
                .await
                .map_err(step_err)?
                .text();
            let remote = remotes
                .lines()
                .find(|r| *r == "origin")
                .or_else(|| remotes.lines().next())
                .map(str::to_owned);
            let Some(remote) = remote else {
                return Err(fail(
                    op,
                    root,
                    "No remote is configured to push to.",
                    Some("remote_unreachable"),
                ));
            };
            ok(op, root, &["push", "-u", &remote, &b], Opts::NETWORK)
                .await
                .map_err(step_err)?;
            result["push"] = json!({"status":"pushed","branch":b,"upstreamBranch":format!("{remote}/{b}"),"setUpstream":true});
        }
    }

    if do_pr {
        *phase = "pr";
        run.event(json!({"kind":"phase_started","phase":"pr","label":"Opening pull request"}))
            .await;
        let head = branch.clone().unwrap_or_default();
        result["pr"] = pull_request(&run.state, root, &head).await?;
    }
    result["toast"] = toast(&result, action);
    Ok(result)
}

fn gh_unavailable(root: &Path, e: &str) -> StepErr {
    (
        format!("GitHub CLI (gh) is unavailable: {e}"),
        json!({"_tag":"GitManagerError","operation":"git.runStackedAction.pullRequest","cwd":root.to_string_lossy(),"detail":format!("GitHub CLI (gh) is unavailable: {e}"),"message":format!("GitHub CLI (gh) is unavailable: {e}")}),
    )
}

async fn pull_request(
    state: &crate::store::State,
    root: &Path,
    head: &str,
) -> Result<Value, StepErr> {
    let existing = proc::run(
        "gh",
        root,
        &[
            "pr",
            "list",
            "--head",
            head,
            "--state",
            "open",
            "--json",
            "url,number,title,baseRefName,headRefName",
            "--limit",
            "1",
        ],
        Opts::NETWORK,
        &[],
    )
    .await
    .map_err(|e| gh_unavailable(root, &format!("{e:?}")))?;
    if !existing.ok() {
        return Err(gh_unavailable(root, &existing.stderr));
    }
    if let Some(pr) = serde_json::from_slice::<Value>(&existing.stdout)
        .ok()
        .and_then(|v| v.get(0).cloned())
    {
        return Ok(
            json!({"status":"opened_existing","url":pr["url"],"number":pr["number"],"baseBranch":pr["baseRefName"],"headBranch":pr["headRefName"],"title":pr["title"]}),
        );
    }
    let base = vcs::default_branch(root).await.ok_or_else(|| {
        fail(
            "git.runStackedAction",
            root,
            "Could not determine the pull request base branch.",
            None,
        )
    })?;
    let title = out(
        "git.runStackedAction",
        root,
        &["log", "-1", "--format=%s"],
        Opts::READ,
    )
    .await
    .map_err(step_err)?
    .text()
    .trim()
    .to_owned();
    let body = out(
        "git.runStackedAction",
        root,
        &["log", "-1", "--format=%b"],
        Opts::READ,
    )
    .await
    .map_err(step_err)?
    .text()
    .trim()
    .to_owned();
    let diff = proc::git(
        root,
        &["diff", &format!("{base}...HEAD")],
        Opts {
            cap: 16000,
            allow_truncate: true,
            ..Opts::READ
        },
    )
    .await
    .ok()
    .map(|o| o.text())
    .unwrap_or_default();
    let body = crate::source_control::generate(
        state,
        root,
        "pull-request",
        &format!("{title}\n{body}\n{diff}"),
    )
    .await
    .unwrap_or(body);
    let created = proc::run(
        "gh",
        root,
        &[
            "pr", "create", "--title", &title, "--body", &body, "--base", &base, "--head", head,
        ],
        Opts::NETWORK,
        &[],
    )
    .await
    .map_err(|e| gh_unavailable(root, &format!("{e:?}")))?;
    if !created.ok() {
        let e = git_fail(
            "git.runStackedAction.pullRequest",
            root,
            &["pr", "create"],
            &created,
        );
        return Err(step_err(e));
    }
    let text = created.text();
    let url = text
        .lines()
        .rev()
        .find(|l| l.starts_with("http"))
        .unwrap_or("")
        .trim()
        .to_owned();
    let number = url.rsplit('/').next().and_then(|n| n.parse::<u64>().ok());
    let mut v =
        json!({"status":"created","url":url,"baseBranch":base,"headBranch":head,"title":title});
    if let Some(n) = number {
        v["number"] = json!(n);
    }
    Ok(v)
}

fn toast(r: &Value, action: &str) -> Value {
    let short = |s: &Value| {
        s.as_str()
            .map(|s| s.chars().take(7).collect::<String>())
            .unwrap_or_default()
    };
    if r["pr"]["status"] == "created" || r["pr"]["status"] == "opened_existing" {
        let n = r["pr"]["number"]
            .as_u64()
            .map(|n| format!("#{n}"))
            .unwrap_or_default();
        let verb = if r["pr"]["status"] == "created" {
            "Created"
        } else {
            "Opened"
        };
        return json!({"title":format!("{verb} PR {n}").trim().to_owned(),"description":r["pr"]["title"],"cta":{"kind":"open_pr","label":"View PR","url":r["pr"]["url"]}});
    }
    if r["push"]["status"] == "pushed" {
        let mut t = json!({"title":format!("Pushed {}", r["push"]["branch"].as_str().unwrap_or("branch")),"cta":{"kind":"none"}});
        if r["commit"]["status"] == "created" {
            t["description"] = r["commit"]["subject"].clone();
        }
        if action == "commit_push" {
            t["cta"] =
                json!({"kind":"run_action","label":"Create PR","action":{"kind":"create_pr"}});
        }
        return t;
    }
    if r["commit"]["status"] == "created" {
        return json!({"title":format!("Committed {}", short(&r["commit"]["commitSha"])),"description":r["commit"]["subject"],"cta":{"kind":"run_action","label":"Push","action":{"kind":"push"}}});
    }
    json!({"title":"Nothing to do","description":"No changes were committed or pushed.","cta":{"kind":"none"}})
}
