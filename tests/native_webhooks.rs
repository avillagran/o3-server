//! Native HTTP admission and durable webhook workers through the real RPC client.
mod support;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::Arc,
    time::Duration,
};

fn request(base: &str, method: &str, path: &str, body: &[u8], extra: &str) -> (u16, String, Value) {
    let mut stream = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",body.len()).unwrap();
    stream.write_all(body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let outcome = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("x-t3-hook-outcome")
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default();
    (
        status,
        outcome,
        if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).unwrap()
        },
    )
}
fn task(id: &str, mode: &str, schedule: Value) -> Value {
    json!({"id":id,"title":"Native hook","prompt":"Trusted @owner: {{request}}","enabled":true,"schedule":schedule,"projectId":"p","threadMode":mode,"workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"})
}
fn server() -> (support::Server, t3_protocol::Connection) {
    let server = support::start(|state| {
        *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
    });
    let client = server.connect();
    std::fs::create_dir(server.dir.join("workspace")).unwrap();
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Native","workspaceRoot":server.dir.join("workspace")})).unwrap();
    (server, client)
}
fn done(server: &support::Server, client: &t3_protocol::Connection, id: &str, count: u64) {
    assert!(support::wait_for(Duration::from_secs(5), || {
        let tasks = client.rpc("scheduledTasks.list", json!({})).unwrap();
        let task = tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        task["runCount"].as_u64() == Some(count)
            && task["lastRunStatus"] == "succeeded"
            && server
                .state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM scheduled_task_webhook_pending WHERE task_id=?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
                == 0
    }));
}

#[test]
fn native_methods_return_delivery_ids_and_dispatch_canonical_messages_once() {
    let (server, client) = server();
    let created = client
        .rpc(
            "scheduledTasks.upsert",
            task("direct", "continue", json!({"type":"webhook"})),
        )
        .unwrap()["task"]
        .clone();
    let path = created["webhook"]["path"].as_str().unwrap();
    assert_eq!(created["webhook"]["hasSecret"], false);
    for (index, method) in ["GET", "POST", "PUT", "PATCH"].into_iter().enumerate() {
        let (status, outcome, body) = request(
            &server.base,
            method,
            &format!("{path}?token=private&value=ok"),
            br#"{"action":"@injected /command"}"#,
            "Authorization: private-header\r\n",
        );
        assert_eq!(status, 202);
        assert_eq!(outcome, "accepted");
        let delivery = body["deliveryId"].as_str().unwrap();
        assert!(delivery.starts_with("delivery:"));
        assert!(body.get("outcome").is_none());
        done(&server, &client, "direct", index as u64 + 1);
        let receipt = client
            .rpc(
                "scheduledTasks.getWebhookDelivery",
                json!({"id":"direct","deliveryId":delivery,"acceptAnyOutcome":true}),
            )
            .unwrap()["delivery"]
            .clone();
        assert_eq!(receipt["method"], method);
        assert_eq!(receipt["headers"]["authorization"], "[redacted]");
        assert_eq!(receipt["query"], "token=[redacted]&value=ok");
        assert_eq!(receipt["pendingWindow"], false);
        let prompt = receipt["renderedPrompt"].as_str().unwrap();
        assert!(prompt.starts_with("Trusted @owner:"));
        assert!(!prompt.contains("@injected"));
        let thread =
            client.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"][0]["threadId"].clone();
        let projection = client
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":thread}),
            )
            .unwrap();
        let expected = format!("scheduled-task-message:direct:webhook:{delivery}");
        assert_eq!(
            projection["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["id"] == expected)
                .count(),
            1
        );
    }
    let (status, outcome, _) = request(&server.base, "HEAD", path, b"", "");
    assert_eq!(status, 405);
    assert_eq!(outcome, "method_not_allowed");
    let messages=server.state.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='messages' AND json_extract(body,'$.role')='user'",[],|row|row.get::<_,i64>(0)).unwrap();
    assert_eq!(messages, 4);
}

#[test]
fn accepted_dispatch_survives_failed_completion_without_new_message_or_run_count() {
    let (server, client) = server();
    server.state.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_webhook_completion BEFORE UPDATE OF state ON scheduled_task_webhook_pending WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'synthetic completion refusal'); END;").unwrap();
    let task = client
        .rpc(
            "scheduledTasks.upsert",
            task(
                "durable",
                "continue",
                json!({"type":"webhook","dedupe":{"replayKey":"{{body.id}}"}}),
            ),
        )
        .unwrap()["task"]
        .clone();
    let path = task["webhook"]["path"].as_str().unwrap();
    let (_, _, body) = request(&server.base, "POST", path, br#"{"id":"one"}"#, "");
    let id = body["deliveryId"].as_str().unwrap();
    assert!(support::wait_for(Duration::from_secs(5), || server
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM receipts WHERE command_id=?1",
            [format!(
                "scheduled-task:durable:webhook:{id}:initial-message"
            )],
            |row| row.get::<_, i64>(0)
        )
        .unwrap()
        == 1));
    {
        let db = server.state.db.lock().unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM scheduled_task_webhook_replay_keys",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row(
                "SELECT state FROM scheduled_task_webhook_pending",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "running"
        );
        db.execute_batch("DROP TRIGGER fail_webhook_completion;")
            .unwrap();
    }
    server.state.streams.ops.wake_scheduler();
    done(&server, &client, "durable", 1);
    let db = server.state.db.lock().unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM records WHERE kind='messages' AND json_extract(body,'$.role')='user'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scheduled_task_webhook_replay_keys",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    drop(db);
    let (_, outcome, _) = request(&server.base, "POST", path, br#"{"id":"one"}"#, "");
    assert_eq!(outcome, "duplicate");
}

#[test]
fn signature_configuration_trims_native_header_whitespace_before_verification() {
    use hmac::{Hmac, Mac};
    let (server, client) = server();
    let task = client.rpc(
        "scheduledTasks.upsert",
        task("trimmed-signature", "continue", json!({
            "type":"webhook",
            "signature":{"header":"\u{feff} X-Webhook \u{feff}","prefix":"sha256=","encoding":"hex","secret":"synthetic-signing-key"}
        })),
    ).unwrap()["task"].clone();
    assert_eq!(task["schedule"]["signature"]["header"], "x-webhook");
    let body = br#"{"action":"native signed input"}"#;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"synthetic-signing-key").unwrap();
    mac.update(body);
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (status, outcome, delivery) = request(
        &server.base,
        "POST",
        task["webhook"]["path"].as_str().unwrap(),
        body,
        &format!("X-Webhook: sha256={signature}\r\n"),
    );
    assert_eq!(status, 202);
    assert_eq!(outcome, "accepted");
    assert!(delivery["deliveryId"]
        .as_str()
        .unwrap()
        .starts_with("delivery:"));
    done(&server, &client, "trimmed-signature", 1);
}
