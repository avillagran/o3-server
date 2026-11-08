#[path = "support/worktree_removal.rs"]
pub mod retained_worktrees;
mod support;
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};
#[test]
fn cleanup_does_not_hold_database_while_git_is_slow() {
    let s = start_recording_cleanup(|_| {});
    let root = s.dir.join("repo");
    let tree = s.dir.join("tree");
    let bin = s.dir.join("bin");
    for p in [&root, &tree, &bin] {
        std::fs::create_dir_all(p).unwrap();
    }
    let marker = s.dir.join("git-started");
    let release = s.dir.join("git-release");
    struct Release(std::path::PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            // Release the exact owned command on assertion failure as well.
            std::fs::write(&self.0, "release").unwrap();
        }
    }
    let release_guard = Release(release.clone());
    let script = format!(
        "#!/bin/sh\ntouch '{}'\nwhile ! test -f '{}'; do sleep 0.01; done\nexit 1\n",
        marker.display(),
        release.display()
    );
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
        support::wait_for(Duration::from_secs(1), || s.state.db.try_lock().is_ok()),
        "git held global database mutex"
    );
    assert!(
        !worker.is_finished(),
        "owned Git must remain blocked during both probes"
    );
    assert!(!release.exists());
    let c = s.connect();
    let before = Instant::now();
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap();
    assert!(before.elapsed() < Duration::from_millis(500));
    assert!(
        !worker.is_finished(),
        "RPC completed while owned Git was still blocked"
    );
    assert!(!release.exists());
    drop(release_guard);
    worker.join().unwrap();
    assert!(tree.exists());
}

fn start_recording_cleanup(
    configure: impl FnOnce(&std::sync::Arc<o3_server::store::State>),
) -> support::Server {
    support::start(|state| {
        retained_worktrees::RetainingExecutor::install(state);
        configure(state);
    })
}
