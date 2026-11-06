//! Real RPC reads and token rotation over a synthetic official-runtime store capture.
mod support;
use o3_server::{store::State, t3_shared};
use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};

fn seed(state: &std::sync::Arc<State>) {
    let db = Connection::open(t3_shared::database(&state.dir)).unwrap();
    db.execute_batch(include_str!("fixtures/t3-statev2-80.sql"))
        .unwrap();
    let capture: Value =
        serde_json::from_str(include_str!("fixtures/t3-task-store-4262.json")).unwrap();
    for table in [
        "scheduled_tasks",
        "scheduled_task_webhook_deliveries",
        "scheduled_task_webhook_pending",
        "scheduled_task_webhook_replay_keys",
        "scheduled_task_webhook_relay_deliveries",
    ] {
        for row in capture["tables"][table].as_array().unwrap() {
            let fields = row.as_object().unwrap();
            let columns = fields.keys().cloned().collect::<Vec<_>>().join(",");
            let values = fields.values().map(|value| match value {
                Value::Null => rusqlite::types::Value::Null,
                Value::String(text) => rusqlite::types::Value::Text(text.clone()),
                Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                _ => panic!("Unexpected captured SQL field"),
            });
            db.execute(
                &format!(
                    "INSERT INTO {table} ({columns}) VALUES ({})",
                    vec!["?"; fields.len()].join(",")
                ),
                params_from_iter(values),
            )
            .unwrap();
        }
    }
    t3_shared::start(state).unwrap().unwrap();
}

#[test]
fn imported_receipts_follow_native_order_outcome_opt_in_and_pending_states_over_rpc() {
    let server = support::start(seed);
    let client = server.connect();
    let legacy = client
        .rpc(
            "scheduledTasks.listWebhookDeliveries",
            json!({"id":"signed"}),
        )
        .unwrap();
    assert_eq!(legacy["deliveries"].as_array().unwrap().len(), 1);
    let accepted = &legacy["deliveries"][0];
    assert_eq!(accepted["id"], "delivery:synthetic-accepted");
    assert_eq!(accepted["pendingWindow"], true);
    assert_eq!(accepted["contentType"], "application/json");
    assert_eq!(accepted["bodyBytes"], 46);
    assert!(accepted.get("body").is_none());
    let any = client
        .rpc(
            "scheduledTasks.listWebhookDeliveries",
            json!({"id":"signed","acceptAnyOutcome":true}),
        )
        .unwrap();
    assert_eq!(
        any["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["outcome"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["accepted", "coalesced", "duplicate"]
    );
    for summary in any["deliveries"].as_array().unwrap() {
        let full = client
            .rpc(
                "scheduledTasks.getWebhookDelivery",
                json!({"id":"signed","deliveryId":summary["id"],"acceptAnyOutcome":true}),
            )
            .unwrap();
        for (key, value) in summary.as_object().unwrap() {
            assert_eq!(&full["delivery"][key], value, "{key}");
        }
        assert_eq!(
            full["delivery"]["headers"]["x-hub-signature-256"],
            "[redacted]"
        );
        assert_eq!(full["delivery"]["bodyTruncated"], false);
        assert!(!full.to_string().contains("synthetic-store-key"));
        assert!(full["delivery"].get("request_json").is_none());
    }
    for (state, pending) in [
        ("pending", true),
        ("dispatching", true),
        ("running", false),
        ("completed", false),
        ("failed", false),
    ] {
        server
            .state
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE scheduled_task_webhook_pending SET state=?1",
                [state],
            )
            .unwrap();
        let full = client
            .rpc(
                "scheduledTasks.getWebhookDelivery",
                json!({"id":"signed","deliveryId":"delivery:synthetic-accepted"}),
            )
            .unwrap();
        assert_eq!(full["delivery"]["pendingWindow"], pending, "{state}");
    }
    assert_eq!(
        client
            .rpc(
                "scheduledTasks.listWebhookDeliveries",
                json!({"id":"absent"})
            )
            .unwrap(),
        json!({"deliveries":[]})
    );
    for input in [
        json!({"id":"signed","deliveryId":"absent"}),
        json!({"id":"unsigned","deliveryId":"delivery:synthetic-accepted"}),
        json!({"id":"signed","deliveryId":"delivery:synthetic-coalesced"}),
    ] {
        let error = client
            .rpc("scheduledTasks.getWebhookDelivery", input)
            .unwrap_err();
        assert!(error.contains("Webhook delivery not found."), "{error}");
    }
    let reader = server.limited(&client, &["orchestration:read"]);
    for method in [
        "scheduledTasks.listWebhookDeliveries",
        "scheduledTasks.getWebhookDelivery",
        "scheduledTasks.rotateWebhookToken",
    ] {
        let error = reader
            .rpc(
                method,
                json!({"id":"signed","deliveryId":"delivery:synthetic-accepted"}),
            )
            .unwrap_err();
        assert!(error.contains("orchestration:operate"), "{error}");
    }
    let operator = server.limited(&client, &["orchestration:operate"]);
    assert!(operator
        .rpc(
            "scheduledTasks.listWebhookDeliveries",
            json!({"id":"signed"})
        )
        .is_ok());
}

