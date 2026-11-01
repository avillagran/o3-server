//! Workspace preparation uses argument vectors; no shell or user's active checkout is changed.
use crate::{events, store::State};
use serde_json::Value;
use std::{path::PathBuf, process::Command};
pub struct Prepared {
    pub path: Value,
    pub branch: Value,
    owned: Option<(PathBuf, PathBuf, Option<String>)>,
}
impl Prepared {
    pub fn is_owned(&self) -> bool {
        self.owned.is_some()
    }
    pub fn commit(&mut self) {
        self.owned = None;
    }
    pub fn rollback(&mut self) -> Result<(), String> {
        if let Some((root, path, branch)) = self.owned.take() {
            if path.exists() {
                git(
                    &root,
                    &[
                        "worktree",
                        "remove",
                        "--force",
                        "--",
                        path.to_str().ok_or("invalid path")?,
                    ],
                )?;
            }
            if let Some(branch) = branch {
                // Only a branch absent before this preparation can be owned here.
                if git(
                    &root,
                    &["show-ref", "--verify", &format!("refs/heads/{branch}")],
                )
                .is_ok()
                {
                    git(&root, &["branch", "-D", "--", &branch])?;
                }
            }
        }
        Ok(())
    }
}
pub fn prepare(s: &State, p: &Value, tid: &str) -> Result<Prepared, String> {
    let strategy = &p["workspaceStrategy"];
    let branch = strategy.get("branch").cloned().unwrap_or(Value::Null);
    match strategy["type"].as_str() {
        Some("root") => Ok(Prepared {
            path: Value::Null,
            branch,
            owned: None,
        }),
        Some("existing_worktree") => {
            let path = strategy["worktreePath"]
                .as_str()
                .ok_or("worktreePath required")?;
            let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
            git(&path, &["rev-parse", "--show-toplevel"])?;
            Ok(Prepared {
                path: serde_json::json!(path),
                branch,
                owned: None,
            })
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
                return Ok(Prepared {
                    path: serde_json::json!(path),
                    branch,
                    owned: None,
                });
            }
            let base = if strategy["startFromOrigin"] == true {
                format!("origin/{}", base.trim_start_matches("origin/"))
            } else {
                base.to_owned()
            };
            let owned_branch = branch
                .as_str()
                .filter(|b| {
                    git(&root, &["show-ref", "--verify", &format!("refs/heads/{b}")]).is_err()
                })
                .map(str::to_owned);
            let mut prepared = Prepared {
                path: serde_json::json!(path),
                branch: branch.clone(),
                owned: Some((root.clone(), path.clone(), owned_branch)),
            };
            let result = if let Some(branch) = branch.as_str() {
                git(
                    &root,
                    &["worktree", "add", "-b", branch, "--", checkout, &base],
                )
            } else {
                git(
                    &root,
                    &["worktree", "add", "--detach", "--", checkout, &base],
                )
            };
            if let Err(error) = result {
                return match prepared.rollback() {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!("{error}; worktree cleanup failed: {cleanup}")),
                };
            }
            if path.join(".gitmodules").is_file() {
                let effective = crate::source_control::effective(s, &root);
                let project_file = std::fs::read(path.join("t3.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                let mode = effective["worktreeSubmodules"]
                    .as_str()
                    .or_else(|| {
                        project_file
                            .as_ref()
                            .and_then(|v| v["worktreeSubmodules"].as_str())
                    })
                    .unwrap_or("recursive");
                let args = match mode {
                    "none" => None,
                    "top-level" => Some(vec!["submodule", "update", "--init"]),
                    _ => Some(vec!["submodule", "update", "--init", "--recursive"]),
                };
                if let Some(args) = args {
                    if let Err(error) = git(&path, &args) {
                        return match prepared.rollback() {
                            Ok(()) => Err(error),
                            Err(cleanup) => Err(format!("{error}; {cleanup}")),
                        };
                    }
                }
            }
            Ok(prepared)
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
