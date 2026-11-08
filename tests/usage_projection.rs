//! Actual native readAccounts observations through the real server-config RPC stream.
mod support;
#[path = "support/sandbox.rs"]
mod test_sandbox;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    time::Duration,
};

#[test]
fn native_accounts_reach_config_snapshots_without_management_credentials() {
    test_sandbox::assert_isolated();
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/cliproxy-projection-4288.json")).unwrap();
    let mut selected = fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| {
            matches!(
                case["label"].as_str(),
                Some("plan:free" | "scoped" | "provider-failure" | "credits")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.push(
        fixtures["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["account"]["id_token"]["chatgpt_account_id"] == " workspace ")
            .unwrap()
            .clone(),
    );
    selected.push(
        fixtures["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["usage"]["spend"]["used"]["currency"] == " USD ")
            .unwrap()
            .clone(),
    );
    for case in selected {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let expected_calls = case["calls"].as_array().unwrap().clone();
        let input = case.clone();
        let hub = std::thread::spawn(move || {
            for expected in expected_calls {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    head.extend(byte);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(head.len() < 64 * 1024);
                }
                let head = String::from_utf8(head).unwrap();
                let url = url::Url::parse(expected["url"].as_str().unwrap()).unwrap();
                assert!(head.starts_with(&format!(
                    "{} {} HTTP/1.1\r\n",
                    expected["method"].as_str().unwrap(),
                    url.path()
                )));
                assert!(head
                    .to_ascii_lowercase()
                    .contains("authorization: bearer private-projection-key\r\n"));
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length < 1024 * 1024);
                let mut bytes = vec![0; length];
                socket.read_exact(&mut bytes).unwrap();
                let body: Value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                if let Some(expected) = expected.get("body") {
                    assert_eq!(&body, expected);
                } else {
                    assert!(body.is_null());
                }
                let response = if url.path().ends_with("/auth-files") {
                    json!({"files":[input["account"]]})
                } else if body["url"]
                    .as_str()
                    .unwrap()
                    .ends_with("/rate-limit-reset-credits")
                {
                    json!({"status_code":input["creditStatus"].as_u64().unwrap_or(200),"body":json!({"credits":input.get("credits").cloned().unwrap_or(json!([]))}).to_string()})
                } else {
                    json!({"status_code":input["usageStatus"].as_u64().unwrap_or(200),"body":input["usage"].to_string()})
                };
                let response = response.to_string();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            }
        });
        let server = support::start(|_| {});
        let owner = server.connect();
        owner.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"hub":{"kind":"cliproxy","url":format!("{base}/panel/management.html"),"managementKey":"private-projection-key"}}}})).unwrap();
        let mut stream = support::subscribe(
            server.connect(),
            "server.subscribeServerConfig",
            json!({"usageLimitSources":true}),
        );
        let snapshot = stream.next(Duration::from_secs(3));
        assert_eq!(snapshot["type"], "snapshot");
        let events = stream.until(Duration::from_secs(5), |event| {
            event["type"] == "usageLimitSourcesUpdated"
        });
        let event = events.last().unwrap();
        let actual = &event["payload"]["sources"][0]["accounts"][0];
        let mut expected = case["result"].clone();
        let checked = actual["usageLimits"]["checkedAt"].as_str().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(checked).is_ok());
        expected["usageLimits"]["checkedAt"] = json!(checked);
        assert_eq!(actual, &expected, "{}", case["label"]);
        assert!(!event.to_string().contains("private-projection-key"));
        assert!(!snapshot.to_string().contains("private-projection-key"));
        let mut reduced = json!({});
        t3_protocol::quota_sources::apply(&mut reduced, &snapshot);
        assert!(t3_protocol::quota_sources::apply(&mut reduced, event));
        assert_eq!(
            reduced["usageLimitSources"][0]["accounts"][0]["driver"],
            expected["driver"]
        );
        stream.cancel();
        hub.join().unwrap();
    }
}