#[test]
fn unknown_outcomes_and_invalid_logged_json_follow_native_fallbacks() {
    let server = support::start(seed);
    let client = server.connect();
    server.state.db.lock().unwrap().execute("UPDATE scheduled_task_webhook_deliveries SET outcome='future-outcome',headers_json=?1,missing_fields_json=?2,signature_verified=2,body_truncated=2 WHERE delivery_id='delivery:synthetic-accepted'",params![r#"{"content-type":"application/json","invalid":42}"#,r#"["valid",42]"#]).unwrap();
    assert_eq!(
        client
            .rpc(
                "scheduledTasks.listWebhookDeliveries",
                json!({"id":"signed"})
            )
            .unwrap(),
        json!({"deliveries":[]})
    );
    let full=client.rpc("scheduledTasks.getWebhookDelivery",json!({"id":"signed","deliveryId":"delivery:synthetic-accepted","acceptAnyOutcome":true})).unwrap();
    let full = &full["delivery"];
    assert_eq!(full["outcome"], "future-outcome");
    assert!(full["contentType"].is_null());
    assert_eq!(full["headers"], json!({}));
    assert_eq!(full["missingFields"], json!([]));
    assert_eq!(full["signatureVerified"], false);
    assert_eq!(full["bodyTruncated"], false);
    assert_eq!(full["pendingWindow"], false);
    // Legacy callers still see every outcome in the native compatibility set.
    for outcome in [
        "accepted",
        "dispatch_failed",
        "rejected_signature",
        "disabled",
        "rate_limited",
        "expired",
    ] {
        server.state.db.lock().unwrap().execute("UPDATE scheduled_task_webhook_deliveries SET outcome=?1 WHERE delivery_id='delivery:synthetic-accepted'",[outcome]).unwrap();
        let list = client
            .rpc(
                "scheduledTasks.listWebhookDeliveries",
                json!({"id":"signed"}),
            )
            .unwrap();
        assert_eq!(list["deliveries"].as_array().unwrap().len(), 1, "{outcome}");
    }
}

fn body(db: &Connection, kind: &str, id: &str) -> Value {
    let text: String = db
        .query_row(
            "SELECT body FROM records WHERE kind=?1 AND id=?2",
            params![kind, id],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn native_decoders_supply_receipt_projections_and_legacy_outcome_cases() {
    let server = support::start(seed);
    let client = server.connect();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/webhook-history.json")).unwrap();
    assert_eq!(
        fixture["revision"],
        "c55db3e9c72fdf584f4b54192fb39beb88667b59"
    );
    assert_eq!(fixture["effect"], "4.1.2");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 79);
    for case in fixture["cases"].as_array().unwrap() {
        let row = &case["row"];
        {
            let db = server.state.db.lock().unwrap();
            db.execute("UPDATE scheduled_task_webhook_deliveries SET outcome=?1,headers_json=?2,missing_fields_json=?3,signature_verified=?4,body_truncated=?5,error=?6,rendered_prompt=?7 WHERE delivery_id='delivery:synthetic-accepted'",params![row["outcome"].as_str().unwrap(),row["headers_json"].as_str().unwrap(),row["missing_fields_json"].as_str().unwrap(),row["signature_verified"].as_i64().unwrap(),row["body_truncated"].as_i64().unwrap(),row["error"].as_str(),row["rendered_prompt"].as_str()]).unwrap();
            db.execute(
                "UPDATE scheduled_task_webhook_pending SET state=?1",
                [case["state"].as_str().unwrap_or("absent")],
            )
            .unwrap();
        }
        for accept in [false, true] {
            let list = client
                .rpc(
                    "scheduledTasks.listWebhookDeliveries",
                    json!({"id":"signed","acceptAnyOutcome":accept}),
                )
                .unwrap();
            let summary = list["deliveries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == "delivery:synthetic-accepted");
            let detail = client.rpc("scheduledTasks.getWebhookDelivery",json!({"id":"signed","deliveryId":"delivery:synthetic-accepted","acceptAnyOutcome":accept}));
            if accept || case["legacy"] == true {
                assert_eq!(summary, Some(&case["summary"]), "{}", case["label"]);
                assert_eq!(detail.unwrap(), case["detail"], "{}", case["label"]);
            } else {
                assert!(summary.is_none(), "{}", case["label"]);
                assert!(
                    detail.unwrap_err().contains("Webhook delivery not found."),
                    "{}",
                    case["label"]
                );
            }
        }
    }
}

#[test]
fn rotation_preserves_secret_and_work_rejects_old_url_and_rolls_back_failed_write() {
    let server = support::start(seed);
    let client = server.connect();
    let timeout = std::time::Duration::from_secs(3);
    let mut subscription =
        support::subscribe(server.connect(), "scheduledTasks.subscribe", json!({}));
    assert_eq!(
        subscription.next(timeout)["tasks"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    for id in ["signed", "unsigned"] {
        let (prior, credentials) = {
            let db = server.state.db.lock().unwrap();
            (
                body(&db, "scheduledTasks", id),
                body(&db, "hookCredentials", id),
            )
        };
        let reply = client
            .rpc("scheduledTasks.rotateWebhookToken", json!({"id":id}))
            .unwrap();
        let token = {
            let db = server.state.db.lock().unwrap();
            let stored = body(&db, "hookCredentials", id);
            assert_eq!(stored["secret"], credentials["secret"]);
            stored["token"].as_str().unwrap().to_owned()
        };
        assert_eq!(token.len(), 43);
        assert!(token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(token, credentials["token"]);
        let mut expected = prior.clone();
        expected["updatedAt"] = reply["task"]["updatedAt"].clone();
        expected["webhook"] = json!({"path":format!("/api/hooks/{id}/{token}"),"url":null,"hasSecret":!credentials["secret"].is_null()});
        assert_eq!(reply, json!({"task":expected}));
        subscription.until(timeout, |snapshot| {
            snapshot["tasks"].as_array().is_some_and(|tasks| {
                tasks
                    .iter()
                    .any(|task| task["id"] == id && task["webhook"] == reply["task"]["webhook"])
            })
        });
        // The retired bearer URL must be refused before signature/body handling.
        let error = client
            .request_json(
                "POST",
                prior["webhook"]["path"].as_str().unwrap(),
                Some(&json!({})),
                false,
            )
            .unwrap_err();
        assert!(error.contains("404"), "{error}");
        assert!(!reply.to_string().contains("synthetic-store-key"));
    }
    for (id, message) in [
        ("continue", "Only webhook tasks have a URL token."),
        ("absent", "Schedule task not found."),
    ] {
        let error = client
            .rpc("scheduledTasks.rotateWebhookToken", json!({"id":id}))
            .unwrap_err();
        assert!(error.contains(message), "{error}");
    }
    let before = {
        let db = server.state.db.lock().unwrap();
        (
            body(&db, "scheduledTasks", "signed"),
            body(&db, "hookCredentials", "signed"),
        )
    };
    server.state.db.lock().unwrap().execute_batch("CREATE TRIGGER refuse_rotated_task BEFORE UPDATE ON records WHEN NEW.kind='scheduledTasks' AND NEW.id='signed' BEGIN SELECT RAISE(ABORT,'synthetic task write refusal'); END;").unwrap();
    let error = client
        .rpc("scheduledTasks.rotateWebhookToken", json!({"id":"signed"}))
        .unwrap_err();
    assert!(error.contains("Could not rotate webhook token."), "{error}");
    let db = server.state.db.lock().unwrap();
    assert_eq!(body(&db, "scheduledTasks", "signed"), before.0);
    assert_eq!(body(&db, "hookCredentials", "signed"), before.1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scheduled_task_webhook_pending",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scheduled_task_webhook_replay_keys",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    db.execute_batch("DROP TRIGGER refuse_rotated_task;")
        .unwrap();
    db.execute(
        "UPDATE records SET body='[]' WHERE kind='hookCredentials' AND id='signed'",
        [],
    )
    .unwrap();
    drop(db);
    let error = client
        .rpc("scheduledTasks.rotateWebhookToken", json!({"id":"signed"}))
        .unwrap_err();
    assert!(error.contains("Could not rotate webhook token."), "{error}");
    let db = server.state.db.lock().unwrap();
    assert_eq!(body(&db, "hookCredentials", "signed"), json!([]));
    assert_eq!(body(&db, "scheduledTasks", "signed"), before.0);
}
