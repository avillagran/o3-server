//! ManagedProjectFolders' Scratch launch normalization and atomic folder claim.
use crate::{
    events,
    store::{iso, now_ms, State},
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub(crate) struct Claim(Option<PathBuf>);
impl Claim {
    pub(crate) fn commit(&mut self) {
        self.0 = None;
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_dir(path);
        }
    }
}
pub(crate) fn normalize(s: &State, p: &Value) -> Result<(Value, Claim), String> {
    let unchanged = || Ok((p.clone(), Claim(None)));
    if !matches!(
        p["workspaceStrategy"]["type"].as_str(),
        Some("root" | "worktree")
    ) {
        return unchanged();
    }
    let pid = p["projectId"].as_str().ok_or("projectId required")?;
    let project = events::get(&s.reader.lock().unwrap(), "project", pid)?;
    let root = Path::new(
        project["workspaceRoot"]
            .as_str()
            .ok_or("Workspace root missing")?,
    );
    let scratch = s.asset_profile_root.join("scratch");
    let matching = root == scratch
        || root
            .canonicalize()
            .ok()
            .zip(scratch.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
    if !matching {
        return unchanged();
    }
    let probe = Command::new("git")
        .arg("-C")
        .arg(&s.asset_profile_root)
        .args(crate::proc::SAFE_CONFIG)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match probe {
        Ok(status) if status.success() => return unchanged(),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return unchanged(),
        _ => {}
    }
    let cid = p["commandId"].as_str().ok_or("commandId required")?;
    let receipt: Option<i64> = s
        .reader
        .lock()
        .unwrap()
        .query_row("SELECT 1 FROM receipts WHERE command_id=?", [cid], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    if receipt.is_some() {
        let mut request = p.clone();
        if request["workspaceStrategy"]["type"] == "worktree" {
            request["workspaceStrategy"] = json!({"type":"root"});
        }
        return Ok((request, Claim(None)));
    }
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let text = p["initialMessage"]["text"]
        .as_str()
        .or_else(|| p["title"].as_str())
        .unwrap_or("");
    let words = text
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(48)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned();
    let tid = p["threadId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(format!("thread-{cid}"));
    let id = tid
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    let date = &iso(now_ms())[..10];
    for attempt in 0..32 {
        let token = if attempt == 0 {
            id[id.len().saturating_sub(8)..].to_owned()
        } else if attempt == 1 {
            id.clone()
        } else {
            format!("{id}-{}", &crate::store::random_id()?[..8])
        };
        let name = [date, words.as_str(), token.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let path = scratch.join(name);
        match std::fs::create_dir(&path) {
            Ok(()) => {
                let mut request = p.clone();
                request["workspaceStrategy"] =
                    json!({"type":"existing_worktree","worktreePath":path});
                return Ok((request, Claim(Some(path))));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Unable to claim Scratch folder: {e}")),
        }
    }
    Err("Unable to claim a free Scratch folder".into())
}
