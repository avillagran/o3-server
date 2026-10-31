//! Conformance: the real client library against a real o3-server.
use serde_json::json;
use t3_protocol::{fetch_descriptor, pair_bearer, Connection, ConnectionOptions, Credential, PairingClient};

/// `O3_CONFORMANCE_BASE` + `O3_CONFORMANCE_PAIRING` aim this suite at any server, such as
/// a real T3 headless: the same assertions must hold on both.
fn target() -> (String, String) {
    match std::env::var("O3_CONFORMANCE_BASE") {
        Ok(base) => (base, std::env::var("O3_CONFORMANCE_PAIRING").expect("O3_CONFORMANCE_PAIRING")),
        Err(_) => spawn_server(),
    }
}

fn spawn_server() -> (String, String) {
    let dir = std::env::temp_dir().join(format!("o3-conformance-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state = o3_server::store::State::open(&dir).unwrap();
    let pairing = state.issue_pairing(Some("test"), 60_000).unwrap()["credential"].as_str().unwrap().to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            o3_server::serve(tokio::net::TcpListener::from_std(listener).unwrap(), std::rc::Rc::new(state)).await.unwrap();
        })
    });
    (base, pairing)
}

fn pair(base: &str, secret: &str) -> t3_protocol::Result<Connection> {
    let paired = pair_bearer(base, secret, None, &PairingClient::default(), &ConnectionOptions::default())?;
    paired.connection(&ConnectionOptions::default())
}

#[test]
fn pairing_is_one_use_and_session_authenticates_rpc() {
    let (base, secret) = target();
    assert_eq!(fetch_descriptor(&base).unwrap()["orchestrationProtocolVersion"], 2);
    let conn = pair(&base, &secret).unwrap();
    assert!(conn.rpc("server.nope", json!({})).is_err());
    assert!(pair(&base, &secret).is_err(), "a pairing secret must not work twice");
}

#[test]
fn rpc_requires_a_valid_credential() {
    let (base, _) = target();
    let conn = Connection::new(&base, None, Credential::bearer("forged-token").unwrap()).unwrap();
    let error = conn.rpc("server.nope", json!({})).unwrap_err();
    assert!(error.to_string().contains("rpc_connection_failed") || error.to_string().contains("unauthorized"), "{error}");
}

#[test]
fn shell_subscription_opens_with_a_snapshot() {
    let (base, secret) = target();
    let conn = pair(&base, &secret).unwrap();
    let done = std::cell::Cell::new(false);
    let mut first = None;
    conn.subscribe("orchestration.subscribeShell", json!({}), || done.get(), |item| {
        if let Some(item) = item {
            first.get_or_insert(item);
            done.set(true);
        }
        Ok(())
    }).unwrap();
    let first = first.expect("snapshot");
    assert_eq!(first["kind"], "snapshot");
    assert!(first["snapshot"]["projects"].is_array());
}
