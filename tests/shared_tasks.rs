//! Event-free task handover using a redacted capture of the official 4262 runtime.
use o3_server::{scheduled, store::State, t3_shared};
use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

fn profile() -> (PathBuf, Arc<State>, Connection) {
    assert!(
        std::path::Path::new("/run/.containerenv").is_file()
            || std::path::Path::new("/.dockerenv").is_file()
    );
    let dir = std::env::temp_dir().join(format!(
        "o3-shared-tasks-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&dir).unwrap();
    let native = Connection::open(t3_shared::database(&dir)).unwrap();
    native
        .execute_batch(include_str!("fixtures/t3-statev2-80.sql"))
        .unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/t3-task-store-4262.json")).unwrap();
    for table in [
        "scheduled_tasks",
        "scheduled_task_webhook_deliveries",
        "scheduled_task_webhook_pending",
        "scheduled_task_webhook_replay_keys",
        "scheduled_task_webhook_relay_deliveries",
    ] {
        for row in fixture["tables"][table].as_array().unwrap() {
            let fields = row.as_object().unwrap();
            let columns = fields.keys().cloned().collect::<Vec<_>>().join(",");
            let values = fields.values().map(|v| match v {
                Value::Null => rusqlite::types::Value::Null,
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                _ => panic!("Unexpected fixture SQL value"),
            });
            native
                .execute(
                    &format!(
                        "INSERT INTO {table} ({columns}) VALUES ({})",
                        vec!["?"; fields.len()].join(",")
                    ),
                    params_from_iter(values),
                )
                .unwrap();
        }
    }
    let state = Arc::new(State::open(&dir).unwrap());
    (dir, state, native)
}
fn text(db: &Connection, sql: &str) -> String {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}
fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn task(s: &State, id: &str) -> Value {
    let body: String =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT body FROM records WHERE kind='scheduledTasks' AND id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
    serde_json::from_str(&body).unwrap()
}
fn rename(s: &State, id: &str, title: &str) {
    let mut value = task(s, id);
    value["title"] = json!(title);
    s.db.lock()
        .unwrap()
        .execute(
            "UPDATE records SET body=?1 WHERE kind='scheduledTasks' AND id=?2",
            params![value.to_string(), id],
        )
        .unwrap();
}
fn start(s: &State) -> Connection {
    t3_shared::start(s).unwrap().unwrap()
}
fn assert_ledger(db: &Connection) {
    assert_eq!(count(db, "scheduled_task_webhook_deliveries"), 3);
    assert_eq!(count(db, "scheduled_task_webhook_pending"), 1);
    assert_eq!(count(db, "scheduled_task_webhook_replay_keys"), 2);
    let request: Value = serde_json::from_str(&text(
        db,
        "SELECT request_json FROM scheduled_task_webhook_pending",
    ))
    .unwrap();
    assert_eq!(
        request["bodyText"],
        r#"{"id":"two","group":"batch","action":"second"}"#
    );
    let ids: Value = serde_json::from_str(&text(
        db,
        "SELECT delivery_ids_json FROM scheduled_task_webhook_pending",
    ))
    .unwrap();
    assert_eq!(ids.as_array().unwrap().len(), 2);
}

#[test]
fn official_task_rows_and_full_pending_ledger_survive_both_directions() {
    let (_dir, s, native) = profile();
    native.execute("INSERT INTO scheduled_task_webhook_relay_deliveries VALUES('relay:synthetic','signed','2026-11-06T00:00:00.000Z')",[]).unwrap();
    native.execute("UPDATE scheduled_tasks SET last_run_webhook_delivery_id='delivery:old' WHERE task_id='signed'",[]).unwrap();
    let mut shared = start(&s);
    assert_eq!(task(&s, "fresh")["threadMode"], "fresh_session");
    assert_eq!(task(&s, "continue")["threadMode"], "continue");
    assert_eq!(task(&s, "unsigned")["webhook"]["hasSecret"], false);
    assert!(!task(&s, "signed")
        .to_string()
        .contains("synthetic-store-key"));
    assert_ledger(&s.db.lock().unwrap());
    assert_eq!(
        count(
            &s.db.lock().unwrap(),
            "scheduled_task_webhook_relay_deliveries"
        ),
        1
    );
    rename(&s, "signed", "Edited by O3");
    assert_eq!(t3_shared::export(&s, &mut shared).unwrap(), 1);
    assert_eq!(
        text(
            &native,
            "SELECT title FROM scheduled_tasks WHERE task_id='signed'"
        ),
        "Edited by O3"
    );
    assert_eq!(
        text(
            &native,
            "SELECT webhook_secret FROM scheduled_tasks WHERE task_id='signed'"
        ),
        "synthetic-store-key"
    );
    assert_eq!(
        text(
            &native,
            "SELECT last_run_webhook_delivery_id FROM scheduled_tasks WHERE task_id='signed'"
        ),
        "delivery:old"
    );
    assert_ledger(&native);
    assert_eq!(count(&native, "orchestration_events"), 0);
    assert_eq!(t3_shared::export(&s, &mut shared).unwrap(), 0);
}

#[test]
fn native_task_edits_and_deletions_import_without_any_thread_events() {
    let (_dir, s, native) = profile();
    drop(start(&s));
    native.execute("UPDATE scheduled_tasks SET title='Native edit',thread_mode='fresh_session' WHERE task_id='continue'",[]).unwrap();
    native
        .execute(
            "DELETE FROM scheduled_task_webhook_deliveries WHERE task_id='signed'",
            [],
        )
        .unwrap();
    native
        .execute("DELETE FROM scheduled_tasks WHERE task_id='signed'", [])
        .unwrap();
    assert_eq!(count(&native, "orchestration_events"), 0);
    drop(start(&s));
    assert_eq!(task(&s, "continue")["title"], "Native edit");
    assert_eq!(task(&s, "continue")["threadMode"], "fresh_session");
    let db = s.db.lock().unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM records WHERE kind IN ('scheduledTasks','hookCredentials') AND id='signed'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(count(&db, "scheduled_task_webhook_pending"), 0);
    assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 0);
    assert_eq!(count(&db, "scheduled_task_webhook_deliveries"), 0);
}

#[test]
fn unsent_local_edits_and_deletions_survive_unchanged_native_state() {
    let (dir, s, native) = profile();
    drop(start(&s));
    rename(&s, "continue", "Unsent local title");
    scheduled::call(&s, "scheduledTasks.delete", &json!({"id":"signed"}))
        .unwrap()
        .unwrap();
    // Whitespace and property order in native JSON do not invent a competing task edit.
    native.execute(r#"UPDATE scheduled_tasks SET schedule_json='{ "everyMs":60000, "type":"interval" }' WHERE task_id='continue'"#,[]).unwrap();
    drop(s);
    let s = State::open(&dir).unwrap();
    let mut shared = start(&s);
    assert_eq!(task(&s, "continue")["title"], "Unsent local title");
    assert_eq!(
        text(
            &native,
            "SELECT title FROM scheduled_tasks WHERE task_id='continue'"
        ),
        "Unsent local title"
    );
    assert_eq!(count(&native, "scheduled_tasks"), 3);
    assert_eq!(count(&native, "scheduled_task_webhook_pending"), 0);
    assert_eq!(count(&native, "scheduled_task_webhook_deliveries"), 0);
    assert_eq!(t3_shared::export(&s, &mut shared).unwrap(), 0);
}

#[test]
fn conflicting_edits_keep_both_copies_and_the_original_baseline() {
    let (_dir, s, native) = profile();
    drop(start(&s));
    let baseline=text(&native,"SELECT row_hash FROM o3_task_baseline WHERE table_name='scheduled_tasks' AND row_key='[\"continue\"]'");
    rename(&s, "continue", "O3 pending");
    native
        .execute(
            "UPDATE scheduled_tasks SET title='T3 pending' WHERE task_id='continue'",
            [],
        )
        .unwrap();
    let error = t3_shared::start(&s).err().unwrap();
    assert!(error.contains("Shared task conflict"), "{error}");
    assert_eq!(task(&s, "continue")["title"], "O3 pending");
    assert_eq!(
        text(
            &native,
            "SELECT title FROM scheduled_tasks WHERE task_id='continue'"
        ),
        "T3 pending"
    );
    assert_eq!(text(&native,"SELECT row_hash FROM o3_task_baseline WHERE table_name='scheduled_tasks' AND row_key='[\"continue\"]'"),baseline);
    assert_ledger(&native);
    assert_ledger(&s.db.lock().unwrap());
}

#[test]
fn task_export_is_atomic_when_another_row_changed_outside_the_owner() {
    let (_dir, s, native) = profile();
    let mut shared = start(&s);
    rename(&s, "continue", "Local intent");
    native
        .execute(
            "UPDATE scheduled_task_webhook_pending SET state='dispatching'",
            [],
        )
        .unwrap();
    let error = t3_shared::export(&s, &mut shared).unwrap_err();
    assert!(error.contains("outside its profile owner"));
    assert_ne!(
        text(
            &native,
            "SELECT title FROM scheduled_tasks WHERE task_id='continue'"
        ),
        "Local intent"
    );
    assert_eq!(task(&s, "continue")["title"], "Local intent");
    assert_eq!(
        text(&native, "SELECT state FROM scheduled_task_webhook_pending"),
        "dispatching"
    );
}

#[test]
fn pending_windows_can_exchange_unique_coalescing_keys_on_import_and_export() {
    let (_dir, s, native) = profile();
    native.execute("INSERT INTO scheduled_task_webhook_pending SELECT 'delivery:second-window',task_id,'other-key',due_at+1,state,task_json,request_json,'[\"delivery:second-window\"]' FROM scheduled_task_webhook_pending",[]).unwrap();
    drop(start(&s));
    let swap = |db: &Connection| {
        let first=text(db,"SELECT coalesce_key FROM scheduled_task_webhook_pending WHERE delivery_id='delivery:synthetic-accepted'");
        let second=text(db,"SELECT coalesce_key FROM scheduled_task_webhook_pending WHERE delivery_id='delivery:second-window'");
        db.execute("UPDATE scheduled_task_webhook_pending SET coalesce_key='temporary-fixture-key' WHERE delivery_id='delivery:synthetic-accepted'",[]).unwrap();
        db.execute("UPDATE scheduled_task_webhook_pending SET coalesce_key=?1 WHERE delivery_id='delivery:second-window'",[first]).unwrap();
        db.execute("UPDATE scheduled_task_webhook_pending SET coalesce_key=?1 WHERE delivery_id='delivery:synthetic-accepted'",[second]).unwrap();
    };
    swap(&native);
    let mut shared = start(&s);
    assert_eq!(text(&s.db.lock().unwrap(),"SELECT coalesce_key FROM scheduled_task_webhook_pending WHERE delivery_id='delivery:synthetic-accepted'"),"other-key");
    swap(&s.db.lock().unwrap());
    t3_shared::export(&s, &mut shared).unwrap();
    assert_ne!(text(&native,"SELECT coalesce_key FROM scheduled_task_webhook_pending WHERE delivery_id='delivery:synthetic-accepted'"),"other-key");
    assert_eq!(count(&native, "scheduled_task_webhook_pending"), 2);
    assert_eq!(count(&native, "scheduled_task_webhook_replay_keys"), 2);
}

#[test]
fn a_recreated_local_database_recovers_unchanged_native_tasks_and_ledger() {
    let (_dir, s, native) = profile();
    drop(start(&s));
    {
        let db = s.db.lock().unwrap();
        db.execute("DELETE FROM records WHERE kind IN ('scheduledTasks','hookCredentials','scheduledTaskNative')",[]).unwrap();
        for table in [
            "scheduled_task_webhook_deliveries",
            "scheduled_task_webhook_pending",
            "scheduled_task_webhook_replay_keys",
        ] {
            db.execute(&format!("DELETE FROM {table}"), []).unwrap();
        }
        db.execute(
            "UPDATE o3_task_state SET identity='recreated-fixture-database',generation=0",
            [],
        )
        .unwrap();
    }
    drop(start(&s));
    assert_eq!(task(&s, "fresh")["threadMode"], "fresh_session");
    assert_ledger(&s.db.lock().unwrap());
    assert_ledger(&native);
}

#[test]
fn a_native_commit_failure_replays_already_committed_local_imports() {
    let (_dir, s, native) = profile();
    drop(start(&s));
    native
        .execute(
            "UPDATE scheduled_tasks SET title='Survives commit failure' WHERE task_id='continue'",
            [],
        )
        .unwrap();
    native.execute_batch("CREATE TABLE commit_parent(id TEXT PRIMARY KEY); CREATE TABLE commit_child(id TEXT REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_task_commit AFTER UPDATE ON o3_task_sync BEGIN INSERT INTO commit_child VALUES('missing'); END;").unwrap();
    let error = t3_shared::start(&s).err().unwrap();
    assert!(error.contains("FOREIGN KEY"), "{error}");
    // The local transaction committed first, while the native baseline transaction rolled back.
    assert_eq!(task(&s, "continue")["title"], "Survives commit failure");
    assert_eq!(count(&native, "commit_child"), 0);
    native
        .execute_batch("DROP TRIGGER fail_task_commit;")
        .unwrap();
    drop(start(&s));
    assert_eq!(task(&s, "continue")["title"], "Survives commit failure");
    assert_ledger(&native);
    assert_ledger(&s.db.lock().unwrap());
}

#[test]
fn malformed_native_task_schema_is_refused_before_sync_metadata() {
    let (_dir, s, native) = profile();
    native
        .execute_batch("ALTER TABLE scheduled_task_webhook_pending DROP COLUMN request_json;")
        .unwrap();
    let error = t3_shared::start(&s).err().unwrap();
    assert!(
        error.contains("scheduled_task_webhook_pending.request_json"),
        "{error}"
    );
    assert_eq!(native.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('o3_sync','o3_task_sync','o3_task_baseline')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM records WHERE kind='scheduledTasks'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn background_exporter_observes_task_only_notifications() {
    let (dir, s, native) = profile();
    let shared = start(&s);
    o3_server::events::mutate_project(&s,&json!({"type":"project.create","commandId":"sync-project","projectId":"p","title":"Initial export completed","workspaceRoot":dir})).unwrap();
    let exporter = t3_shared::Exporter::spawn(s.clone(), shared);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !native
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_projects WHERE project_id='p')",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "Initial history export did not complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let history_version = *s.changed.borrow();
    scheduled::call(
        &s,
        "scheduledTasks.setEnabled",
        &json!({"id":"continue","enabled":true}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        *s.changed.borrow(),
        history_version,
        "Fixture must wake only the task subscription"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while native
        .query_row(
            "SELECT enabled FROM scheduled_tasks WHERE task_id='continue'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        != 1
    {
        assert!(
            std::time::Instant::now() < deadline,
            "Task-only update was not exported"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    exporter.finish();
}

#[test]
fn native_request_bytes_and_header_order_survive_handover_and_order_only_edits() {
    let (_dir, s, native) = profile();
    let first = r#"{"method":"POST","path":"/api/hooks/signed/synthetic-token-signed","query":"","headers":{"x-event":"opened","content-type":"application/json"},"bodyText":"{\"action\":\"second\"}"}"#;
    let second = r#"{"method":"POST","path":"/api/hooks/signed/synthetic-token-signed","query":"","headers":{"content-type":"application/json","x-event":"opened"},"bodyText":"{\"action\":\"second\"}"}"#;
    native
        .execute(
            "UPDATE scheduled_task_webhook_pending SET request_json=?1",
            [first],
        )
        .unwrap();
    drop(start(&s));
    assert_eq!(
        text(
            &s.db.lock().unwrap(),
            "SELECT request_json FROM scheduled_task_webhook_pending"
        ),
        first
    );
    native
        .execute(
            "UPDATE scheduled_task_webhook_pending SET request_json=?1",
            [second],
        )
        .unwrap();
    let mut shared = start(&s);
    assert_eq!(
        text(
            &s.db.lock().unwrap(),
            "SELECT request_json FROM scheduled_task_webhook_pending"
        ),
        second
    );
    s.db.lock()
        .unwrap()
        .execute(
            "UPDATE scheduled_task_webhook_pending SET request_json=?1",
            [first],
        )
        .unwrap();
    t3_shared::export(&s, &mut shared).unwrap();
    assert_eq!(
        text(
            &native,
            "SELECT request_json FROM scheduled_task_webhook_pending"
        ),
        first
    );
    assert_eq!(count(&native, "scheduled_task_webhook_pending"), 1);
    assert_eq!(count(&native, "scheduled_task_webhook_replay_keys"), 2);
}
