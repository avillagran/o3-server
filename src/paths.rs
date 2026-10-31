//! Path sandboxing as T3 does it: workspace-relative paths stay inside their root (lexically and
//! after symlink resolution); roots a client may name are the registered projects, the server's
//! working directory and managed worktrees.
use crate::store::State;
use std::path::{Component, Path, PathBuf};

pub fn expand_home(p: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (p, home) {
        ("~", Some(h)) => h,
        (p, Some(h)) if p.starts_with("~/") => h.join(&p[2..]),
        _ => PathBuf::from(p),
    }
}

/// Collapses `.` and `..` without touching the filesystem.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

pub fn within(root: &Path, candidate: &Path) -> bool {
    candidate.strip_prefix(root).is_ok()
}

/// `relative` joined to `root`, lexically proven to stay strictly inside it. Absolute paths,
/// `.` and anything that escapes with `..` are refused.
pub fn resolve_relative(root: &Path, relative: &str) -> Option<(PathBuf, String)> {
    let rel = relative.trim();
    if rel.is_empty() || rel.contains('\0') || Path::new(rel).is_absolute() {
        return None;
    }
    let root = normalize(root);
    let abs = normalize(&root.join(rel));
    let inside = abs.strip_prefix(&root).ok()?;
    let inside = inside.to_str()?.to_owned();
    if inside.is_empty() {
        return None;
    }
    Some((abs, inside))
}

/// Real path of `p`, or of its nearest existing ancestor with the missing tail re-appended.
pub fn real_or_ancestor(p: &Path) -> std::io::Result<(PathBuf, bool)> {
    let mut cur = p.to_path_buf();
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(real) => {
                let tail = p.strip_prefix(&cur).unwrap_or(Path::new(""));
                // `join("")` would add a trailing separator and turn a file into a directory path.
                return Ok((
                    if tail.as_os_str().is_empty() {
                        real
                    } else {
                        real.join(tail)
                    },
                    cur == p,
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A dangling link is not absence.
                if std::fs::symlink_metadata(&cur).is_ok() || !cur.pop() {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn real(p: &str) -> Option<PathBuf> {
    std::fs::canonicalize(expand_home(p)).ok()
}

/// Symlink-resolved roots the server may be asked to look into.
pub fn known_roots(s: &State) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(std::fs::canonicalize(cwd).ok());
    }
    let managed = s
        .dir
        .parent()
        .map(|p| p.join("worktrees"))
        .and_then(|p| std::fs::canonicalize(p).ok());
    roots.extend(managed.clone());
    let db = s.db.lock().unwrap();
    if let Ok(mut q) = db.prepare("SELECT body FROM records WHERE kind IN ('project','thread')") {
        if let Ok(rows) = q.query_map([], |r| r.get::<_, String>(0)) {
            for body in rows.flatten() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                    continue;
                };
                if !v["deletedAt"].is_null() {
                    continue;
                }
                if let Some(root) = v["workspaceRoot"].as_str() {
                    roots.extend(real(root));
                }
                // Thread worktrees count only beneath a managed worktree folder.
                if let (Some(w), Some(m)) = (v["worktreePath"].as_str(), managed.as_ref()) {
                    if let Some(w) = real(w).filter(|w| within(m, w)) {
                        roots.push(w);
                    }
                }
            }
        }
    }
    roots.retain(|r| r.parent().is_some());
    roots
}

pub fn is_known_workspace(s: &State, cwd: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(cwd) else {
        return false;
    };
    known_roots(s).iter().any(|r| within(r, &real))
}
