//! Reset credits through the actual scoped RPC transport and a private synthetic hub.
mod support;
#[path = "support/sandbox.rs"]
mod test_sandbox;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[test]
fn scoped_rpc_redeems_only_the_named_hub_account_and_preserves_cooldown_warning() {
    test_sandbox::assert_isolated();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let hub = std::thread::spawn(move || {
        for (index, socket) in listener.incoming().take(4).enumerate() {
            let mut socket = socket.unwrap();
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
            assert!(head
                .to_ascii_lowercase()
                .contains("authorization: bearer private-rpc-management\r\n"));
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length < 1024 * 1024);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            let body: Value = if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            };
            let (status, response) = match index {
                0 => {
                    assert!(head.starts_with("GET /v0/management/auth-files HTTP/1.1\r\n"));
                    (
                        200,
                        json!({"files":[{"id":"account","auth_index":"auth-index","provider":"codex","id_token":{"chatgpt_account_id":"workspace"}}]}),
                    )
                }
                1 => {
                    assert!(head.starts_with("POST /v0/management/api-call HTTP/1.1\r\n"));
                    assert_eq!(
                        body["url"],
                        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume"
                    );
                    assert_eq!(
                        serde_json::from_str::<Value>(body["data"].as_str().unwrap()).unwrap(),
                        json!({"redeem_request_id":"32841ada-7baf-55b8-9cf8-00c30ab64f8d","credit_id":"credit"})
                    );
                    (
                        200,
                        json!({"status_code":200,"body":"{\"code\":\"reset\"}"}),
                    )
                }
                2 => {
                    assert!(head.starts_with("POST /v0/management/reset-quota HTTP/1.1\r\n"));
                    assert_eq!(body, json!({"auth_index":"auth-index"}));
                    (503, json!({"private":"private-rpc-management"}))
                }
                _ => {
                    assert!(head.starts_with("GET /v0/management/auth-files HTTP/1.1\r\n"));
                    (200, json!({"files":[]}))
                }
            };
            let response = response.to_string();
            write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    let server = support::start(|_| {});
    let owner = server.connect();
    owner.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"hub":{"kind":"cliproxy","url":format!("{base}/management.html"),"managementKey":"private-rpc-management"}}}})).unwrap();
    let payload = json!({"sourceId":"hub","accountId":" account ","creditId":" credit "});
    let reader = server.limited(&owner, &["orchestration:read"]);
    assert!(reader
        .rpc("provider.consumeResetCredit", payload.clone())
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(owner
        .rpc(
            "provider.consumeResetCredit",
            json!({"sourceId":"hub","accountId":"","creditId":"credit"})
        )
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let result = owner.rpc("provider.consumeResetCredit", payload).unwrap();
    assert_eq!(
        result,
        json!({"outcome":"reset","warning":"Credit redeemed, but the hub cooldown could not be cleared. Routing may resume after its cooldown expires."})
    );
    assert!(!result.to_string().contains("private-rpc-management"));
    hub.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    for payload in [
        json!({"sourceId":"missing","accountId":"account","creditId":"credit"}),
        json!({"instanceId":"missing"}),
        json!({"instanceId":"scripted"}),
    ] {
        assert!(owner.rpc("provider.consumeResetCredit", payload).is_err());
    }
}
