mod support;
use serde_json::json;
use std::process::Command;
use t3_protocol::api::orchestration;
#[test]
fn startup_replays_restore_interrupted_before_receipt_commit() {
    for boundary in 0..4 {
        let s = support::start(|s| {
            let root = s.dir.join("repo");
            std::fs::create_dir_all(&root).unwrap();
            assert!(Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success());
            std::fs::write(root.join("file"), "baseline").unwrap();
            {
                let db = s.db.lock().unwrap();
                for (kind, id, v) in [
                    (
                        "thread",
                        "t",
                        json!({"id":"t","projectId":"p","title":"Rollback","deletedAt":null,"archivedAt":null,"modelSelection":{"instanceId":"scripted","model":"m"}}),
                    ),
                    (
                        "runs",
                        "r",
                        json!({"id":"r","threadId":"t","ordinal":1,"status":"completed"}),
                    ),
                ] {
                    db.execute(
                        "INSERT INTO records VALUES(?,?,?,?)",
                        rusqlite::params![kind, id, "t", v.to_string()],
                    )
                    .unwrap();
                }
            }
            o3_server::checkpoint::capture(s, "t", "r", &root, true).unwrap();
            let synchronous: i64 =
                s.db.lock()
                    .unwrap()
                    .query_row("PRAGMA synchronous", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(synchronous, 2, "receipt commits must use FULL durability");
            // Inject the crash window: durable intent, partially restored filesystem,
            // and the old SQLite state (the receipt transaction was rolled back).
            std::fs::write(root.join("file"), "partial restore").unwrap();
            std::fs::write(root.join("untracked"), "post checkpoint").unwrap();
            let mut command = json!({"type":"checkpoint.rollback","commandId":"restore","threadId":"t","scopeId":"scope-t","checkpointId":"r-baseline"});
            command["rollbackIdentity"] = o3_server::checkpoint::workspace_identity(&root).unwrap();
            let dir = s.dir.join("rollback-journal");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("injected.json"), command.to_string()).unwrap();
            if boundary >= 1 {
                std::fs::write(root.join("file"), "baseline").unwrap();
                std::fs::remove_file(root.join("untracked")).unwrap();
            }
            if boundary >= 2 {
                let db = s.db.lock().unwrap();
                db.execute("UPDATE records SET body=json_set(body,'$.rollbackRequestId','restore') WHERE kind='thread' AND id='t'", []).unwrap();
                db.execute(
                    "INSERT INTO receipts VALUES('restore','{\"sequence\":0}')",
                    [],
                )
                .unwrap();
            }
            if boundary == 3 {
                std::fs::remove_file(dir.join("injected.json")).unwrap();
            }
        });
        let c = s.connect();
        let p = orchestration::thread_projection(&c, "t").unwrap();
        assert_eq!(p["thread"]["rollbackRequestId"], "restore", "{p}");
        assert_eq!(
            std::fs::read_to_string(s.dir.join("repo/file")).unwrap(),
            "baseline"
        );
        assert!(!s.dir.join("repo/untracked").exists());
        assert_eq!(
            s.state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM receipts WHERE command_id='restore'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn replaced_workspace_symlink_is_refused_without_touching_target() {
    let s = support::start(|s| {
        let root = s.dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("file"), "baseline").unwrap();
        let db = s.db.lock().unwrap();
        for (kind, id, v) in [
            ("thread", "t", json!({"id":"t","projectId":"p"})),
            (
                "runs",
                "r",
                json!({"id":"r","threadId":"t","ordinal":1,"status":"completed"}),
            ),
        ] {
            db.execute(
                "INSERT INTO records VALUES(?,?,?,?)",
                rusqlite::params![kind, id, "t", v.to_string()],
            )
            .unwrap();
        }
        drop(db);
        o3_server::checkpoint::capture(s, "t", "r", &root, true).unwrap();
        let target = s.dir.join("other");
        std::fs::rename(&root, &target).unwrap();
        std::os::unix::fs::symlink(&target, &root).unwrap();
        std::fs::write(target.join("file"), "other workspace edit").unwrap();
        std::fs::write(target.join("untracked"), "keep").unwrap();
    });
    let result = s.connect().rpc("orchestration.dispatchCommand", json!({"type":"checkpoint.rollback","commandId":"refuse","threadId":"t","scopeId":"scope-t","checkpointId":"r-baseline"}));
    assert!(result.is_err(), "replaced symlink authorized restore");
    assert_eq!(
        std::fs::read_to_string(s.dir.join("other/file")).unwrap(),
        "other workspace edit"
    );
    assert!(s.dir.join("other/untracked").exists());
}
