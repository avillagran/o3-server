#[path = "support/worktree_removal.rs"]
mod retained_worktrees;
mod support;
use serde_json::json;
use std::{path::Path, process::Command};
fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
        .status
        .success());
}
#[test]
fn real_storage_writers_drive_collection_and_preserve_unsafe_worktrees() {
    let recorder = std::sync::Arc::new(std::sync::Mutex::new(None));
    let configure = recorder.clone();
    let s = support::start(move |state| {
        *configure.lock().unwrap() = Some(retained_worktrees::RetainingExecutor::install(state));
    });
    let removal = recorder.lock().unwrap().clone().unwrap();
    let c = s.connect();
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init"]);
    git(&root, &["config", "user.name", "Storage Test"]);
    git(&root, &["config", "user.email", "storage@example.invalid"]);
    std::fs::write(root.join("a"), "a").unwrap();
    git(&root, &["add", "a"]);
    git(&root, &["commit", "-m", "initial"]);
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Storage","workspaceRoot":root})).unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    for key in ["storageCleanup", "projectWorktreeCleanup"] {
        assert_eq!(
            t3_protocol::storage_settings::capabilities(&config)[key],
            true
        );
    }
    for id in [
        "clean",
        "dirty",
        "pinned",
        "shared",
        "busy",
        "deleted",
        "untracked",
        "locked",
        "reused",
    ] {
        c.rpc("orchestration.launchThread",json!({"commandId":id,"threadId":id,"projectId":"p","title":id,"modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}})).unwrap();
    }
    assert!(support::wait_for(
        std::time::Duration::from_secs(10),
        || {
            s.state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM thread_launches WHERE completed=1",
                    [],
                    |r| r.get::<_, usize>(0),
                )
                .unwrap()
                == 9
        }
    ));
    let path = |id: &str| {
        assert!(support::wait_for(
            std::time::Duration::from_secs(10),
            || {
                let p = t3_protocol::api::orchestration::thread_projection(&c, id).unwrap();
                p["thread"]["worktreePath"].is_string()
            }
        ));
        t3_protocol::api::orchestration::thread_projection(&c, id).unwrap()["thread"]
            ["worktreePath"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    std::fs::write(
        Path::new(&path("untracked")).join("note"),
        "keep unless configured",
    )
    .unwrap();
    git(&root, &["worktree", "lock", &path("locked")]);
    let external = s.dir.join("external");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            external.to_str().unwrap(),
            "HEAD",
        ],
    );
    c.rpc("orchestration.launchThread",json!({"commandId":"reuse-external","threadId":"reused","reuseExistingThread":true,"projectId":"p","title":"Clean","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"existing_worktree","worktreePath":external}})).unwrap();
    assert!(support::wait_for(
        std::time::Duration::from_secs(10),
        || path("reused") == external.to_str().unwrap()
    ));
    assert!(!s.state.db.lock().unwrap().query_row("SELECT json_extract(body,'$.ownedWorktree') FROM records WHERE kind='thread' AND id='reused'",[],|r|r.get::<_,bool>(0)).unwrap());
    // Reuse must not transfer ownership to an existing external worktree.
    let dirty = path("dirty");
    std::fs::write(Path::new(&dirty).join("a"), "changed").unwrap();
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.pin","commandId":"pin","threadId":"pinned"}),
    )
    .unwrap();
    let shared = path("shared");
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"other","threadId":"other","projectId":"p","title":"Other","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","worktreePath":shared})).unwrap();
    {
        let db = s.state.db.lock().unwrap();
        db.execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES('runs','busy-run','busy',?)",
            [json!({"id":"busy-run","status":"running"}).to_string()],
        )
        .unwrap();
    }
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"delete","threadId":"deleted"}),
    )
    .unwrap();
    for (key, value) in [
        ("storageCleanup.worktreeUnchanged", json!(true)),
        ("storageCleanup.logsAfterDays", json!(1)),
        ("storageCleanup.browserArtifactsAfterDays", json!(1)),
    ] {
        let patch =
            t3_protocol::storage_settings::patch(&json!({"key":key,"value":value})).unwrap();
        c.rpc("server.updateSettings", patch).unwrap();
    }
    for kind in ["logs", "browser-artifacts"] {
        std::fs::create_dir_all(s.dir.join(kind)).unwrap();
        let file = s.dir.join(kind).join("old");
        std::fs::write(&file, "old").unwrap();
        std::fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400))
            .unwrap();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let guard = runtime.block_on(o3_server::proc::lock_repo(&root));
    let report = c.rpc("server.runStorageCleanup", json!({})).unwrap();
    assert_eq!(
        report["counts"]["removed"], 2,
        "repository mutation lock retains worktrees"
    );
    assert!(Path::new(&path("clean")).is_dir());
    drop(guard);
    let clean_path = std::path::PathBuf::from(path("clean"));
    let deleted_path = std::path::PathBuf::from(path("deleted"));
    let report = c.rpc("server.runStorageCleanup", json!({})).unwrap();
    removal.assert_retained_worktree(&clean_path);
    removal.assert_retained_worktree(&deleted_path);
    assert_eq!(report["counts"]["removed"], 2); // clean and deleted; log/browser files were collected in the first pass
    for id in ["clean", "deleted"] {
        let p = t3_protocol::api::orchestration::thread_projection(&c, id).unwrap();
        assert!(p["thread"]["worktreePath"].is_null());
        assert!(t3_protocol::timeline::view(
            &p,
            &json!({}),
            &t3_protocol::timeline::Render::neutral(false)
        )
        .unwrap()
        .is_object());
    }
    for id in [
        "dirty",
        "pinned",
        "shared",
        "busy",
        "untracked",
        "locked",
        "reused",
    ] {
        assert!(Path::new(&path(id)).is_dir(), "{id}");
    }
    let untracked_path = std::path::PathBuf::from(path("untracked"));
    let patch = t3_protocol::storage_settings::patch(
        &json!({"key":"storageCleanup.worktreeKeepWhen","value":"tracked-changes"}),
    )
    .unwrap();
    c.rpc("server.updateSettings", patch).unwrap();
    let report = c.rpc("server.runStorageCleanup", json!({})).unwrap();
    assert_eq!(report["counts"]["removed"], 1);
    removal.assert_retained_worktree(&untracked_path);
    let retained = removal.retained.lock().unwrap();
    let record = retained
        .iter()
        .find(|record| record.original == untracked_path && record.kind == "checkout")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(record.path.join("note")).unwrap(),
        "keep unless configured"
    );
    drop(retained);
    assert!(
        t3_protocol::api::orchestration::thread_projection(&c, "untracked").unwrap()["thread"]
            ["worktreePath"]
            .is_null()
    );
    assert!(Path::new(&path("locked")).is_dir());
    let saved = c.rpc("server.getSettings", json!({})).unwrap();
    assert_eq!(
        t3_protocol::storage_settings::resolved_rules(&saved).unwrap()["worktreeUnchanged"],
        true
    );
}
