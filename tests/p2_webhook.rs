mod support;
use serde_json::json;
use t3_protocol::api::scheduled_tasks::{self, TaskUpsert};
#[test]
fn signed_webhook_definition_is_readable_without_leaking_secret() {
    let s = support::start(|_| {});
    let c = s.connect();
    let p = json!({"id":"hook","title":"Hook","prompt":"inspect {{body.action}}","enabled":true,"schedule":{"type":"webhook","signature":{"header":"x-hub-signature-256","encoding":"hex","prefix":"sha256=","secret":"fixture-only-key"},"maxDeliveryAgeMinutes":5,"filters":[{"path":"body.action","op":"equals","value":"opened"}],"dedupe":{"replayKey":"{{body.id}}","coalesceKey":"{{body.group}}","coalesceWindowSeconds":60}},"projectId":"p","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"});
    let task = TaskUpsert::from_json(&p).unwrap();
    let reply = scheduled_tasks::upsert(&c, &task).unwrap();
    assert_eq!(reply["task"]["webhook"]["hasSecret"], true);
    assert!(reply["task"]["webhook"]["path"]
        .as_str()
        .unwrap()
        .starts_with("/api/hooks/hook/"));
    assert!(!reply.to_string().contains("fixture-only-key"));
    let path = reply["task"]["webhook"]["path"].as_str().unwrap();
    let body = r#"{"id":"one","group":"batch","action":"opened"}"#;
    assert!(post(&s.base, path, body, "bad").contains("401"));
    assert!(post(&s.base, path, &" ".repeat(1024 * 1024 + 1), "bad").contains("413"));
    let signature = sign(body);
    assert!(post(&s.base, path, body, &signature).contains("accepted"));
    assert!(post(&s.base, path, body, &signature).contains("duplicate"));
    let filtered = r#"{"id":"two","group":"batch","action":"closed"}"#;
    assert!(post(&s.base, path, filtered, &sign(filtered)).contains("filtered"));
    let second = r#"{"id":"two","group":"batch","action":"opened"}"#;
    assert!(post(&s.base, path, second, &sign(second)).contains("accepted"));
    let db = s.state.db.lock().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM hook_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM hook_replay", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    // Expired coalesced requests must never launch, even if the clock has passed due.
    db.execute("UPDATE hook_queue SET due=0,expires=0", [])
        .unwrap();
    drop(db);
    scheduled_tasks::set_enabled(&c, "hook", true).unwrap();
    assert!(support::wait_for(std::time::Duration::from_secs(2), || s
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM hook_queue", [], |r| r
            .get::<_, i64>(0))
        .unwrap()
        == 0));
    let list = scheduled_tasks::list(&c).unwrap();
    assert!(!list.to_string().contains("fixture-only-key"));
    assert_eq!(list["tasks"][0]["runCount"], 0);
    let mut unsigned = p.clone();
    unsigned["schedule"]["signature"] = serde_json::Value::Null;
    assert!(scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&unsigned).unwrap()).is_err());
}

fn sign(body: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut m = Hmac::<sha2::Sha256>::new_from_slice(b"fixture-only-key").unwrap();
    m.update(body.as_bytes());
    format!(
        "sha256={}",
        m.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn post(base: &str, path: &str, body: &str, signature: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    write!(stream,"POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\nX-Hub-Signature-256: {signature}\r\nConnection: close\r\n\r\n{body}",body.len()).ok();
    let mut result = String::new();
    let _ = stream.read_to_string(&mut result);
    result
}

#[test]
fn unsigned_template_and_filter_inputs_are_rejected() {
    let s = support::start(|_| {});
    let c = s.connect();
    for (prompt, filters, dedupe) in [
        ("{{query.text}}", json!([]), json!({})),
        ("safe", json!([{"path":"headers.x-event","op":"equals","value":"go"}]), json!({})),
        ("safe", json!([]), json!({"replayKey":"{{headers.x-delivery}}"})),
        ("safe", json!([]), json!({"coalesceKey":"{{query.group}}"})),
    ] {
        let p = json!({"id":"unsafe","title":"Hook","prompt":prompt,"enabled":true,"schedule":{"type":"webhook","signature":{"header":"X-Hub-Signature-256","encoding":"hex","prefix":"sha256=","secret":"fixture-only-key"},"filters":filters,"dedupe":dedupe},"projectId":"p","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"});
        assert!(scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&p).unwrap()).is_err(), "unsigned input accepted: {p}");
    }
}
