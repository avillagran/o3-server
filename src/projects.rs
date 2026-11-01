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

/// Resolve repository metadata without credentials or network I/O.
pub async fn repository_identity(root: &str) -> Value {
    let cwd = std::path::Path::new(root);
    let read = |args: Vec<&'static str>| async move {
        crate::proc::git(cwd, &args, crate::proc::Opts::READ)
            .await
            .ok()
            .filter(|o| o.ok())
            .map(|o| o.text().trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let Some(path) = read(vec!["rev-parse", "--show-toplevel"]).await else {
        return Value::Null;
    };
    let mut selected = None;
    for remote in ["upstream", "origin"] {
        if let Some(url) = read(vec!["remote", "get-url", remote]).await {
            selected = Some((remote, url));
            break;
        }
    }
    let Some((remote, url)) = selected else {
        return Value::Null;
    };
    let normalized = if let Ok(u) = url::Url::parse(&url) {
        u.host_str()
            .map(|host| format!("{}/{}", host, u.path().trim_matches('/')))
    } else {
        url.split_once(':').map(|(host, path)| {
            format!(
                "{}/{}",
                host.rsplit('@').next().unwrap_or(host),
                path.trim_matches('/')
            )
        })
    };
    let key = normalized
        .unwrap_or_else(|| url.clone())
        .trim_end_matches(".git")
        .to_lowercase();
    let mut identity = json!({"canonicalKey":key,"locator":{"source":"git-remote","remoteName":remote,"remoteUrl":url},"rootPath":path});
    if let Some((host, repo)) = key.split_once('/') {
        identity["displayName"] = json!(repo);
        identity["owner"] = json!(repo.split('/').next());
        identity["name"] = json!(repo.rsplit('/').next());
        if let Some(provider) = match host {
            "github.com" => Some("github"),
            "gitlab.com" => Some("gitlab"),
            _ => None,
        } {
            identity["provider"] = json!(provider);
        }
    }
    identity
}

pub async fn mutate_resolved(s: &State, c: &Value) -> Result<Value> {
    let mut c = c.clone();
    let root = c["workspaceRoot"].as_str().map(str::to_owned).or_else(|| {
        let id = c["projectId"].as_str()?;
        let p = events::get(&s.db.lock().unwrap(), "project", id).ok()?;
        p["workspaceRoot"].as_str().map(str::to_owned)
    });
    if let Some(root) = root {
        c["repositoryIdentity"] = repository_identity(&root).await;
    }
    mutate(s, &c)
}

/// One replacement snapshot publishes the resolved roots and current shell together.
pub async fn refresh_identities(s: &State) -> Result<()> {
    refresh_identities_if_changed(s, true).await
}
pub(crate) async fn refresh_identities_if_changed(s: &State, force: bool) -> Result<()> {
    let projects = live_projects(s)?;
    let mut resolved = Vec::new();
    for p in projects {
        if let Some(root) = p["workspaceRoot"].as_str() {
            resolved.push((
                p["id"].as_str().unwrap_or("").to_owned(),
                root.to_owned(),
                repository_identity(root).await,
                p["repositoryIdentity"].clone(),
            ));
        }
    }
    if resolved.is_empty() {
        return Ok(());
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut changed = force;
    for (id, root, identity, previous) in &resolved {
        let mut p = events::get(&tx, "project", id)?;
        if p["workspaceRoot"] == root.as_str()
            && p["repositoryIdentity"] != *identity
            && p["repositoryIdentity"] == *previous
        {
            p["repositoryIdentity"] = identity.clone();
            events::put(&tx, "project", id, "", &p)?;
            changed = true;
        }
    }
    if changed {
        tx.execute("INSERT INTO events(thread_id,body) VALUES('', '{}')", [])
            .map_err(|e| e.to_string())?;
        let seq = tx.last_insert_rowid();
        let item = json!({"kind":"repository-identities-resolved","resolvedRepositoryIdentityRoots":resolved.iter().map(|(_,root,_,_)|root).collect::<Vec<_>>()});
        tx.execute(
            "UPDATE events SET shell=? WHERE sequence=?",
            rusqlite::params![item.to_string(), seq],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    if changed {
        s.notify();
    }
    Ok(())
}

/// One server-owned poller shared by every TCP and embedded bridge connection.
pub(crate) fn ensure_identity_watcher(state: &std::sync::Arc<State>) {
    if state
        .identities_started
        .swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }
    let weak = std::sync::Arc::downgrade(state);
    tokio::spawn(async move {
        let mut previous_roots = Vec::new();
        loop {
            let Some(state) = weak.upgrade() else { break };
            let projects = crate::projects::snapshot(&state).unwrap_or(Value::Null);
            let roots = projects["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["workspaceRoot"].as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            drop(projects);
            let force = roots != previous_roots;
            let _ = crate::projects::refresh_identities_if_changed(&state, force).await;
            previous_roots = roots;
            drop(state);
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}
