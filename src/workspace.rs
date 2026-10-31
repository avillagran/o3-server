//! Workspace preparation uses argument vectors; no shell or user's active checkout is changed.
use crate::{events, store::State};
use serde_json::Value;
use std::{path::PathBuf, process::Command};
pub fn prepare(s: &State, p: &Value, tid: &str) -> Result<(Value, Value), String> {
    let strategy = &p["workspaceStrategy"];
    let branch = strategy.get("branch").cloned().unwrap_or(Value::Null);
    match strategy["type"].as_str() {
        Some("root") => Ok((Value::Null, branch)),
        Some("existing_worktree") => {
            let path = strategy["worktreePath"]
                .as_str()
                .ok_or("worktreePath required")?;
            let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
            git(&path, &["rev-parse", "--show-toplevel"])?;
            Ok((serde_json::json!(path), branch))
        }
        Some("worktree") => {
            let project = events::get(
                &s.db.lock().unwrap(),
                "project",
                p["projectId"].as_str().ok_or("projectId required")?,
            )?;
            let root = PathBuf::from(
                project["workspaceRoot"]
                    .as_str()
                    .ok_or("workspace root missing")?,
            );
            let base = strategy["baseRef"]
                .as_str()
                .filter(|v| !v.starts_with('-'))
                .ok_or("baseRef required")?;
            let settings = crate::settings::read(s)?;
            let directory = settings["worktreesDirectory"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or(s.dir.join("worktrees"));
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            // Encode arbitrary thread IDs, preventing traversal and collisions.
            let name: String = tid.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
            let path = directory.join(name);
            let checkout = path.to_str().ok_or("invalid worktree path")?;
            if path.exists() {
                git(&path, &["rev-parse", "--show-toplevel"])?;
                return Ok((serde_json::json!(path), branch));
            }
            let base = if strategy["startFromOrigin"] == true {
                format!("origin/{}", base.trim_start_matches("origin/"))
            } else {
                base.to_owned()
            };
            if let Some(branch) = branch.as_str() {
                git(
                    &root,
                    &["worktree", "add", "-b", branch, "--", checkout, &base],
                )?;
            } else {
                git(
                    &root,
                    &["worktree", "add", "--detach", "--", checkout, &base],
                )?;
            }
            Ok((serde_json::json!(path), branch))
        }
        _ => Err("invalid workspace strategy".into()),
    }
}
fn git(root: &std::path::Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}
