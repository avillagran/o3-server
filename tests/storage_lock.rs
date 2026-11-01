mod support;
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};
#[test]
fn cleanup_does_not_hold_database_while_git_is_slow() {
    let s = support::start(|_| {});
    let root = s.dir.join("repo");
    let tree = s.dir.join("tree");
    let bin = s.dir.join("bin");
    for p in [&root, &tree, &bin] {
        std::fs::create_dir_all(p).unwrap();
    }
    let marker = s.dir.join("git-started");
    let script = format!("#!/bin/sh\ntouch '{}'\nsleep 1\nexit 1\n", marker.display());
    std::fs::write(bin.join("git"), script).unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    {
        let db = s.state.db.lock().unwrap();
        for (kind, id, tid, value) in [
            ("project", "p", "", json!({"id":"p","workspaceRoot":root})),
            (
                "thread",
                "t",
                "t",
                json!({"id":"t","projectId":"p","ownedWorktree":true,"worktreePath":tree}),
            ),
        ] {
            db.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES(?,?,?,?)",
                rusqlite::params![kind, id, tid, value.to_string()],
            )
            .unwrap();
        }
    }
    let c = s.connect();
    let worker = std::thread::spawn(move || c.rpc("server.runStorageCleanup", json!({})).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        s.state.db.try_lock().is_ok(),
        "git held global database mutex"
    );
    let c = s.connect();
    let before = Instant::now();
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap();
    assert!(before.elapsed() < Duration::from_millis(500));
    worker.join().unwrap();
    assert!(tree.exists());
}
