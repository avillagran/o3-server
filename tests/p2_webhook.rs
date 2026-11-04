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
    s.state
        .db
        .lock()
        .unwrap()
        .execute("UPDATE hook_replay SET expires=0", [])
        .unwrap();
    assert!(post(
        &s.base,
        &format!("{path}?text=attacker&delivery=changed"),
        body,
        &signature
    )
    .contains("duplicate"));
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
    // Valid-sized queued prompts can exhaust bytes well before the 500-job cap.
    let db = s.state.db.lock().unwrap();
    for n in 0..130 {
        db.execute(
            "INSERT INTO hook_queue VALUES(?,'hook','',?,?,?)",
            rusqlite::params![
                format!("budget-{n}"),
                o3_server::store::now_ms() + 3_600_000,
                o3_server::store::now_ms() + 7_200_000,
                "x".repeat(64 * 1024)
            ],
        )
        .unwrap();
    }
    drop(db);
    let overflow = r#"{"id":"overflow","action":"opened"}"#;
    assert!(
        post(&s.base, path, overflow, &sign(overflow)).contains("429"),
        "logical queue byte budget was ignored"
    );
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
        (
            "safe",
            json!([{"path":"headers.x-event","op":"equals","value":"go"}]),
            json!({}),
        ),
        (
            "safe",
            json!([]),
            json!({"replayKey":"{{headers.x-delivery}}"}),
        ),
        ("safe", json!([]), json!({"coalesceKey":"{{query.group}}"})),
    ] {
        let p = json!({"id":"unsafe","title":"Hook","prompt":prompt,"enabled":true,"schedule":{"type":"webhook","signature":{"header":"X-Hub-Signature-256","encoding":"hex","prefix":"sha256=","secret":"fixture-only-key"},"filters":filters,"dedupe":dedupe},"projectId":"p","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"});
        assert!(
            scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&p).unwrap()).is_err(),
            "unsigned input accepted: {p}"
        );
    }
}

#[test]
fn unauthenticated_traffic_cannot_lock_out_or_enumerate_hooks() {
    let s = support::start(|_| {});
    let c = s.connect();
    let p = json!({"id":"guarded","title":"Hook","prompt":"safe {{body.action}}","enabled":true,"schedule":{"type":"webhook","signature":{"header":"x-hub-signature-256","encoding":"hex","prefix":"sha256=","secret":"fixture-only-key"}},"projectId":"missing-project","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"});
    let reply = scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&p).unwrap()).unwrap();
    let path = reply["task"]["webhook"]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let wrong_token = format!("{}/{}", path.rsplit_once('/').unwrap().0, "not-the-token");
    let status = |reply: &str| reply.lines().next().unwrap_or("").to_owned();
    let body_of = |reply: &str| reply.rsplit("\r\n\r\n").next().unwrap_or("").to_owned();
    // An unknown hook and a known hook with a wrong token must be indistinguishable.
    let unknown = post(&s.base, "/api/hooks/missing/token", "{}", "bad");
    let wrong = post(&s.base, &wrong_token, "{}", "bad");
    assert_eq!(status(&unknown), status(&wrong));
    assert_eq!(body_of(&unknown), body_of(&wrong));
    assert!(status(&unknown).contains("404"), "{unknown}");
    // A burst of unauthenticated traffic, held writer included, never costs the real sender.
    let writer = s.state.db.lock().unwrap();
    let started = std::time::Instant::now();
    for _ in 0..80 {
        post(&s.base, "/api/hooks/missing/token", "", "bad");
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    drop(writer);
    let body = r#"{"action":"go"}"#;
    let delivered = post(&s.base, &path, body, &sign(body));
    assert!(delivered.contains("accepted"), "{delivered}");
}

#[test]
fn failed_dispatch_keeps_accepted_work_durable() {
    let s = support::start(|_| {});
    let c = s.connect();
    let p = json!({"id":"retry","title":"Hook","prompt":"safe {{body.action}}","enabled":true,"schedule":{"type":"webhook","signature":{"header":"x-hub-signature-256","encoding":"hex","prefix":"sha256=","secret":"fixture-only-key"}},"projectId":"missing-project","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"});
    let reply = scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&p).unwrap()).unwrap();
    let body = r#"{"action":"go"}"#;
    assert!(post(
        &s.base,
        reply["task"]["webhook"]["path"].as_str().unwrap(),
        body,
        &sign(body)
    )
    .contains("accepted"));
    assert!(support::wait_for(std::time::Duration::from_secs(2), || {
        scheduled_tasks::list(&c).unwrap()["tasks"][0]["runCount"]
            .as_u64()
            .unwrap_or(0)
            > 0
    }));
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM hook_queue", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1,
        "failed dispatch lost accepted job"
    );
    // runCount commits before the failed claim is requeued. Wait for that final
    // transition so the worker cannot overwrite our simulated restart with its backoff.
    assert!(support::wait_for(std::time::Duration::from_secs(2), || {
        let due: i64 = s
            .state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT due FROM hook_queue", [], |r| r.get(0))
            .unwrap();
        due != i64::MAX && due > o3_server::store::now_ms()
    }));
    // Simulate a crash after the dispatch receipt commit but before queue deletion.
    let db = s.state.db.lock().unwrap();
    let job: String = db
        .query_row("SELECT id FROM hook_queue", [], |r| r.get(0))
        .unwrap();
    db.execute(
        "INSERT INTO receipts VALUES(?,'{}')",
        [format!("webhook:retry:{job}")],
    )
    .unwrap();
    db.execute("UPDATE hook_queue SET due=0", []).unwrap();
    drop(db);
    scheduled_tasks::set_enabled(&c, "retry", true).unwrap();
    assert!(support::wait_for(std::time::Duration::from_secs(2), || s
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM hook_queue", [], |r| r
            .get::<_, i64>(0))
        .unwrap()
        == 0));
    assert_eq!(
        scheduled_tasks::list(&c).unwrap()["tasks"][0]["runCount"],
        1
    );
}
