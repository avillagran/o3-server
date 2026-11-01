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
pub fn capture(s: &State, tid: &str, rid: &str, cwd: &Path, baseline: bool) -> Result<(), String> {
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
            &json!({"id":scope,"threadId":tid,"kind":"root_run","cwd":cwd,"status":"ready"}),
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
        Ok(())
    })();
    let _ = std::fs::remove_file(index);
    result
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
        git(
            &root,
            &index,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        )
        .unwrap();
        let gitdir = git(&linked, &index, &["rev-parse", "--absolute-git-dir"]).unwrap();
        let linked_index = std::path::PathBuf::from(gitdir).join("index");
        let before = std::fs::read(&linked_index).ok();
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
        assert_eq!(std::fs::read(linked_index).ok(), before);
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
