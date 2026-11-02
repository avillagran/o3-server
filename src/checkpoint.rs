//! Git tree snapshots use a private index; the user's index and branch are untouched.
use crate::{
    events::{append, get, put},
    store::State,
};
use serde_json::json;
use std::{path::Path, process::Command};
fn git(cwd: &Path, index: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_AUTHOR_NAME", "o3 checkpoint")
        .env("GIT_AUTHOR_EMAIL", "checkpoint@localhost")
        .env("GIT_COMMITTER_NAME", "o3 checkpoint")
        .env("GIT_COMMITTER_EMAIL", "checkpoint@localhost")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().into())
}
/// Bind a checkpoint to a physical workspace, refusing symlink traversal.
pub fn workspace_identity(cwd: &Path) -> Result<serde_json::Value, String> {
    use std::os::unix::fs::MetadataExt;
    let canonical = cwd.canonicalize().map_err(|e| e.to_string())?;
    for ancestor in cwd.ancestors() {
        if std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?.file_type().is_symlink() {
            return Err("rollback workspace contains a symlink".into());
        }
    }
    let metadata = std::fs::metadata(&canonical).map_err(|e| e.to_string())?;
    let out = Command::new("git").current_dir(&canonical).args(["rev-parse", "--path-format=absolute", "--git-common-dir", "--absolute-git-dir"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err("rollback repository unavailable".into()); }
    let mut repositories = Vec::new();
    for path in String::from_utf8_lossy(&out.stdout).lines() {
        let path = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
        let m = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        repositories.push(json!({"path":path,"dev":m.dev(),"ino":m.ino()}));
    }
    Ok(json!({"cwd":canonical,"dev":metadata.dev(),"ino":metadata.ino(),"repositories":repositories}))
}
fn sync_tree(path: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == ".git" { continue; }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() { sync_tree(&entry.path())?; }
        else if kind.is_file() { std::fs::File::open(entry.path()).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?; }
    }
    std::fs::File::open(path).and_then(|f| f.sync_all()).map_err(|e| e.to_string())
}
pub fn capture(s: &State, tid: &str, rid: &str, cwd: &Path, baseline: bool) -> Result<(), String> {
    let identity = workspace_identity(cwd)?;
    let index = s
        .dir
        .join(format!("checkpoint-{}.index", crate::store::random_id()?));
    let result = (|| {
        // An empty index also works for unborn repositories.
        git(cwd, &index, &["read-tree", "--empty"])?;
        git(cwd, &index, &["add", "-A", "--", "."])?;
        let tree = git(cwd, &index, &["write-tree"])?;
        let commit = git(cwd, &index, &["commit-tree", &tree, "-m", "o3 checkpoint"])?;
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let t = get(&tx, "thread", tid)?;
        let mut run = get(&tx, "runs", rid)?;
        let scope = format!("scope-{tid}");
        let id = if baseline {
            format!("{rid}-baseline")
        } else {
            format!("{rid}-checkpoint")
        };
        let reference = format!("refs/o3/checkpoints/{}", crate::store::random_id()?);
        git(cwd, &index, &["update-ref", &reference, &commit])?;
        let ordinal = run["ordinal"].as_u64().unwrap_or(1);
        put(
            &tx,
            "checkpointScopes",
            &scope,
            tid,
            &json!({"id":scope,"threadId":tid,"kind":"root_run","cwd":cwd,"identity":identity,"projectId":t["projectId"],"status":"ready"}),
        )?;
        let cp = json!({"id":id,"threadId":tid,"scopeId":scope,"runId":rid,"status":"ready","ref":reference,"appRunOrdinal":if baseline {0}else{ordinal},"ordinalWithinScope":if baseline {0}else{ordinal}});
        put(&tx, "checkpoints", &id, tid, &cp)?;
        append(&tx, tid, "checkpoint.updated", cp, &t)?;
        if !baseline {
            run["checkpointId"] = json!(id);
            put(&tx, "runs", rid, tid, &run)?;
        }
        tx.commit().map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(index);
    result
}
pub fn restore(s: &State, cwd: &Path, reference: &str) -> Result<(), String> {
    if !reference.starts_with("refs/o3/checkpoints/") || !crate::vcs::valid_ref_name(reference) {
        return Err("invalid checkpoint ref".into());
    }
    let index = s
        .dir
        .join(format!("rollback-{}.index", crate::store::random_id()?));
    let result = (|| {
        git(cwd, &index, &["read-tree", "--empty"])?;
        git(cwd, &index, &["add", "-A", "--", "."])?;
        git(cwd, &index, &["read-tree", "--reset", "-u", reference])?;
        sync_tree(cwd)
    })();
    let _ = std::fs::remove_file(index);
    result
}

/// Filesystem intent is synced before restore, independently of the receipt transaction.
pub(crate) fn journal(s: &State, command: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    if s.rollback_pending
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return Err("a rollback requires recovery before another mutation".into());
    }
    let result = (|| {
        let dir = s.dir.join("rollback-journal");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::File::open(&s.dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let temp = dir.join("intent.part");
        let mut f = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        f.write_all(command.to_string().as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(temp, dir.join("intent.json")).map_err(|e| e.to_string())?;
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    })();
    // A failed sync leaves recovery required: do not allow unrelated mutations.
    result
}
pub(crate) fn complete_journal(s: &State) -> Result<(), String> {
    let dir = s.dir.join("rollback-journal");
    match std::fs::remove_file(dir.join("intent.json")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    std::fs::File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    s.rollback_pending
        .store(false, std::sync::atomic::Ordering::Release);
    Ok(())
}
/// Replay before serving requests or resuming providers. An invalid intent fails startup closed.
pub(crate) fn recover_rollbacks(s: &std::sync::Arc<State>) -> Result<(), String> {
    use std::io::Read;
    let dir = s.dir.join("rollback-journal");
    let entries = match std::fs::read_dir(&dir) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let paths = entries
        .take(1025)
        .map(|e| e.map(|v| v.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.len() > 1024 {
        return Err("too many rollback intents".into());
    }
    for path in paths {
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .map_err(|e| e.to_string())?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("rollback intent exceeds recovery budget".into());
        }
        let c: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "invalid rollback intent")?;
        if c["type"] != "checkpoint.rollback" {
            return Err("invalid rollback intent command".into());
        }
        let db = s.db.lock().unwrap();
        let scope = get(&db, "checkpointScopes", c["scopeId"].as_str().ok_or("rollback scope missing")?)?;
        if c["rollbackIdentity"].is_null() || c["rollbackIdentity"] != scope["identity"] {
            return Err("rollback intent identity missing or changed; manual recovery required".into());
        }
        let cwd = Path::new(scope["cwd"].as_str().ok_or("rollback cwd missing")?);
        if workspace_identity(cwd)? != c["rollbackIdentity"] {
            return Err("rollback workspace identity changed; refusing replay".into());
        }
        drop(db);
        crate::thread_commands::dispatch(s, &c)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        std::fs::File::open(&dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    s.rollback_pending
        .store(false, std::sync::atomic::Ordering::Release);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rollback_linked_worktree_leaves_root_and_both_indexes_untouched() {
        let dir =
            std::env::temp_dir().join(format!("o3-linked-{}", crate::store::random_id().unwrap()));
        let root = dir.join("root");
        let linked = dir.join("linked");
        std::fs::create_dir_all(&root).unwrap();
        let index = dir.join("seed-index");
        git(&root, &index, &["init", "-q"]).unwrap();
        std::fs::write(root.join("file"), "base").unwrap();
        git(&root, &index, &["add", "."]).unwrap();
        git(&root, &index, &["commit", "-qm", "base"]).unwrap();
        assert!(Command::new("git")
            .current_dir(&root)
            .env_remove("GIT_INDEX_FILE")
            .args([
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD"
            ])
            .output()
            .unwrap()
            .status
            .success());
        let gitdir = git(&linked, &index, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let linked_index = std::path::PathBuf::from(gitdir).join("index");
        let before = std::fs::read(&linked_index).unwrap();
        let state = State::open(&dir.join("state")).unwrap();
        {
            let db = state.db.lock().unwrap();
            put(&db, "thread", "t", "t", &json!({"id":"t"})).unwrap();
            put(&db, "runs", "r", "t", &json!({"id":"r","ordinal":1})).unwrap();
        }
        std::fs::write(linked.join("file"), "dirty baseline").unwrap();
        capture(&state, "t", "r", &linked, true).unwrap();
        std::fs::write(linked.join("file"), "after").unwrap();
        std::fs::write(linked.join("new"), "new").unwrap();
        let reference = get(&state.db.lock().unwrap(), "checkpoints", "r-baseline").unwrap()["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        restore(&state, &linked, &reference).unwrap();
        assert_eq!(
            std::fs::read_to_string(linked.join("file")).unwrap(),
            "dirty baseline"
        );
        assert!(!linked.join("new").exists());
        assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "base");
        assert_eq!(std::fs::read(linked_index).unwrap(), before);
        assert!(!root.join(".git/index").exists());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn captures_dirty_untracked_deleted_files_and_restores_without_changing_index() {
        let dir = std::env::temp_dir().join(format!(
            "o3-checkpoint-{}",
            crate::store::random_id().unwrap()
        ));
        let root = dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let index = dir.join("initial-index");
        git(&root, &index, &["init", "-q"]).unwrap();
        std::fs::write(root.join("tracked"), "before").unwrap();
        git(&root, &index, &["add", "."]).unwrap();
        git(&root, &index, &["commit", "-qm", "base"]).unwrap();
        let state = State::open(&dir.join("state")).unwrap();
        {
            let db = state.db.lock().unwrap();
            put(&db, "thread", "t", "t", &json!({"id":"t"})).unwrap();
            put(
                &db,
                "runs",
                "r",
                "t",
                &json!({"id":"r","ordinal":1,"status":"running"}),
            )
            .unwrap();
        }
        capture(&state, "t", "r", &root, true).unwrap();
        std::fs::remove_file(root.join("tracked")).unwrap();
        std::fs::write(root.join("new"), "after").unwrap();
        capture(&state, "t", "r", &root, false).unwrap();
        let reference = {
            let db = state.db.lock().unwrap();
            get(&db, "checkpoints", "r-baseline").unwrap()["ref"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        restore(&state, &root, &reference).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("tracked")).unwrap(),
            "before"
        );
        assert!(!root.join("new").exists());
        assert!(!root.join(".git/index").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
