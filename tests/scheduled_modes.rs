//! Real-client scheduling admission. Run inside tools/test-scheduled-modes.
mod support;
use serde_json::{json, Value};
use std::sync::Arc;
use support::{start, Server};
use t3_protocol::Connection;

fn setup() -> (Server, Connection) {
    assert!(
        std::path::Path::new("/run/.containerenv").is_file()
            || std::path::Path::new("/.dockerenv").is_file()
    );
    let server = start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider))
    });
    let client = server.connect();
    let root = server.dir.join("workspace");
    std::fs::create_dir(&root).unwrap();
    client.rpc("projects.mutate", json!({"type":"project.create","commandId":"project","projectId":"p","title":"Tasks","workspaceRoot":root})).unwrap();
    (server, client)
}
fn input(id: &str, mode: &str) -> Value {
    json!({"id":id,"title":"Task","prompt":"Task prompt","enabled":false,"schedule":{"type":"interval","everyMs":60000},"projectId":"p","threadMode":mode,"workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"agent","creationSource":"server"})
}
fn save(c: &Connection, input: Value) -> Value {
    c.rpc("scheduledTasks.upsert", input).unwrap()["task"].clone()
}
fn fire(c: &Connection, id: &str) -> Value {
    let task = c.rpc("scheduledTasks.runNow", json!({"id":id})).unwrap()["task"].clone();
    assert_eq!(task["lastRunStatus"], "succeeded", "{task}");
    task
}
fn projection(c: &Connection, thread: &Value) -> Value {
    c.rpc(
        "orchestration.getThreadProjection",
        json!({"threadId":thread}),
    )
    .unwrap()
}

#[test]
fn continuing_and_fresh_tasks_bind_once_queue_and_preserve_message_provenance() {
    let (server, c) = setup();
    save(&c, input("continue", "continue"));
    let first = fire(&c, "continue");
    assert!(first["threadId"].is_string());
    let second = fire(&c, "continue");
    assert_eq!(second["threadId"], first["threadId"]);
    let mut edit = input("continue", "fresh_session");
    edit["modelSelection"] = json!({"instanceId":"unused","model":"not-used-for-bound-runs"});
    let edited = save(&c, edit.clone());
    assert_eq!(edited["threadId"], first["threadId"]);
    let third = fire(&c, "continue");
    assert_eq!(third["runCount"], 3);
    let p = projection(&c, &third["threadId"]);
    let latest = p["runs"].as_array().unwrap().last().unwrap();
    assert_eq!(latest["status"], "queued");
    assert_eq!(latest["freshContext"], true);
    assert_eq!(latest["providerInstanceId"], "scripted");
    let messages: Vec<_> = p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .collect();
    assert_eq!(messages.len(), 3);
    for message in messages {
        assert_eq!(message["scheduledTaskId"], "continue");
        assert_eq!(message["createdBy"], "agent");
        assert_eq!(message["creationSource"], "server");
    }
    let items: Vec<_> = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "user_message")
        .collect();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i["scheduledTaskId"] == "continue"));
    assert!(items.iter().any(|i| i["freshContext"] == true));
    assert_eq!(p["thread"]["createdBy"], "agent");
    assert_eq!(p["thread"]["creationSource"], "server");
    // A legacy/stale form omits both mode and binding; it cannot undo the first-run binding.
    edit.as_object_mut().unwrap().remove("threadMode");
    edit["threadId"] = Value::Null;
    let stale = save(&c, edit);
    assert_eq!(stale["threadMode"], "fresh_session");
    assert_eq!(stale["threadId"], first["threadId"]);
    assert_eq!(
        o3_server::settings::descriptor(&server.state)["capabilities"]["scheduledTaskThreadModes"],
        true
    );
}

#[test]
fn new_thread_runs_never_bind_and_switching_modes_unbinds() {
    let (server, c) = setup();
    save(&c, input("task", "continue"));
    let first = fire(&c, "task");
    let mut change = input("task", "new_thread");
    change["threadId"] = first["threadId"].clone();
    assert!(save(&c, change)["threadId"].is_null());
    assert!(fire(&c, "task")["threadId"].is_null());
    assert!(fire(&c, "task")["threadId"].is_null());
    let count: i64 = server
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE kind='thread'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn moving_projects_releases_old_binding_and_legacy_rebinding_selects_continue() {
    let (server, c) = setup();
    let root = server.dir.join("new-project");
    std::fs::create_dir(&root).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"new-project","projectId":"new-project","title":"New project","workspaceRoot":root})).unwrap();
    save(&c, input("task", "continue"));
    let first = fire(&c, "task");
    let mut change = input("task", "continue");
    change["projectId"] = json!("new-project");
    assert!(save(&c, change)["threadId"].is_null());
    let mut legacy = input("legacy", "new_thread");
    legacy.as_object_mut().unwrap().remove("threadMode");
    legacy["threadId"] = first["threadId"].clone();
    let legacy = save(&c, legacy);
    assert_eq!(legacy["threadMode"], "continue");
    assert_eq!(legacy["threadId"], first["threadId"]);
}

#[test]
fn invalid_modes_are_atomic_and_archived_targets_report_failure() {
    let (_server, c) = setup();
    save(&c, input("task", "continue"));
    let first = fire(&c, "task");
    let mut invalid = input("task", "invalid");
    assert!(c.rpc("scheduledTasks.upsert", invalid.clone()).is_err());
    invalid["threadMode"] = Value::Null;
    assert!(c.rpc("scheduledTasks.upsert", invalid).is_err());
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.archive","commandId":"archive","threadId":first["threadId"]}),
    )
    .unwrap();
    let failed = c
        .rpc("scheduledTasks.runNow", json!({"id":"task"}))
        .unwrap()["task"]
        .clone();
    assert_eq!(failed["lastRunStatus"], "failed");
    assert!(failed["lastRunError"]
        .as_str()
        .unwrap()
        .contains("archived"));
    assert_eq!(failed["threadMode"], "continue");
    assert_eq!(
        projection(&c, &first["threadId"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for _ in 0..2 {
        let error = c
            .rpc("scheduledTasks.runNow", json!({"id":"missing"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Schedule task not found"), "{error}");
    }
}
