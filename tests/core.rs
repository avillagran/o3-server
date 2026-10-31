#[path = "support/wire.rs"]
mod wire;
use o3_server::{import::from_t3, store::State};
use serde_json::json;
fn directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "o3-core-test-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn import_backup_includes_wal_is_idempotent_and_preserves_source() {
    let dir = directory();
    let path = dir.join("source.sqlite");
    let source = rusqlite::Connection::open(&path).unwrap();
    source
        .execute_batch(include_str!("fixtures/t3-import.sql"))
        .unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    source.execute("INSERT INTO projection_projects VALUES('p','Project','/var/tmp/o3-build/tmp',NULL,'[]','now','now',NULL)",[]).unwrap();
    let thread = json!({"id":"t","projectId":"p","title":"Thread","updatedAt":"now"});
    source.execute("INSERT INTO orchestration_v2_projection_threads VALUES('t','p','Thread','codex','full-access','default',NULL,'now','now',NULL,NULL,?)",[thread.to_string()]).unwrap();
    let message = json!({"id":"m","threadId":"t","text":"durable","role":"user"});
    source.execute("INSERT INTO orchestration_v2_projection_messages VALUES('m','t',NULL,NULL,'user',0,'now','now',?)",[message.to_string()]).unwrap();
    let item = json!({"id":"i","threadId":"t","ordinal":1,"type":"user_message","messageId":"m","text":"durable"});
    source.execute("INSERT INTO orchestration_v2_projection_turn_items VALUES('i','t',NULL,NULL,NULL,NULL,NULL,1,'user_message','completed','now',?)",[item.to_string()]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let wal = std::fs::read(path.with_extension("sqlite-wal")).unwrap();
    let state = State::open(&dir.join("target")).unwrap();
    let result = from_t3(&state, &path).unwrap();
    assert_eq!(
        result,
        json!({"project":1,"thread":1,"messages":1,"turnItems":1})
    );
    assert_eq!(
        from_t3(&state, &path).unwrap(),
        json!({"project":0,"thread":0,"messages":0,"turnItems":0})
    );
    assert_eq!(before, std::fs::read(&path).unwrap());
    assert_eq!(
        wal,
        std::fs::read(path.with_extension("sqlite-wal")).unwrap()
    );
    let db = state.db.lock().unwrap();
    let text: String = db
        .query_row(
            "SELECT json_extract(body,'$.text') FROM records WHERE kind='messages'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(text, "durable");
    drop(db);
    drop(state);
    drop(source);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn restart_recovers_dead_runs_requests_and_sessions_but_preserves_deferred_runs() {
    let dir = directory();
    let s = State::open(&dir).unwrap();
    for (kind, id, v) in [
        ("thread", "t", json!({"id":"t"})),
        (
            "runs",
            "active",
            json!({"id":"active","threadId":"t","status":"running"}),
        ),
        (
            "runs",
            "deferred",
            json!({"id":"deferred","threadId":"t","status":"preparing","executionRequested":false}),
        ),
        (
            "runtimeRequests",
            "request",
            json!({"id":"request","threadId":"t","status":"pending"}),
        ),
        (
            "providerSessions",
            "session",
            json!({"id":"session","threadId":"t","status":"ready"}),
        ),
    ] {
        s.db.lock()
            .unwrap()
            .execute(
                "INSERT INTO records VALUES(?,?,?,?)",
                rusqlite::params![kind, id, "t", v.to_string()],
            )
            .unwrap();
    }
    drop(s);
    let s = State::open(&dir).unwrap();
    o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
    let db = s.db.lock().unwrap();
    for (id, status) in [
        ("active", "failed"),
        ("deferred", "preparing"),
        ("request", "cancelled"),
        ("session", "disconnected"),
    ] {
        let actual: String = db
            .query_row(
                "SELECT json_extract(body,'$.status') FROM records WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actual, status);
    }
    let count: i64 = db
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
    drop(db);
    drop(s);
    let s = State::open(&dir).unwrap();
    o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
    assert_eq!(
        s.db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn snapshot_is_consistent_and_chunks_are_bounded() {
    let dir = directory();
    let s = State::open(&dir).unwrap();
    s.db.lock()
        .unwrap()
        .execute(
            "INSERT INTO records VALUES('thread','t','t',?)",
            [json!({"id":"t","updatedAt":"now"}).to_string()],
        )
        .unwrap();
    for i in 0..40 {
        let text = "x".repeat(4096);
        let db = s.db.lock().unwrap();
        let m = json!({"id":format!("m{i}"),"threadId":"t","text":text});
        db.execute(
            "INSERT INTO records VALUES('messages',?,'t',?)",
            rusqlite::params![format!("m{i}"), m.to_string()],
        )
        .unwrap();
        let item = json!({"id":format!("i{i}"),"messageId":format!("m{i}"),"type":"user_message","text":text});
        db.execute(
            "INSERT INTO records VALUES('turnItems',?,'t',?)",
            rusqlite::params![format!("i{i}"), item.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO records VALUES('visibleTurnItems',?,'t',?)",
            rusqlite::params![
                format!("i{i}"),
                json!({"sourceThreadId":"t","visibility":"local","item":item}).to_string()
            ],
        )
        .unwrap();
    }
    let (_, mut rx) = o3_server::snapshot::start(
        &s,
        Some("t".into()),
        o3_server::snapshot::Options {
            bounded: true,
            dedupe: true,
            compact_ids: true,
            compact_items: true,
        },
        None,
    )
    .unwrap();
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM records WHERE kind='messages'", [])
        .unwrap();
    let mut bytes = Vec::new();
    while let Some(part) = rx.recv().await {
        let part = part.unwrap();
        assert!(part.len() <= 16384);
        bytes.extend(part);
    }
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v["ids"].is_array());
    assert_eq!(v["projection"]["turnItems"], json!([]));
    wire::restore_snapshot(&mut v).unwrap();
    assert_eq!(v["projection"]["messages"].as_array().unwrap().len(), 10);
    assert_eq!(
        v["projection"]["visibleTurnItems"][0]["item"]["text"],
        "x".repeat(4096)
    );
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn worktree_and_reuse_launch() {
    let dir = directory();
    let root = dir.join("repo");
    std::fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "fixture",
    ]);
    let s = std::sync::Arc::new(State::open(&dir.join("profile")).unwrap());
    o3_server::events::mutate_project(&s,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root})).unwrap();
    let mut p = json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"HEAD","branch":"fixture-branch"}});
    let result = o3_server::events::launch(&s, &p).unwrap();
    let path = result["projection"]["thread"]["worktreePath"]
        .as_str()
        .unwrap();
    assert!(std::path::Path::new(path).join(".git").exists());
    assert_eq!(o3_server::events::launch(&s, &p).unwrap(), result);
    p["commandId"] = json!("reuse");
    p["reuseExistingThread"] = json!(true);
    assert_eq!(o3_server::events::launch(&s, &p).unwrap()["resumed"], true);
    p["commandId"] = json!("existing");
    p["threadId"] = json!("t2");
    p["reuseExistingThread"] = json!(false);
    p["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":path});
    assert!(o3_server::events::launch(&s, &p).is_ok());
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn cli_import_operates_only_in_isolated_profile_and_rolls_back_bad_source() {
    let dir = directory();
    let path = dir.join("fixture.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("fixtures/t3-import.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO projection_projects VALUES('p','Project','/var/tmp/o3-build/tmp',NULL,'[]','now','now',NULL)",
        [],
    )
    .unwrap();
    drop(db);
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
            .args(["import", "--from"])
            .arg(&path)
            .arg("--base-dir")
            .arg(dir.join("profile"))
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.stdout).unwrap()["project"],
        1
    );
    let second = run();
    assert!(second.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&second.stdout).unwrap()["project"],
        0
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DROP TABLE orchestration_v2_projection_messages", [])
        .unwrap();
    db.execute("INSERT INTO projection_projects VALUES('rollback','Rollback','/var/tmp/o3-build/tmp',NULL,'[]','now','now',NULL)",[]).unwrap();
    drop(db);
    assert!(!run().status.success());
    let target = rusqlite::Connection::open(dir.join("profile/userdata/o3.sqlite")).unwrap();
    assert_eq!(
        target
            .query_row(
                "SELECT count(*) FROM records WHERE id='rollback'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(target);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn shell_stream_has_no_ten_thousand_project_cap() {
    let dir = directory();
    let s = State::open(&dir).unwrap();
    {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        for i in 0..10002 {
            tx.execute(
                "INSERT INTO records VALUES('project',?,'',?)",
                rusqlite::params![
                    format!("p{i}"),
                    json!({"id":format!("p{i}"),"deletedAt":null}).to_string()
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let (_, mut rx) = o3_server::snapshot::start(&s, None, Default::default(), None).unwrap();
    let mut bytes = Vec::new();
    let mut chunks = 0;
    while let Some(part) = rx.recv().await {
        let part = part.unwrap();
        assert!(part.len() <= 16384);
        chunks += 1;
        bytes.extend(part);
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["projects"].as_array().unwrap().len(), 10002);
    assert!(chunks > 1);
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}
