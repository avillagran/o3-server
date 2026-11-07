use o3_server::{device_native_ssh as plan, device_native_ssh_transport::Cli};
use serde_json::Value;
use std::sync::Arc;

use o3_server::device_native_ssh::Config;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
pub struct Fixture {
    pub cli: Arc<Cli>,
    pub healthy: Arc<AtomicBool>,
    server: tokio::task::JoinHandle<()>,
    pub trace: PathBuf,
    pub executable: PathBuf,
    pub directory: PathBuf,
    settings_path: PathBuf,
    pub devices: Arc<std::sync::Mutex<Value>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub async fn make(fail_forwards: u8) -> Self {
        crate::test_sandbox::assert_isolated();
        use http_body_util::Full;
        use hyper::{service::service_fn, Response};
        use hyper_util::rt::TokioIo;
        let base = std::env::temp_dir().join(format!(
            "private-ssh-{}",
            o3_server::store::random_id().unwrap()
        ));
        std::fs::create_dir(&base).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let devices = Arc::new(std::sync::Mutex::new(
            serde_json::json!({"simulators":[],"emulators":[]}),
        ));
        let devices_for_server = devices.clone();
        let healthy = Arc::new(AtomicBool::new(true));
        let ready = healthy.clone();
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let healthy = ready.clone();
                let devices = devices_for_server.clone();
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            let healthy = healthy.clone();
                            let devices = devices.clone();
                            async move {
                                let status = if request.uri().path() == "/readyz"
                                    && !healthy.load(Ordering::Acquire)
                                {
                                    503
                                } else {
                                    200
                                };
                                let (body, content_type) = if request.uri().path() == "/api/devices"
                                {
                                    (
                                        serde_json::to_vec(&*devices.lock().unwrap()).unwrap(),
                                        "application/json",
                                    )
                                } else if request.uri().path().ends_with("/screenshot") {
                                    (
                                        b"\x89PNG\r\n\x1a\nprivate-ssh-fixture".to_vec(),
                                        "image/png",
                                    )
                                } else if request.method() == hyper::Method::POST {
                                    (br#"{"ok":true}"#.to_vec(), "application/json")
                                } else {
                                    (b"ready".to_vec(), "text/plain")
                                };
                                Ok::<_, std::convert::Infallible>(
                                    Response::builder()
                                        .status(status)
                                        .header("content-type", content_type)
                                        .header("set-cookie", "private-upstream=fixture")
                                        .body(Full::new(hyper::body::Bytes::from(body)))
                                        .unwrap(),
                                )
                            }
                        });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        // Never create/write a runnable file while parallel tests spawn children.
        // The checked-in executable is mounted read-only; its per-fixture
        // settings travel through the existing explicit SSH identity argv slot.
        let executable = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/device/ssh-private-cli.py");
        let settings_path = base.join("key with ' quote.json");
        let trace = base.join("trace.jsonl");
        std::fs::write(&settings_path,serde_json::to_vec(&serde_json::json!({"remotePort":port,"receipt":"c".repeat(48),"trace":trace,"counter":base.join("counter"),"failForwards":fail_forwards})).unwrap()).unwrap();
        let config=Config::decode(&serde_json::json!({"id":"mini","label":"Private SSH","target":"user@private.invalid","port":2222,"identityFile":settings_path})).unwrap();
        let owner = plan::owner(Some("private-environment"), "/private/state", &config.id);
        let cli = Arc::new(Cli::new(executable.clone(), base.clone(), config, owner).unwrap());
        Self {
            cli,
            healthy,
            server,
            trace,
            executable,
            directory: base,
            settings_path,
            devices,
        }
    }
    pub fn trace(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.trace)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn settings(&self, patch: Value) {
        let path = &self.settings_path;
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for (key, data) in patch.as_object().unwrap() {
            value[key] = data.clone();
        }
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
}
