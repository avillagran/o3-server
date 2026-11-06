//! Migration-80 reducers and legacy compatibility. Execute through tools/test-shared-nightly.
use o3_server::{events, store::State, t3_shared};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

const SCHEMA: &str = include_str!("fixtures/t3-statev2-80.sql");

fn profile(schema: &str) -> PathBuf {
    assert!(
        std::path::Path::new("/run/.containerenv").is_file()
            || std::path::Path::new("/.dockerenv").is_file()
    );
    let dir = std::env::temp_dir().join(format!(
        "o3-shared-nightly-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&dir).unwrap();
    Connection::open(t3_shared::database(&dir))
        .unwrap()
        .execute_batch(schema)
        .unwrap();
    dir
}

fn create(state: &Arc<State>) {
    events::mutate_project(state, &json!({"type":"project.create","commandId":"create-project","projectId":"p","title":"Project","workspaceRoot":"/tmp/workspace"})).unwrap();
    events::dispatch(state, &json!({"type":"thread.create","commandId":"create-thread","threadId":"t","projectId":"p","title":"Thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
}

fn put(
    db: &Connection,
    kind: &str,
    id: &str,
    tid: &str,
    value: &Value,
) -> Result<(), rusqlite::Error> {
    db.execute("INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body", rusqlite::params![kind,id,tid,value.to_string()]).map(|_| ())
}

fn row(db: &Connection, sql: &str) -> String {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn nightly_projects_preserve_pin_archive_and_subagent_delivery_columns() {
    let dir = profile(SCHEMA);
    let state = Arc::new(State::open(&dir).unwrap());
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    create(&state);
    events::mutate_project(&state, &json!({"type":"project.update","commandId":"pin","projectId":"p","pinned":true,"archived":true})).unwrap();
    let mut subagent = json!({"id":"child","threadId":"t","runId":null,"parentNodeId":"parent","origin":"provider_native","createdBy":"agent","driver":"codex","providerInstanceId":"codex","providerThreadId":null,"childThreadId":null,"nativeTaskRef":null,"prompt":"Task","title":null,"model":null,"status":"completed","result":"Done","startedAt":null,"completedAt":"2026-11-06T12:00:00.000Z","updatedAt":"2026-11-06T12:00:00.000Z","completionDelivery":{"state":"pending","observedByRunId":null}});
    put(
        &state.db.lock().unwrap(),
        "subagents",
        "child",
        "t",
        &subagent,
    )
    .unwrap();
    assert!(t3_shared::export(&state, &mut shared).unwrap() > 0);
    assert_eq!(row(&shared, "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id='child'"), "pending");
    let flags: (String, String) = shared
        .query_row(
            "SELECT pinned_at,archived_at FROM projection_projects WHERE project_id='p'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(flags.0, flags.1);
    assert!(!flags.0.is_empty());
    assert_eq!(row(&shared, "SELECT json_extract(payload_json,'$.pinnedAt') FROM orchestration_events WHERE aggregate_kind='project' ORDER BY sequence DESC LIMIT 1"), flags.0);

    // The upstream reducer retains delivery information when a compact update omits it.
    subagent
        .as_object_mut()
        .unwrap()
        .remove("completionDelivery");
    subagent["updatedAt"] = json!("2026-11-06T12:01:00.000Z");
    put(
        &state.db.lock().unwrap(),
        "subagents",
        "child",
        "t",
        &subagent,
    )
    .unwrap();
    t3_shared::export(&state, &mut shared).unwrap();
    assert_eq!(row(&shared, "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id='child'"), "pending");
    assert_eq!(row(&shared, "SELECT json_extract(payload_json,'$.completionDelivery.state') FROM orchestration_v2_projection_subagents WHERE subagent_id='child'"), "pending");
    subagent["completionDelivery"] = json!({"state":"acknowledged","observedByRunId":null});
    subagent["updatedAt"] = json!("2026-11-06T12:02:00.000Z");
    put(
        &state.db.lock().unwrap(),
        "subagents",
        "child",
        "t",
        &subagent,
    )
    .unwrap();
    t3_shared::export(&state, &mut shared).unwrap();
    assert_eq!(row(&shared, "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id='child'"), "acknowledged");

    // A new O3 profile adopts T3's project fields instead of dropping them.
    let adopted_dir = profile(SCHEMA);
    let mut destination = Connection::open(t3_shared::database(&adopted_dir)).unwrap();
    rusqlite::backup::Backup::new(&shared, &mut destination)
        .unwrap()
        .run_to_completion(128, std::time::Duration::ZERO, None)
        .unwrap();
    destination.execute("DELETE FROM o3_sync", []).unwrap();
    drop(destination);
    let adopted = State::open(&adopted_dir).unwrap();
    let _ = t3_shared::start(&adopted).unwrap().unwrap();
    let project: Value = serde_json::from_str(&row(
        &adopted.db.lock().unwrap(),
        "SELECT body FROM records WHERE kind='project' AND id='p'",
    ))
    .unwrap();
    assert_eq!(project["pinnedAt"], flags.0);
    assert_eq!(project["archivedAt"], flags.1);
    events::mutate_project(&state, &json!({"type":"project.update","commandId":"unpin","projectId":"p","pinned":false,"archived":false})).unwrap();
    t3_shared::export(&state, &mut shared).unwrap();
    let flags: (Option<String>, Option<String>) = shared
        .query_row(
            "SELECT pinned_at,archived_at FROM projection_projects WHERE project_id='p'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(flags, (None, None));
    assert_eq!(t3_shared::export(&state, &mut shared).unwrap(), 0);
}

#[test]
fn migration_claim_without_required_columns_is_refused_before_sync_writes() {
    let dir = profile(include_str!("fixtures/t3-statev2-73.sql"));
    let shared = Connection::open(t3_shared::database(&dir)).unwrap();
    shared.execute("INSERT INTO effect_sql_migrations(migration_id,name) VALUES(80,'ScheduledTaskThreadMode')", []).unwrap();
    assert!(t3_shared::open(&dir)
        .unwrap_err()
        .contains("completion_delivery_state"));
    assert_eq!(
        shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='o3_sync'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn intermediate_migrations_are_not_implicitly_trusted() {
    let dir = profile(SCHEMA);
    let shared = Connection::open(t3_shared::database(&dir)).unwrap();
    shared
        .execute(
            "DELETE FROM effect_sql_migrations WHERE migration_id=80",
            [],
        )
        .unwrap();
    assert!(t3_shared::open(&dir)
        .unwrap_err()
        .contains("DelegatedRecoveryIndexes"));
    assert_eq!(
        shared
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='o3_sync'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn legacy_schema_retains_unsupported_project_intent_as_a_rejection() {
    let dir = profile(include_str!("fixtures/t3-statev2-73.sql"));
    let state = Arc::new(State::open(&dir).unwrap());
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    create(&state);
    t3_shared::export(&state, &mut shared).unwrap();
    events::mutate_project(
        &state,
        &json!({"type":"project.update","commandId":"pin","projectId":"p","pinned":true}),
    )
    .unwrap();
    t3_shared::export(&state, &mut shared).unwrap();
    let db = state.db.lock().unwrap();
    assert!(row(
        &db,
        "SELECT error FROM t3_rejects WHERE kind='project' AND id='p'"
    )
    .contains("cannot store project pinnedAt"));
    let project: Value = serde_json::from_str(&row(
        &db,
        "SELECT body FROM records WHERE kind='project' AND id='p'",
    ))
    .unwrap();
    assert!(!project["pinnedAt"].is_null());
}

#[test]
fn newly_supported_project_flags_are_revisited_and_clear_resolved_rejections() {
    let dir = profile(SCHEMA);
    let state = Arc::new(State::open(&dir).unwrap());
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    create(&state);
    events::mutate_project(
        &state,
        &json!({"type":"project.update","commandId":"pin","projectId":"p","pinned":true}),
    )
    .unwrap();
    t3_shared::export(&state, &mut shared).unwrap();
    // Represent the old exporter's state: its schema had no pin column and its dirty mark
    // was already consumed with a rejection. T3's migration adds the column without an event.
    shared
        .execute("UPDATE projection_projects SET pinned_at=NULL", [])
        .unwrap();
    shared
        .execute("DELETE FROM o3_sync WHERE name='o3ProjectFlagsVersion'", [])
        .unwrap();
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO t3_rejects VALUES('project','p','Legacy missing column',0)",
            [],
        )
        .unwrap();
    drop(shared);
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    assert_eq!(t3_shared::export(&state, &mut shared).unwrap(), 1);
    assert!(!row(
        &shared,
        "SELECT pinned_at FROM projection_projects WHERE project_id='p'"
    )
    .is_empty());
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM t3_rejects", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(shared);
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    assert_eq!(t3_shared::export(&state, &mut shared).unwrap(), 0);
}

#[test]
fn nightly_search_triggers_exclude_attachment_placeholders_on_export() {
    let dir = profile(SCHEMA);
    let state = Arc::new(State::open(&dir).unwrap());
    let mut shared = t3_shared::start(&state).unwrap().unwrap();
    create(&state);
    let placeholder = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
    for (id, text) in [
        ("attachment", placeholder.to_string()),
        (
            "prefixed",
            format!("uLtRaThInK:\t\n\u{b}\u{c}\r \u{a0}\u{2003}{placeholder}"),
        ),
        ("normal", "A searchable attachment description".to_string()),
    ] {
        let message = json!({"id":id,"threadId":"t","runId":null,"nodeId":null,"role":"user","createdBy":"user","text":text,"attachments":[],"streaming":false,"createdAt":"2026-11-06T12:00:00.000Z","updatedAt":"2026-11-06T12:00:00.000Z"});
        put(&state.db.lock().unwrap(), "messages", id, "t", &message).unwrap();
    }
    assert_eq!(t3_shared::export(&state, &mut shared).unwrap(), 5);
    assert_eq!(
        shared
            .query_row(
                "SELECT count(*) FROM orchestration_v2_message_search",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(row(&shared, "SELECT m.message_id FROM orchestration_v2_projection_messages m JOIN orchestration_v2_message_search f ON f.rowid=m.rowid"),"normal");
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM t3_rejects", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
