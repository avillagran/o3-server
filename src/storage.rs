//! Conservative policy collection, limited to worktrees created by this profile.
use crate::{
    events::{append, get, put},
    store::{iso, now_ms, State},
};
use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Command, Stdio},
};
fn git(path: &Path, args: &[&str]) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = Command::new("timeout")
        .args(["--kill-after=1", "10", "git", "-C"])
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = Vec::new();
    let read = child.stdout.take()?.take(65537).read_to_end(&mut stdout);
    if read.is_err() || stdout.len() > 65536 {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let status = child.wait().ok()?;
    status.success().then_some(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn age(v: &Value, now: i64) -> Option<i64> {
    time::OffsetDateTime::parse(v.as_str()?, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| now.saturating_sub(t.unix_timestamp() * 1000))
}
fn due(rules: &Value, key: &str, stamp: &Value, now: i64) -> bool {
    rules[key]
        .as_i64()
        .zip(age(stamp, now))
        .is_some_and(|(days, age)| age >= days * 86_400_000)
}
pub(crate) fn worktrees(s: &State, settings: &Value) -> Result<Vec<Value>, String> {
    // Reject competing mutations rather than parking an async worker on cleanup.
    // This reservation protects ownership checks while the database stays available.
    let _reservation = s
        .worktree_gate
        .try_write()
        .map_err(|_| "worktree mutation in progress; retry cleanup")?;
    let rows = {
        let db = s.db.lock().unwrap();
        let mut q = db
            .prepare("SELECT body FROM records WHERE kind='thread'")
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut entries = Vec::new();
    for body in rows {
        let mut t: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if t["ownedWorktree"] != true {
            continue;
        }
        let Some(path) = t["worktreePath"].as_str().map(std::path::PathBuf::from) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        let tid = t["id"].as_str().unwrap_or("").to_owned();
        let db = s.db.lock().unwrap();
        let blocked:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=? AND ((kind='runs' AND json_extract(body,'$.status') NOT IN ('completed','rolled_back','cancelled','failed','interrupted')) OR (kind='runtimeRequests' AND json_extract(body,'$.status')='pending') OR (kind IN ('subagents','backgroundTasks') AND json_extract(body,'$.status') NOT IN ('completed','failed','cancelled','interrupted')) OR (kind='controlOutbox' AND json_extract(body,'$.status') IN ('pending','delivery-unknown'))))",[&tid],|r|r.get(0)).map_err(|e|e.to_string())?;
        if blocked || t["pinnedAt"].is_string() {
            continue;
        }
        let shared:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='thread' AND id<>? AND json_extract(body,'$.worktreePath')=?)",rusqlite::params![tid,path.to_string_lossy()],|r|r.get(0)).map_err(|e|e.to_string())?;
        if shared {
            continue;
        }
        let project = get(&db, "project", t["projectId"].as_str().unwrap_or(""))?;
        let Some(root) = project["workspaceRoot"]
            .as_str()
            .map(std::path::PathBuf::from)
        else {
            continue;
        };
        drop(db);
        let Some(common) = git(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .and_then(|o| String::from_utf8(o.stdout).ok()) else {
            continue;
        };
        let Some(_guard) = crate::proc::try_lock_common_dir(common.trim().into()) else {
            continue;
        };
        let mut effective = settings.clone();
        if let Some(overrides) =
            settings["projectSettingsOverrides"].get(t["projectId"].as_str().unwrap_or(""))
        {
            crate::settings::merge(&mut effective, overrides);
        }
        let rules = match effective["worktreeCleanup"]["mode"].as_str() {
            Some("off") => continue,
            Some("custom") => &effective["worktreeCleanup"]["rules"],
            _ => &effective["storageCleanup"],
        };
        let keep = rules["worktreeKeepWhen"]
            .as_str()
            .unwrap_or("uncommitted-changes");
        let args = if keep == "tracked-changes" {
            vec!["status", "--porcelain", "--untracked-files=no"]
        } else {
            vec!["status", "--porcelain", "--untracked-files=all"]
        };
        let Some(status) = git(&path, &args) else {
            continue;
        };
        if !status.stdout.is_empty() {
            continue;
        }
        if keep == "any-local-files"
            && git(
                &path,
                &["ls-files", "--others", "--ignored", "--exclude-standard"],
            )
            .is_none_or(|o| !o.stdout.is_empty())
        {
            continue;
        }
        let Some(head) =
            git(&root, &["rev-parse", "HEAD"]).and_then(|o| String::from_utf8(o.stdout).ok())
        else {
            continue;
        };
        let now = now_ms();
        let eligible = due(rules, "worktreeAfterDays", &t["updatedAt"], now)
            || due(rules, "worktreeSettledAfterDays", &t["settledAt"], now)
            || rules["worktreeOnDelete"] == true && t["deletedAt"].is_string()
            || rules["worktreeOnMerge"] == true
                && git(&path, &["merge-base", "--is-ancestor", "HEAD", head.trim()]).is_some()
            || rules["worktreeUnchanged"] == true
                && git(&path, &["diff", "--quiet", "HEAD", head.trim(), "--"]).is_some();
        if !eligible {
            continue;
        }
        // One force permits files the selected policy allows discarding. Git still
        // refuses locked worktrees (which require two force flags).
        let mut args = vec!["worktree", "remove"];
        if keep != "any-local-files" {
            args.push("--force");
        }
        args.extend(["--", path.to_str().ok_or("invalid path")?]);
        let removed = git(&root, &args).is_some();
        entries.push(json!({"kind":"worktree","outcome":if removed{"removed"}else{"failed"},"reason":"cleanup policy","path":path,"projectPath":root,"threadId":tid,"threadTitle":t["title"],"bytes":0,"files":0}));
        if removed {
            let mut db = s.db.lock().unwrap();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            t = get(&tx, "thread", &tid)?;
            t["worktreePath"] = Value::Null;
            t["ownedWorktree"] = json!(false);
            t["updatedAt"] = json!(iso(now));
            put(&tx, "thread", &tid, &tid, &t)?;
            append(&tx, &tid, "thread.metadata-updated", t.clone(), &t)?;
            tx.commit().map_err(|e| e.to_string())?;
        }
    }
    Ok(entries)
}
