//! Shared harness: a real o3-server on a temporary profile, driven by the real `t3-protocol` client.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use t3_protocol::{pair_bearer, Connection, ConnectionOptions, PairingClient};

pub struct Server {
    pub base: String,
    pub secret: String,
    pub state: Arc<o3_server::store::State>,
    pub dir: std::path::PathBuf,
}

/// Starts a server on an ephemeral port. `configure` runs before it serves (install a provider, a shell...).
pub fn start(configure: impl FnOnce(&Arc<o3_server::store::State>)) -> Server {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "o3-it-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let state = Arc::new(o3_server::store::State::open(&dir).unwrap());
    let secret = state.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    configure(&state);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = state.clone();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .thread_stack_size(512 * 1024)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                o3_server::serve(
                    tokio::net::TcpListener::from_std(listener).unwrap(),
                    serving,
                )
                .await
                .unwrap();
            })
    });
    Server {
        base,
        secret,
        state,
        dir,
    }
}

pub fn pair(base: &str, secret: &str) -> t3_protocol::Result<Connection> {
    pair_bearer(
        base,
        secret,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )?
    .connection(&ConnectionOptions::default())
}

impl Server {
    /// A new owner session; every call spends a fresh one-use pairing secret.
    pub fn connect(&self) -> Connection {
        let secret = self.state.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
            .as_str()
            .unwrap()
            .to_owned();
        pair(&self.base, &secret).unwrap()
    }
    /// A second session limited to `scopes`, minted through the owner's pairing endpoint.
    pub fn limited(&self, owner: &Connection, scopes: &[&str]) -> Connection {
        let grant = owner
            .request_json(
                "POST",
                "/api/auth/pairing-token",
                Some(&json!({"label":"limited","scopes":scopes})),
                false,
            )
            .unwrap();
        pair(&self.base, grant["credential"].as_str().unwrap()).unwrap()
    }
}

/// A subscription read on its own thread; items arrive in order through `recv`.
pub struct Stream {
    rx: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
    done: Arc<std::sync::Mutex<Option<String>>>,
    pub seen: Vec<Value>,
}

pub fn subscribe(conn: Connection, tag: &str, payload: Value) -> Stream {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(std::sync::Mutex::new(None));
    let (s, d, tag) = (stop.clone(), done.clone(), tag.to_owned());
    std::thread::spawn(move || {
        let result = conn.subscribe(
            &tag,
            payload,
            || s.load(Ordering::Relaxed),
            |item| {
                if let Some(item) = item {
                    let _ = tx.send(item);
                }
                Ok(())
            },
        );
        *d.lock().unwrap() = Some(match result {
            Ok(()) => "cancelled".into(),
            Err(e) => e.to_string(),
        });
    });
    Stream {
        rx,
        stop,
        done,
        seen: Vec::new(),
    }
}

impl Stream {
    /// Next item, or panics after `within`.
    pub fn next(&mut self, within: Duration) -> Value {
        let item = self
            .rx
            .recv_timeout(within)
            .unwrap_or_else(|_| panic!("no stream item within {within:?}; seen {:?}", self.seen));
        self.seen.push(item.clone());
        item
    }
    pub fn try_next(&mut self, within: Duration) -> Option<Value> {
        let item = self.rx.recv_timeout(within).ok()?;
        self.seen.push(item.clone());
        Some(item)
    }
    /// Reads items until `done(item)` holds; returns everything read, the matching one last.
    pub fn until(&mut self, within: Duration, mut done: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let end = Instant::now() + within;
        let mut out = Vec::new();
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let item = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("condition not met within {within:?}; read {out:?}"));
            self.seen.push(item.clone());
            let hit = done(&item);
            out.push(item);
            if hit {
                return out;
            }
        }
    }
    pub fn ended(&self) -> Option<String> {
        self.done.lock().unwrap().clone()
    }
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn wait_for(within: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

/// One RPC over a raw websocket, returning the whole `Exit` frame (the client library reports only error tags).
pub fn raw_exit(server: &Server, tag: &str, payload: Value) -> Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let secret = server.state.issue_pairing(Some("raw"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = pair_bearer(
        &server.base,
        &secret,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )
    .unwrap()
    .access_token
    .to_string();
    let addr = server.base.trim_start_matches("http://").to_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let mut request = format!("ws://{addr}/ws?orchestrationProtocol=2")
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("Authorization", format!("Bearer {token}").parse().unwrap());
            let (mut ws, _) = tokio_tungstenite::client_async(request, tcp).await.unwrap();
            ws.send(Message::Text(
                json!({"_tag":"Request","id":"r","tag":tag,"payload":payload,"headers":[]})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            loop {
                let m = tokio::time::timeout(Duration::from_secs(10), ws.next())
                    .await
                    .expect("exit frame")
                    .unwrap()
                    .unwrap();
                let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
                if v["_tag"] == "Exit" {
                    return v;
                }
            }
        })
}
