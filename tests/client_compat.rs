//! Conformance: the real client library against a real o3-server.
use serde_json::json;
use t3_protocol::{fetch_descriptor, Connection, Credential};

/// `O3_CONFORMANCE_BASE` + `O3_CONFORMANCE_TOKEN` aim this suite at any server, such as
/// a real T3 headless: the same assertions must hold on both.
fn target() -> (String, String) {
    match std::env::var("O3_CONFORMANCE_BASE") {
        Ok(base) => (base, std::env::var("O3_CONFORMANCE_TOKEN").expect("O3_CONFORMANCE_TOKEN")),
        Err(_) => (spawn_server(), "synthetic-token".into()),
    }
}

fn spawn_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            o3_server::serve(tokio::net::TcpListener::from_std(listener).unwrap()).await.unwrap();
        })
    });
    base
}

#[test]
fn descriptor_and_unary_rpc() {
    let (base, token) = target();
    assert_eq!(fetch_descriptor(&base).unwrap()["orchestrationProtocolVersion"], 2);
    let conn = Connection::new(&base, None, Credential::bearer(&token).unwrap()).unwrap();
    assert!(conn.rpc("server.getVersion", json!({})).unwrap()["version"].is_string());
    assert!(conn.rpc("server.nope", json!({})).is_err());
}
