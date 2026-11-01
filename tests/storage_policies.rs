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
    let s = support::start(|_| {});
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
    for id in ["clean", "dirty", "pinned", "shared", "busy", "deleted"] {
        c.rpc("orchestration.launchThread",json!({"commandId":id,"threadId":id,"projectId":"p","title":id,"modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}})).unwrap();
    }
    let path = |id: &str| {
        t3_protocol::api::orchestration::thread_projection(&c, id).unwrap()["thread"]
            ["worktreePath"]
            .as_str()
            .unwrap()
            .to_owned()
    };
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
    let report = c.rpc("server.runStorageCleanup", json!({})).unwrap();
    assert_eq!(report["counts"]["removed"], 4); // clean, deleted, log, browser artifact
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
    for id in ["dirty", "pinned", "shared", "busy"] {
        assert!(Path::new(&path(id)).is_dir(), "{id}");
    }
    let saved = c.rpc("server.getSettings", json!({})).unwrap();
    assert_eq!(
        t3_protocol::storage_settings::resolved_rules(&saved).unwrap()["worktreeUnchanged"],
        true
    );
}
