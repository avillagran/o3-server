//! Project snapshot and validated mutations. Persistence, receipts and shell events live in
//! `events::mutate_project`; this adds T3's workspace-root rules in front of it.
use crate::{
    events, paths,
    store::{iso, now_ms, State},
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::PathBuf;

type Result<T> = std::result::Result<T, String>;

fn live_projects(s: &State) -> Result<Vec<Value>> {
    let db = s.db.lock().unwrap();
    let mut q = db
        .prepare("SELECT body FROM records WHERE kind='project' ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for body in rows {
        let v: Value =
            serde_json::from_str(&body.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if v["deletedAt"].is_null() {
            out.push(v);
        }
    }
    Ok(out)
}

/// `GET /api/projects`: every live project and the newest update time.
pub fn snapshot(s: &State) -> Result<Value> {
    let projects = live_projects(s)?;
    let updated = projects
        .iter()
        .filter_map(|p| p["updatedAt"].as_str())
        .max()
        .map(str::to_owned)
        .unwrap_or_else(|| iso(now_ms()));
    Ok(json!({"projects":projects,"updatedAt":updated}))
}

fn normalize_root(raw: &str, create: bool) -> Result<PathBuf> {
    let root = std::path::absolute(paths::normalize(&paths::expand_home(raw.trim())))
        .map_err(|e| e.to_string())?;
    match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() => Ok(root),
        Ok(_) => Err(format!(
            "Workspace root is not a directory: {}",
            root.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            std::fs::create_dir_all(&root)
                .map_err(|e| format!("Failed to create workspace root: {}: {e}", root.display()))?;
            Ok(root)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("Workspace root does not exist: {}", root.display()))
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn mutate(s: &State, c: &Value) -> Result<Value> {
    let cid = c["commandId"].as_str().ok_or("commandId required")?;
    let replay =
        s.db.lock()
            .unwrap()
            .query_row("SELECT 1 FROM receipts WHERE command_id=?", [cid], |_| {
                Ok(())
            })
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
    if replay {
        return events::mutate_project(s, c);
    }
    let mut c = c.clone();
    let id = c["projectId"]
        .as_str()
        .ok_or("projectId required")?
        .to_owned();
    match c["type"].as_str() {
        Some(kind @ ("project.create" | "project.update")) if c["workspaceRoot"].is_string() => {
            let root = normalize_root(
                c["workspaceRoot"].as_str().unwrap_or(""),
                kind == "project.create" && c["createWorkspaceRootIfMissing"] == true,
            )?;
            let root = root.to_string_lossy().into_owned();
            if live_projects(s)?
                .iter()
                .any(|p| p["id"] != id.as_str() && p["workspaceRoot"] == root.as_str())
            {
                return Err(format!("A project already uses workspace root {root}"));
            }
            c["workspaceRoot"] = json!(root);
        }
        Some("project.delete") if c["force"] != true => {
            let db = s.db.lock().unwrap();
            let mut q = db
                .prepare("SELECT body FROM records WHERE kind='thread'")
                .map_err(|e| e.to_string())?;
            let rows = q
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for body in rows.flatten() {
                let t: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                if t["projectId"] == id.as_str() && t["deletedAt"].is_null() {
                    return Err(format!(
                        "Project {id} still has threads; delete them or pass force"
                    ));
                }
            }
        }
        _ => {}
    }
    events::mutate_project(s, &c)
}
