mod support;
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
    // Fixture trees are retained for private container teardown.
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
    // Fixture trees are retained for private container teardown.
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
            resolved_roots: Vec::new(),
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
    // Fixture trees are retained for private container teardown.
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
    let server = support::start(|_| {});
    let s = server.state.clone();
    o3_server::events::mutate_project(&s,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root})).unwrap();
    let mut p = json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"HEAD","branch":"fixture-branch"}});
    let result = o3_server::events::launch(&s, &p).unwrap();
    assert_eq!(
        result["projection"]["thread"]["worktreePath"],
        serde_json::Value::Null
    );
    assert!(support::wait_for(
        std::time::Duration::from_secs(10),
        || o3_server::events::projection(&s, "t").unwrap()["thread"]["worktreePath"].is_string()
    ));
    let path = o3_server::events::projection(&s, "t").unwrap()["thread"]["worktreePath"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(std::path::Path::new(&path).join(".git").exists());
    assert_eq!(o3_server::events::launch(&s, &p).unwrap()["resumed"], true);
    p["commandId"] = json!("reuse");
    p["reuseExistingThread"] = json!(true);
    p["workspaceStrategy"] =
        json!({"type":"existing_worktree","worktreePath":path,"branch":"fixture-branch"});
    assert_eq!(o3_server::events::launch(&s, &p).unwrap()["resumed"], false);
    p["commandId"] = json!("existing");
    p["threadId"] = json!("t2");
    p["reuseExistingThread"] = json!(false);
    p["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":path});
    assert!(o3_server::events::launch(&s, &p).is_ok());
    assert!(support::wait_for(
        std::time::Duration::from_secs(10),
        || s.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM thread_launches WHERE completed=0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
            == 0
    ));
    drop(s);
    // Fixture trees are retained for private container teardown.
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
    // Fixture trees are retained for private container teardown.
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
    // Fixture trees are retained for private container teardown.
}

#[test]
fn control_outbox_is_receipt_atomic_and_restart_reports_uncertain_delivery() {
    let dir = directory();
    let s = State::open(&dir).unwrap();
    fn record(
        db: &rusqlite::Connection,
        kind: &str,
        id: &str,
    ) -> Result<serde_json::Value, String> {
        let raw: String = db
            .query_row(
                "SELECT body FROM records WHERE kind=? AND id=?",
                [kind, id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }
    let command = json!({"type":"runtime-request.respond","commandId":"answer","threadId":"t","requestId":"approval","decision":"accept"});
    {
        let mut db = s.db.lock().unwrap();
        db.execute("INSERT INTO records VALUES('thread','t','t',?)", [json!({"id":"t","activeProviderThreadId":"claude-native","codexNativeThreadId":"codex-native"}).to_string()]).unwrap();
        let tx = db.transaction().unwrap();
        o3_server::outbox::stage(&tx, &command).unwrap();
        // Dropping the transaction must discard the outbox together with the receipt.
    }
    assert!(record(&s.db.lock().unwrap(), "controlOutbox", "answer").is_err());
    {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        o3_server::outbox::stage(&tx, &command).unwrap();
        tx.execute("INSERT INTO receipts VALUES('answer','{}')", [])
            .unwrap();
        tx.commit().unwrap();
    }
    drop(s);
    let s = State::open(&dir).unwrap();
    o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
    // A late channel-handoff write cannot erase an already detected delivery failure.
    o3_server::outbox::handed_off(&s, "answer").unwrap();
    let db = s.db.lock().unwrap();
    let entry = record(&db, "controlOutbox", "answer").unwrap();
    assert_eq!(entry["status"], "delivery-unknown");
    assert_eq!(entry["command"], command);
    let thread = record(&db, "thread", "t").unwrap();
    assert_eq!(thread["activeProviderThreadId"], "claude-native");
    assert_eq!(thread["codexNativeThreadId"], "codex-native");
    assert_eq!(thread["lastControlDeliveryError"]["commandId"], "answer");
    drop(db);
    drop(s);
    // Fixture trees are retained for private container teardown.
}

#[test]
fn restart_finalizes_streaming_transcript_and_provider_nodes() {
    let dir = directory();
    let s = State::open(&dir).unwrap();
    {
        let db = s.db.lock().unwrap();
        for (kind, id, value) in [
            (
                "thread",
                "t",
                json!({"id":"t","providerInstanceId":"claudeAgent"}),
            ),
            (
                "runs",
                "r",
                json!({"id":"r","threadId":"t","status":"running"}),
            ),
            (
                "nodes",
                "r",
                json!({"id":"r","runId":"r","status":"running"}),
            ),
            (
                "providerTurns",
                "r",
                json!({"id":"r","runId":"r","status":"running"}),
            ),
            (
                "messages",
                "assistant-r",
                json!({"id":"assistant-r","threadId":"t","runId":"r","role":"assistant","text":"partial","streaming":true}),
            ),
            (
                "turnItems",
                "tool",
                json!({"id":"tool","threadId":"t","runId":"r","type":"command_execution","status":"running"}),
            ),
        ] {
            db.execute(
                "INSERT INTO records VALUES(?,?,?,?)",
                rusqlite::params![kind, id, "t", value.to_string()],
            )
            .unwrap();
        }
    }
    o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
    let db = s.db.lock().unwrap();
    for (kind, id) in [
        ("runs", "r"),
        ("nodes", "r"),
        ("providerTurns", "r"),
        ("turnItems", "tool"),
    ] {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &db.query_row(
                    "SELECT body FROM records WHERE kind=? AND id=?",
                    [kind, id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap()
            )
            .unwrap()["status"],
            "failed"
        );
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &db.query_row(
                "SELECT body FROM records WHERE kind='messages' AND id='assistant-r'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap()
        )
        .unwrap()["streaming"],
        false
    );
    let count: i64 = db
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    drop(db);
    o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
    assert_eq!(
        s.db.lock()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        count
    );
    drop(s);
    // Fixture trees are retained for private container teardown.
}

#[test]
fn import_copies_t3_attachment_files_without_following_links_or_overwriting() {
    let dir = directory();
    let source = dir.join("attachments");
    std::fs::create_dir_all(&source).unwrap();
    let id = "thread-1-00000000-0000-4000-8000-000000000001";
    std::fs::write(source.join(format!("{id}.png")), b"png bytes").unwrap();
    std::fs::write(source.join("notes.txt"), b"not an attachment").unwrap();
    let outside = dir.join("outside.png");
    std::fs::write(&outside, b"outside").unwrap();
    let linked = "thread-1-00000000-0000-4000-8000-000000000002";
    std::os::unix::fs::symlink(&outside, source.join(format!("{linked}.png"))).unwrap();
    let state = State::open(&dir.join("target")).unwrap();
    let report = o3_server::import::attachments(&state, &source).unwrap();
    assert_eq!(report, json!({"copied":1,"alreadyPresent":0,"skipped":2}));
    let copied = o3_server::assets::attachment_path(&state, id).unwrap();
    assert_eq!(std::fs::read(&copied).unwrap(), b"png bytes");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(o3_server::assets::attachment_path(&state, linked).is_none());
    // A second run never replaces what is already in the profile.
    std::fs::write(&copied, b"edited").unwrap();
    let again = o3_server::import::attachments(&state, &source).unwrap();
    assert_eq!(again, json!({"copied":0,"alreadyPresent":1,"skipped":2}));
    assert_eq!(std::fs::read(&copied).unwrap(), b"edited");
    assert_eq!(
        std::fs::read(source.join(format!("{id}.png"))).unwrap(),
        b"png bytes"
    );
    // Fixture trees are retained for private container teardown.
}
