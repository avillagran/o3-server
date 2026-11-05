//! O3 and T3 share one `statev2.sqlite`: O3 writes its changes there the way T3 does and takes in
//! what T3 wrote. The fixture is T3's schema at migration 73, without data.
use o3_server::{store::State, t3_shared};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::Arc;

const SCHEMA: &str = include_str!("fixtures/t3-statev2-73.sql");

/// A private profile directory with T3's empty database in `userdata/`.
fn profile() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "o3-t3-shared-{}",
        o3_server::store::random_id().unwrap()
    ));
    let userdata = root.join("userdata");
    std::fs::create_dir_all(&userdata).unwrap();
    Connection::open(t3_shared::database(&userdata))
        .unwrap()
        .execute_batch(SCHEMA)
        .unwrap();
    root
}

fn t3(userdata: &std::path::Path) -> Connection {
    Connection::open(t3_shared::database(userdata)).unwrap()
}

fn one<T: rusqlite::types::FromSql>(db: &Connection, sql: &str) -> T {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// The checks T3's `ProjectionMaintenance.verify` makes before it trusts its projections.
fn assert_t3_verifies(db: &Connection) {
    let latest: i64 = one(
        db,
        "SELECT coalesce(max(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'",
    );
    let (version, last): (i64, i64) = db
        .query_row(
            "SELECT schema_version,last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (version, last),
        (t3_shared::PROJECTION_SCHEMA_VERSION, latest)
    );
    let unanchored: i64 = one(
        db,
        "SELECT count(*) FROM orchestration_v2_projection_threads t WHERE NOT EXISTS (SELECT 1 FROM orchestration_events e WHERE e.application_event_version=2 AND e.aggregate_kind='thread' AND e.event_type='thread.created' AND e.stream_id=t.thread_id)",
    );
    let orphaned: i64 = one(
        db,
        "SELECT count(DISTINCT stream_id) FROM orchestration_events e WHERE application_event_version=2 AND aggregate_kind='thread' AND event_type='thread.created' AND NOT EXISTS (SELECT 1 FROM orchestration_v2_projection_threads t WHERE t.thread_id=e.stream_id)",
    );
    assert_eq!((unanchored, orphaned), (0, 0));
}

fn exporter_drains(state: &Arc<State>, db: Connection) {
    t3_shared::Exporter::spawn(state.clone(), db).finish();
}

#[test]
fn o3_and_t3_hand_the_same_history_back_and_forth() {
    let root = profile();
    let userdata = root.join("userdata");

    // O3 owns the profile: what it creates lands in T3's base.
    {
        let state = Arc::new(State::open(&userdata).unwrap());
        let shared = t3_shared::start(&state).unwrap().expect("T3 base");
        o3_server::events::mutate_project(&state, &json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root})).unwrap();
        o3_server::events::dispatch(&state, &json!({"type":"thread.create","commandId":"c","threadId":"t","projectId":"p","title":"First title","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        exporter_drains(&state, shared);
    }
    let db = t3(&userdata);
    assert_eq!(
        one::<String>(
            &db,
            "SELECT title FROM projection_projects WHERE project_id='p'"
        ),
        "Project"
    );
    assert_eq!(
        one::<String>(&db, "SELECT event_type FROM orchestration_events WHERE aggregate_kind='project' AND stream_id='p' AND stream_version=0"),
        "project.created"
    );
    let thread: Value = serde_json::from_str(&one::<String>(
        &db,
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id='t'",
    ))
    .unwrap();
    assert_eq!(thread["title"], "First title");
    assert_eq!(
        one::<String>(&db, "SELECT event_type FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id='t' AND stream_version=0"),
        "thread.created"
    );
    assert_t3_verifies(&db);

    // T3 owns it next and renames the thread the way its reducer does; its encoder drops fields
    // only O3 keeps.
    let mut renamed = thread.clone();
    renamed
        .as_object_mut()
        .unwrap()
        .insert("o3OnlyField".into(), json!(true));
    {
        let state = State::open(&userdata).unwrap();
        let mut body: Value = serde_json::from_str(
            &state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT body FROM records WHERE kind='thread' AND id='t'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        body["o3OnlyField"] = json!(true);
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE records SET body=? WHERE kind='thread' AND id='t'",
                [body.to_string()],
            )
            .unwrap();
        state
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM t3_dirty", [])
            .unwrap();
    }
    renamed.as_object_mut().unwrap().remove("o3OnlyField");
    renamed["title"] = json!("Renamed in T3");
    renamed["updatedAt"] = json!("2030-01-01T00:00:00.000Z");
    db.execute(
        "INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES('t3-rename','thread','t',(SELECT max(stream_version)+1 FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id='t'),'thread.metadata-updated','2030-01-01T00:00:00.000Z','client',?1,'{}',2)",
        [renamed.to_string()],
    )
    .unwrap();
    db.execute(
        "UPDATE orchestration_v2_projection_threads SET title='Renamed in T3',updated_at='2030-01-01T00:00:00.000Z',payload_json=?1 WHERE thread_id='t'",
        [renamed.to_string()],
    )
    .unwrap();
    db.execute(
        "UPDATE orchestration_v2_projection_metadata SET last_sequence=(SELECT max(sequence) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread')",
        [],
    )
    .unwrap();
    let t3_events: i64 = one(&db, "SELECT count(*) FROM orchestration_events");

    // O3 again: it takes the rename in, and writing back changes nothing.
    {
        let state = Arc::new(State::open(&userdata).unwrap());
        let mut shared = t3_shared::start(&state).unwrap().expect("T3 base");
        let projection = o3_server::events::projection(&state, "t").unwrap();
        assert_eq!(projection["thread"]["title"], "Renamed in T3");
        assert_eq!(
            t3_shared::export(&state, &mut shared).unwrap(),
            0,
            "taken-in records were written back"
        );
        let shell = o3_server::events::shell(&state).unwrap().to_string();
        assert!(shell.contains("Renamed in T3"), "{shell}");
    }
    assert_eq!(
        one::<i64>(&db, "SELECT count(*) FROM orchestration_events"),
        t3_events
    );
    assert_t3_verifies(&db);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn history_only_in_t3_is_adopted_and_only_new_o3_records_are_written() {
    let root = profile();
    let userdata = root.join("userdata");
    let db = t3(&userdata);
    // A thread T3 wrote before O3 ever opened this profile.
    let thread = json!({"id":"old","projectId":"p","title":"From T3","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","activeProviderThreadId":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","archivedAt":null,"deletedAt":null});
    db.execute(
        "INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES('t3-created','thread','old',0,'thread.created','2026-01-01T00:00:00.000Z','client',?1,'{}',2)",
        [thread.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode,created_at,updated_at,payload_json) VALUES('old','p','From T3','codex','codex','full-access','default','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',?1)",
        [thread.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO orchestration_v2_projection_metadata VALUES('thread-projections',2,1,'2026-01-01T00:00:00.000Z')",
        [],
    )
    .unwrap();
    let state = Arc::new(State::open(&userdata).unwrap());
    let shared = t3_shared::start(&state).unwrap().expect("T3 base");
    assert_eq!(
        o3_server::events::projection(&state, "old").unwrap()["thread"]["title"],
        "From T3"
    );
    exporter_drains(&state, shared);
    // The adopted thread was already T3's: nothing about it was written back.
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT count(*) FROM orchestration_events WHERE stream_id='old'"
        ),
        1
    );
    assert_t3_verifies(&db);
    drop(state);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_t3_schema_newer_than_o3_knows_is_never_written() {
    let root = profile();
    let userdata = root.join("userdata");
    let db = t3(&userdata);
    db.execute(
        "INSERT INTO effect_sql_migrations VALUES(?1,'2030-01-01 00:00:00','FutureMigration')",
        params![t3_shared::MIGRATION_HEAD.0 + 1],
    )
    .unwrap();
    let state = State::open(&userdata).unwrap();
    let error = t3_shared::start(&state).expect_err("refused");
    assert!(error.contains("FutureMigration"), "{error}");
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT count(*) FROM sqlite_master WHERE name='o3_sync'"
        ),
        0
    );
    drop(state);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_profile_without_t3_keeps_marks_until_t3_creates_its_base() {
    let root = std::env::temp_dir().join(format!(
        "o3-t3-shared-{}",
        o3_server::store::random_id().unwrap()
    ));
    let userdata = root.join("userdata");
    let state = Arc::new(State::open(&userdata).unwrap());
    assert!(t3_shared::start(&state).unwrap().is_none());
    o3_server::events::mutate_project(&state, &json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root})).unwrap();
    let marked: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM t3_dirty WHERE kind='project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marked, 1);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn put_record(state: &State, kind: &str, id: &str, tid: &str, body: &Value) {
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",
            params![kind, id, tid, body.to_string()],
        )
        .unwrap();
}

fn run(id: &str, ordinal: i64, at: &str) -> Value {
    json!({"id":id,"threadId":"t","ordinal":ordinal,"providerInstanceId":"codex","providerThreadId":null,"status":"completed","requestedAt":at,"completedAt":at})
}

#[test]
fn runs_o3_numbered_without_seeing_t3_move_after_t3_and_o3_values_fit_t3() {
    let root = profile();
    let userdata = root.join("userdata");
    let db = t3(&userdata);
    let thread = json!({"id":"t","projectId":"p","title":"Forked","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"text-only","interactionMode":"default","activeProviderThreadId":null,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z","archivedAt":null,"deletedAt":null});
    db.execute(
        "INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) VALUES('created','thread','t',0,'thread.created','2026-01-01T00:00:00.000Z','client',?1,'{}',2)",
        [thread.to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode,created_at,updated_at,payload_json) VALUES('t','p','Forked','codex','codex','approval-required','default','2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',?1)",
        [thread.to_string()],
    )
    .unwrap();
    for (id, ordinal) in [("t3-run-1", 1), ("t3-run-2", 2)] {
        db.execute(
            "INSERT INTO orchestration_v2_projection_runs(run_id,thread_id,ordinal,provider,provider_instance_id,status,requested_at,payload_json) VALUES(?1,'t',?2,'codex','codex','completed','2026-01-01T00:00:00.000Z',?3)",
            params![id, ordinal, run(id, ordinal, "2026-01-01T00:00:00.000Z").to_string()],
        )
        .unwrap();
    }
    let state = Arc::new(State::open(&userdata).unwrap());
    put_record(&state, "thread", "t", "t", &thread);
    put_record(
        &state,
        "runs",
        "o3-run-a",
        "t",
        &run("o3-run-a", 1, "2026-02-01T00:00:00.000Z"),
    );
    put_record(
        &state,
        "runs",
        "o3-run-b",
        "t",
        &run("o3-run-b", 2, "2026-02-02T00:00:00.000Z"),
    );
    put_record(
        &state,
        "checkpoints",
        "cp",
        "t",
        &json!({"id":"cp","threadId":"t","scopeId":"scope","runId":"o3-run-b","ordinalWithinScope":2,"appRunOrdinal":2,"ref":"refs/o3/checkpoints/x","status":"ready"}),
    );
    for (id, at) in [
        ("turn-early", "2026-02-01T00:00:00.000Z"),
        ("turn-late", "2026-02-02T00:00:00.000Z"),
    ] {
        put_record(
            &state,
            "providerTurns",
            id,
            "t",
            &json!({"id":id,"providerThreadId":"pt","nodeId":"n","runAttemptId":null,"ordinal":3,"status":"completed","startedAt":at,"completedAt":at}),
        );
    }
    for baseline in ["base-a", "base-b"] {
        put_record(
            &state,
            "checkpoints",
            baseline,
            "t",
            &json!({"id":baseline,"threadId":"t","scopeId":"scope","runId":"o3-run-a","ordinalWithinScope":0,"appRunOrdinal":0,"ref":"refs/o3/checkpoints/b","status":"ready"}),
        );
    }
    put_record(
        &state,
        "providerSessions",
        "session",
        "t",
        &json!({"id":"session","driver":"codex","providerInstanceId":"codex","status":"disconnected","cwd":"/","model":null,"capabilities":{},"createdAt":"2026-02-01T00:00:00.000Z","updatedAt":"2026-02-01T00:00:00.000Z","lastError":null}),
    );
    let shared = t3_shared::start(&state).unwrap().expect("T3 base");
    exporter_drains(&state, shared);

    let ordinal = |id: &str| -> i64 {
        db.query_row(
            "SELECT ordinal FROM orchestration_v2_projection_runs WHERE run_id=?",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        (
            ordinal("t3-run-1"),
            ordinal("t3-run-2"),
            ordinal("o3-run-a"),
            ordinal("o3-run-b")
        ),
        (1, 2, 3, 4)
    );
    let checkpoint: Value = serde_json::from_str(&one::<String>(
        &db,
        "SELECT payload_json FROM orchestration_v2_projection_checkpoints WHERE checkpoint_id='cp'",
    ))
    .unwrap();
    assert_eq!(checkpoint["appRunOrdinal"], 4);
    let turn = |id: &str| -> i64 {
        db.query_row("SELECT ordinal FROM orchestration_v2_projection_provider_turns WHERE provider_turn_id=?", [id], |r| r.get(0))
            .unwrap()
    };
    assert_eq!((turn("turn-early"), turn("turn-late")), (3, 4));
    assert_eq!(
        one::<i64>(&db, "SELECT count(*) FROM orchestration_v2_projection_checkpoints WHERE scope_id='scope' AND ordinal_within_scope=0"),
        1
    );
    assert!(checkpoint["nodeId"].is_string() && checkpoint["capturedAt"].is_string());
    assert_eq!(
        one::<String>(&db, "SELECT status FROM orchestration_v2_projection_provider_sessions WHERE provider_session_id='session'"),
        "stopped"
    );
    assert_eq!(
        one::<String>(
            &db,
            "SELECT runtime_mode FROM orchestration_v2_projection_threads WHERE thread_id='t'"
        ),
        "approval-required"
    );
    let rejects: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM t3_rejects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rejects, 0);
    assert_t3_verifies(&db);
    drop(state);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sessions_issued_by_either_server_are_accepted_by_the_other() {
    let root = profile();
    let userdata = root.join("userdata");
    let db = t3(&userdata);
    let scopes: Vec<String> = o3_server::store::STANDARD_SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    db.execute(
        "INSERT INTO auth_sessions(session_id,subject,scopes,method,client_label,client_device_type,issued_at,expires_at) VALUES('t3-session','owner',?1,'bearer-access-token','phone','mobile','2026-01-01T00:00:00.000Z','2099-01-01T00:00:00.000Z')",
        [json!(scopes).to_string()],
    )
    .unwrap();
    let state = Arc::new(State::open(&userdata).unwrap());
    let shared = t3_shared::start(&state).unwrap().expect("T3 base");
    let (o3_scopes, client): (String, String) = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT scopes,client FROM auth_sessions WHERE session_id='t3-session'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(o3_scopes, scopes.join(" "));
    assert!(client.contains("phone"), "{client}");

    let token = state
        .issue_session("owner", &scopes, 3_600_000, "desktop")
        .unwrap();
    assert!(!token.is_empty());
    let issued: String = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT session_id FROM auth_sessions WHERE session_id<>'t3-session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(state.revoke_session("t3-session").unwrap());
    exporter_drains(&state, shared);
    let (t3_scopes, label, revoked): (String, String, Option<String>) = db
        .query_row(
            "SELECT scopes,client_label,revoked_at FROM auth_sessions WHERE session_id=?",
            [&issued],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&t3_scopes).unwrap(),
        scopes
    );
    assert_eq!((label.as_str(), revoked), ("desktop", None));
    assert!(one::<Option<String>>(
        &db,
        "SELECT revoked_at FROM auth_sessions WHERE session_id='t3-session'"
    )
    .is_some());
    drop(state);
    // A session O3 issued before this profile was shared reaches T3 on the next start too.
    let state = Arc::new(State::open(&userdata).unwrap());
    state
        .db
        .lock()
        .unwrap()
        .execute("INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at) VALUES('older','owner','orchestration:read','bearer-access-token','{}',1,4102444800000)", [])
        .unwrap();
    state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM t3_dirty", [])
        .unwrap();
    let shared = t3_shared::start(&state).unwrap().expect("T3 base");
    exporter_drains(&state, shared);
    assert_eq!(
        one::<String>(
            &db,
            "SELECT scopes FROM auth_sessions WHERE session_id='older'"
        ),
        "[\"orchestration:read\"]"
    );
    drop(state);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
