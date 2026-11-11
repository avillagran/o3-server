//! Source branch naming policy and cancellation-safe temporary-branch rename.
use crate::{
    events::{append, get, put},
    proc::{self, Opts},
    store::{iso, now_ms, State},
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use unicode_normalization::UnicodeNormalization;
pub(crate) fn temporary(name: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(||regex::Regex::new(r"^(?:t3[-/][0-9a-f]{8}|t3code(?:[-/][0-9a-f]{8}|/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}))$").unwrap()).is_match(&name.trim().to_lowercase())
}
fn fragment(raw: &str) -> String {
    let folded = raw
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect::<String>()
        .trim()
        .to_lowercase();
    let mut result = String::new();
    for c in folded.chars().filter(|c| !matches!(c, '\'' | '"' | '`')) {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-') {
            c
        } else {
            '-'
        };
        if matches!(c, '/' | '-') && result.ends_with(c) {
            continue;
        }
        result.push(c);
    }
    let result = result
        .trim_matches(['.', '/', '_', '-'])
        .chars()
        .take(64)
        .collect::<String>()
        .trim_end_matches(['.', '/', '_', '-'])
        .to_owned();
    if result.is_empty() {
        "update".into()
    } else {
        result
    }
}
fn formatted(raw: &str, settings: &Value) -> String {
    if settings["branchNamingMode"] == "custom" {
        return raw.trim().into();
    }
    let branch = fragment(raw);
    if settings["branchNamingMode"] != "static" {
        return branch;
    }
    let prefix = settings["branchNamePrefix"]
        .as_str()
        .unwrap_or("")
        .split('/')
        .map(|part| {
            let mut value = String::new();
            for c in part.chars() {
                let c = if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                    c
                } else {
                    '-'
                };
                if c == '-' && value.ends_with('-') {
                    continue;
                }
                value.push(c);
            }
            value.trim_matches('-').to_owned()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if prefix.is_empty() {
        branch
    } else {
        format!("{prefix}/{branch}")
    }
}
fn available(raw: &str, names: &[String]) -> String {
    let name = fragment(raw);
    let base = if name.contains('/') {
        name
    } else {
        format!("feature/{name}")
    };
    let parts = base.split('/').collect::<Vec<_>>();
    let lower = names.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
    let existing: HashSet<_> = lower.iter().map(String::as_str).collect();
    let mut resolved = String::new();
    for (index, part) in parts.iter().enumerate() {
        let base = if resolved.is_empty() {
            part.to_string()
        } else {
            format!("{resolved}/{part}")
        };
        let mut next = base.clone();
        let mut suffix = 2;
        while existing.contains(next.as_str())
            || (index == parts.len() - 1
                && lower.iter().any(|s| s.starts_with(&format!("{next}/"))))
        {
            next = format!("{base}-{suffix}");
            suffix += 1;
        }
        resolved = next;
    }
    resolved
}
async fn cancelled(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    while !*cancel.borrow() {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
pub(crate) struct Naming {
    pub(crate) id: String,
    pub(crate) tid: String,
    pub(crate) root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) old: String,
    pub(crate) message: Value,
    pub(crate) project: String,
}
pub(crate) fn start(
    s: Arc<State>,
    n: Naming,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let generated = tokio::select! {value=crate::source_control::generate_launch_branch(&s,&n.path,&n.project,&n.message)=>value,_=cancelled(&mut cancel)=>None};
        let Some(raw) = generated else {
            return;
        };
        if *cancel.borrow() {
            return;
        }
        if let Err(error) = rename(&s, &n, &raw, &cancel).await {
            eprintln!("Launch branch rename: {error}");
        }
    })
}

async fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = proc::git(root, args, Opts::WRITE)
        .await
        .map_err(|e| format!("Git branch rename failed: {e:?}"))?;
    if out.ok() {
        Ok(out.text().trim().into())
    } else {
        Err(out.stderr)
    }
}
async fn rename(
    s: &State,
    n: &Naming,
    raw: &str,
    cancel: &tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let Naming {
        id,
        tid,
        root,
        path,
        old,
        ..
    } = n;
    let _repo = proc::lock_repo(root).await;
    if *cancel.borrow() {
        return Ok(());
    }
    let settings = crate::source_control::project_settings(s, &n.project);
    let exact = settings["branchNamingMode"] == "custom";
    let preferred = formatted(raw, &settings);
    let names = git(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )
    .await?
    .lines()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let next = if exact {
        preferred
    } else {
        available(&preferred, &names)
    };
    if next == *old {
        return Ok(());
    }
    if next.starts_with('-') {
        return Err("Invalid generated branch name".into());
    }
    git(root, &["check-ref-format", "--branch", &next]).await?;
    {
        let db = s.db.lock().unwrap();
        let t = get(&db, "thread", tid)?;
        if !t["deletedAt"].is_null() || t["worktreePath"] != json!(path) || t["branch"] != *old {
            return Ok(());
        }
        db.execute(
            "UPDATE thread_launches SET rename_to=?1 WHERE id=?2",
            rusqlite::params![next, id],
        )
        .map_err(|e| e.to_string())?;
    }
    // Cancellation waits for this mutation and its ownership record to settle.
    git(path, &["branch", "-m", "--", old, &next]).await?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE thread_launches SET branch=?1,rename_to=NULL WHERE id=?2",
        rusqlite::params![next, id],
    )
    .map_err(|e| e.to_string())?;
    crate::worktree_setup::update(&tx, id, |snapshot| snapshot["branch"] = json!(next))?;
    let mut cursor = 0i64;
    loop {
        use rusqlite::OptionalExtension;
        let row:Option<(i64,String)>=tx.query_row("SELECT rowid,id FROM records WHERE kind='thread' AND rowid>?1 AND json_extract(body,'$.worktreePath')=?2 AND json_extract(body,'$.branch')=?3 AND json_extract(body,'$.deletedAt') IS NULL ORDER BY rowid LIMIT 1",rusqlite::params![cursor,path.to_string_lossy(),old],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        let Some((row, thread)) = row else {
            break;
        };
        cursor = row;
        let mut t = get(&tx, "thread", &thread)?;
        t["branch"] = json!(next);
        t["updatedAt"] = json!(iso(now_ms()));
        put(&tx, "thread", &thread, &thread, &t)?;
        append(&tx, &thread, "thread.metadata-updated", t.clone(), &t)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(())
}
