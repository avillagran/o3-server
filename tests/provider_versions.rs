mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
struct Installed;
impl Provider for Installed {
    fn config(&self) -> Vec<Value> {
        vec![
            json!({"instanceId":"codex","driver":"codex","installed":true,"version":"codex-cli 1.2.3"}),
        ]
    }
    fn execute<'a>(
        &'a self,
        _: RunInput,
        _: tokio::sync::mpsc::Sender<ProviderEvent>,
        _: tokio::sync::mpsc::Receiver<ProviderControl>,
        _: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
#[test]
fn version_checks_reach_real_provider_reader_and_respect_disable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // One test in this executable; set before server threads start.
    std::env::set_var(
        "O3_PROVIDER_VERSION_REGISTRY",
        format!("http://{}", listener.local_addr().unwrap()),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let hits = count.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0; 2048];
            let _ = stream.read(&mut request);
            let n = hits.fetch_add(1, Ordering::SeqCst);
            let body = if n == 0 {
                r#"{"version":"1.3.0"}"#
            } else if n == 1 {
                r#"{"version":"1.2.3"}"#
            } else {
                "invalid"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(Arc::new(Installed)));
    let c = s.connect();
    for (status, latest) in [
        ("behind_latest", json!("1.3.0")),
        ("current", json!("1.2.3")),
        ("unknown", Value::Null),
    ] {
        c.rpc("server.refreshProviders", json!({})).unwrap();
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        let view = t3_protocol::provider_instances::projection(&config["settings"], &config, true);
        let instance = view["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == "codex")
            .unwrap();
        assert_eq!(instance["versionStatus"], status);
        assert_eq!(instance["latestVersion"], latest);
        assert_eq!(instance["currentVersion"], "codex-cli 1.2.3");
    }
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let patch = t3_protocol::native_settings::patch(
        &json!({"key":"enableProviderUpdateChecks","value":false}),
        Some(&config),
    )
    .unwrap();
    c.rpc("server.updateSettings", patch).unwrap();
    c.rpc("server.refreshProviders", json!({})).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 3);
}
