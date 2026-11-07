mod support;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn native_case(label: &str) -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/composer-context.json")).unwrap();
    fixture["projections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["label"] == label)
        .unwrap()
        .clone()
}
fn setup(instance: &str) -> (support::Server, t3_protocol::Connection) {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/input/composer.py");
    let server = support::start(|state| {
        *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                binary.clone(),
            ))),
            codex: Arc::new(o3_server::codex::CodexProvider::new(binary, None)),
        }));
    });
    let client = server.connect();
    client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Context","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    (server, client)
}
fn projection(client: &t3_protocol::Connection) -> Value {
    client
        .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap()
}
fn wait(client: &t3_protocol::Connection, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = projection(client);
        assert!(
            !value["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["status"] == "failed"),
            "{value}"
        );
        if done(&value) {
            return value;
        }
        assert!(Instant::now() < deadline, "{value}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn context(case: &Value) -> Value {
    let mut records = case["input"]["records"].as_array().unwrap().clone();
    // Forward-compatible decoding drops this malformed known record, including its payload.
    records.push(json!({"version":1,"kind":"terminal","contextId":"malformed","label":"bad","payload":"cannot impersonate a valid terminal"}));
    json!({"version":1,"records":records})
}
fn dispatch(
    client: &t3_protocol::Connection,
    id: &str,
    text: &str,
    context: Option<Value>,
    mode: Value,
) {
    let mut request = json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"t","text":text,"attachments":[],"dispatchMode":mode});
    if let Some(context) = context {
        request["context"] = context;
    }
    client
        .rpc("orchestration.dispatchCommand", request)
        .unwrap();
}
#[test]
fn native_context_reaches_both_transports_and_stored_text_stays_portable() {
    for instance in ["codex", "claudeAgent"] {
        for label in ["kind:terminal", "kind:future-kind", "missing"] {
            let (_server, client) = setup(instance);
            let case = native_case(label);
            let text = case["input"]["text"].as_str().unwrap();
            dispatch(
                &client,
                "send",
                text,
                Some(context(&case)),
                json!({"type":"start_immediately"}),
            );
            let value = wait(&client, |value| value["runs"][0]["status"] == "completed");
            let message = value["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["id"] == "send")
                .unwrap();
            assert_eq!(message["text"], text);
            assert_eq!(
                message["context"]["records"].as_array().unwrap().len(),
                case["input"]["records"].as_array().unwrap().len()
            );
            let item = value["turnItems"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["messageId"] == "send")
                .unwrap();
            assert_eq!(item["text"], text);
            assert_eq!(item["context"], message["context"]);
            assert!(value["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["text"] == "verified-composer"));
        }
    }
}
#[test]
fn active_steering_and_queued_edit_promotion_use_native_projection() {
    for instance in ["codex", "claudeAgent"] {
        for queued in [false, true] {
            let (_server, client) = setup(instance);
            dispatch(
                &client,
                "hold",
                "hold",
                None,
                json!({"type":"start_immediately"}),
            );
            let value = wait(&client, |value| {
                value["runs"][0]["status"] == "running"
                    && !value["providerTurns"].as_array().unwrap().is_empty()
            });
            let active = value["runs"][0]["id"].clone();
            let case = native_case("kind:terminal");
            let text = case["input"]["text"].as_str().unwrap();
            if queued {
                dispatch(
                    &client,
                    "queued",
                    "before edit",
                    None,
                    json!({"type":"queue_after_active"}),
                );
                let value = projection(&client);
                let run = value["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|run| run["status"] == "queued")
                    .unwrap();
                client.rpc("orchestration.dispatchCommand",json!({"type":"queued-run.edit","commandId":"edit","threadId":"t","runId":run["id"],"text":text,"context":context(&case)})).unwrap();
                client.rpc("orchestration.dispatchCommand",json!({"type":"queued-message.promote-to-steer","commandId":"promote","threadId":"t","queuedRunId":run["id"],"targetRunId":active})).unwrap();
            } else {
                dispatch(
                    &client,
                    "steer",
                    text,
                    Some(context(&case)),
                    json!({"type":"steer_active","targetRunId":active}),
                );
            }
            let value = wait(&client, |value| {
                value["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|run| run["id"] == active)
                    .unwrap()["status"]
                    == "completed"
            });
            let item = value["turnItems"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["messageId"] == if queued { "queued" } else { "steer" })
                .unwrap();
            assert_eq!(item["text"], text);
            assert_eq!(item["context"]["records"].as_array().unwrap().len(), 1);
            assert_eq!(item["inputIntent"], "steer");
        }
    }
}
#[test]
fn duplicate_native_ids_reject_before_message_receipt_or_run_mutation() {
    let (server, client) = setup("codex");
    let case = native_case("kind:terminal");
    let record = case["input"]["records"][0].clone();
    let result=client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"reject","messageId":"reject","threadId":"t","text":case["input"]["text"],"context":{"version":1,"records":[record,record]},"attachments":[],"dispatchMode":{"type":"start_immediately"}}));
    assert!(result.is_err());
    let value = projection(&client);
    assert!(value["messages"].as_array().unwrap().is_empty());
    assert!(value["runs"].as_array().unwrap().is_empty());
    assert_eq!(
        server
            .state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM receipts WHERE command_id='reject'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn malformed_initial_context_is_rejected_before_launch_claims_or_workspace_preparation() {
    let (server, client) = setup("codex");
    let root = server.dir.join("workspace");
    std::fs::create_dir(&root).unwrap();
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Context","workspaceRoot":root})).unwrap();
    let case = native_case("kind:terminal");
    let record = case["input"]["records"][0].clone();
    let result=client.rpc("orchestration.launchThread",json!({"commandId":"bad-launch","threadId":"new","projectId":"p","title":"Context","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"messageId":"bad-message","text":case["input"]["text"],"attachments":[],"context":{"version":1,"records":[record,record]}}}));
    assert!(result.is_err());
    let db = server.state.db.lock().unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM records WHERE thread_id='new'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM launch_claims WHERE command_id='bad-launch'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
