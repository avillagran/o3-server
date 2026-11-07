//! Private single-client loopback CDP relay. URLs and payloads have no Debug/Serialize surface.
use super::{Channel, TabKey};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    collections::VecDeque,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::Notify,
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{Callback, ErrorResponse, Request, Response},
        protocol::WebSocketConfig,
        Message,
    },
};

pub(super) struct Inbound {
    state: Mutex<QueueState>,
    changed: Notify,
}
struct Admission {
    path: String,
    connected: Arc<AtomicBool>,
}
impl Callback for Admission {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        if request.uri().path() != self.path || self.connected.swap(true, Ordering::AcqRel) {
            let mut rejected = tokio_tungstenite::tungstenite::http::Response::new(Some(
                "Unavailable desktop endpoint".to_owned(),
            ));
            *rejected.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(rejected)
        } else {
            Ok(response)
        }
    }
}
struct QueueState {
    messages: VecDeque<String>,
    closed: bool,
}
impl Inbound {
    pub(super) fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.messages.clear();
        drop(state);
        self.changed.notify_waiters();
    }
    pub(super) fn offer(&self, message: String) {
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            state.messages.push_back(message);
            drop(state);
            self.changed.notify_one();
        }
    }
    async fn next(&self) -> Option<String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                if state.closed {
                    return None;
                }
                if let Some(message) = state.messages.pop_front() {
                    return Some(message);
                }
            }
            changed.await;
        }
    }
}

pub struct Endpoint {
    url: String,
    key: TabKey,
    inbound: Arc<Inbound>,
    channel: Channel,
    task: JoinHandle<()>,
    released: bool,
}
impl Endpoint {
    pub fn url(&self) -> &str {
        &self.url
    }
    fn cleanup(&self) {
        self.task.abort();
        self.inbound.shutdown();
        let mut state = self.channel.state.lock().unwrap();
        if state
            .inbound
            .get(&self.key.id())
            .is_some_and(|slot| Arc::ptr_eq(slot, &self.inbound))
        {
            state.inbound.remove(&self.key.id());
        }
    }
    /// Await release before shutting down the parent channel/runtime.
    pub async fn close(mut self) {
        self.cleanup();
        self.released = true;
        let channel = self.channel.clone();
        let message =
            json!({"type":"release","threadId":self.key.thread_id,"tabId":self.key.tab_id});
        // Finalization must survive cancellation of the caller awaiting close.
        let _ = tokio::spawn(async move {
            channel.command(message).await;
        })
        .await;
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.cleanup();
        let channel = self.channel.clone();
        let message =
            json!({"type":"release","threadId":self.key.thread_id,"tabId":self.key.tab_id});
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                channel.command(message).await;
            });
        }
    }
}

impl Channel {
    pub async fn endpoint(&self, key: TabKey) -> io::Result<Endpoint> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let mut secret = [0; 24];
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        let path = format!("/{}", URL_SAFE_NO_PAD.encode(secret));
        let inbound = Arc::new(Inbound {
            state: Mutex::new(QueueState {
                messages: VecDeque::new(),
                closed: false,
            }),
            changed: Notify::new(),
        });
        {
            let mut state = self.state.lock().unwrap();
            state.inbound.insert(key.id(), inbound.clone());
            if !state.kernel.is_attached(&key) {
                state.inbound.remove(&key.id());
                return Err(io::Error::other(
                    "The desktop tab detached before the server connected.",
                ));
            }
        }
        let url = format!("ws://127.0.0.1:{port}{path}");
        let channel = self.clone();
        let thread_id = key.thread_id.clone();
        let tab_id = key.tab_id.clone();
        let queue = inbound.clone();
        let task = tokio::spawn(async move {
            let connected = Arc::new(AtomicBool::new(false));
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream,_)) = accepted else { break; };
                        let path = path.clone();
                        let connected = connected.clone();
                        let channel = channel.clone();
                        let thread_id = thread_id.clone();
                        let tab_id = tab_id.clone();
                        let queue = queue.clone();
                        // The native endpoint permits one client ever. Complete
                        // this handshake here so exactly that client owns the queue.
                        let config = WebSocketConfig::default().max_message_size(Some(100*1024*1024)).max_frame_size(Some(100*1024*1024));
                        clients.spawn(async move {
                        let handshake = accept_hdr_async_with_config(stream,Admission {path,connected},Some(config));
                        if let Ok(Ok(socket)) = tokio::time::timeout(Duration::from_secs(5),handshake).await {
                            relay(socket,queue,channel,thread_id,tab_id).await;
                        }
                        });
                    }
                    Some(_) = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Ok(Endpoint {
            url,
            key,
            inbound,
            channel: self.clone(),
            task,
            released: false,
        })
    }
}

async fn relay(
    mut socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    queue: Arc<Inbound>,
    channel: Channel,
    thread_id: String,
    tab_id: String,
) {
    let mut outbound = true;
    loop {
        tokio::select! {
            incoming = socket.next() => {
                let message = match incoming {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => {
                        let text = String::from_utf8_lossy(&bytes);
                        text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
                    }
                    Some(Ok(Message::Ping(bytes))) => { if socket.send(Message::Pong(bytes)).await.is_err() { return; } continue; }
                    Some(Ok(Message::Pong(_))) => continue,
                    _ => return,
                };
                channel.command(json!({"type":"cdp","threadId":thread_id,"tabId":tab_id,"message":message})).await;
            }
            message = queue.next(), if outbound => {
                match message {
                    Some(message) => if socket.send(Message::Text(message.into())).await.is_err() { return; },
                    None => { outbound = false; }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
    fn key() -> TabKey {
        TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        }
    }
    async fn setup() -> (Channel, DuplexStream, BufReader<DuplexStream>) {
        let (mut launcher, input) = tokio::io::duplex(4096);
        let (control, commands) = tokio::io::duplex(4096);
        let channel = Channel::start(input, control);
        event(
            &mut launcher,
            json!({"type":"attached","threadId":"thread","tabId":"tab"}),
        )
        .await;
        assert!(channel.await_attached(&key(), Duration::from_secs(1)).await);
        (channel, launcher, BufReader::new(commands))
    }
    async fn event(launcher: &mut DuplexStream, event: Value) {
        launcher
            .write_all(format!("{event}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn command(commands: &mut BufReader<DuplexStream>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(1), commands.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    async fn connect(
        endpoint: &Endpoint,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        tokio_tungstenite::tungstenite::Error,
    > {
        let url = url::Url::parse(endpoint.url()).unwrap();
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
            .await
            .unwrap();
        tokio_tungstenite::client_async(endpoint.url(), stream)
            .await
            .map(|(socket, _)| socket)
    }
    #[tokio::test]
    async fn real_rust_relay_matches_native_roundtrip_and_releases() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/desktop-browser-4288.json"
        ))
        .unwrap();
        let expected = &fixture["endpointCases"][0]["expected"];
        let (channel, mut launcher, mut commands) = setup().await;
        let endpoint = channel.endpoint(key()).await.unwrap();
        let url = url::Url::parse(endpoint.url()).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(URL_SAFE_NO_PAD.decode(&url.path()[1..]).unwrap().len(), 24);
        let mut wrong = url.clone();
        wrong.set_path("/wrong-secret");
        let unauthenticated = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap()))
            .await
            .unwrap();
        assert!(
            matches!(tokio_tungstenite::client_async(wrong.as_str(),unauthenticated).await,Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status()==403)
        );
        let mut socket = connect(&endpoint).await.unwrap();
        event(
            &mut launcher,
            json!({"type":"cdp","threadId":"thread","tabId":"tab","message":"native opaque 😀"}),
        )
        .await;
        let received = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            received.to_text().unwrap(),
            expected["received"].as_str().unwrap()
        );
        socket
            .send(Message::Text("client opaque 😀".into()))
            .await
            .unwrap();
        let outbound = command(&mut commands).await;
        let native: Value =
            serde_json::from_str(expected["writes"][0]["line"].as_str().unwrap()).unwrap();
        assert_eq!(outbound, native);
        assert!(
            matches!(connect(&endpoint).await,Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status()==403)
        );
        socket.close(None).await.unwrap();
        drop(socket);
        assert!(
            matches!(connect(&endpoint).await,Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status()==403)
        );
        endpoint.close().await;
        let release: Value =
            serde_json::from_str(expected["writes"][1]["line"].as_str().unwrap()).unwrap();
        assert_eq!(command(&mut commands).await, release);
    }
    #[tokio::test]
    async fn detach_keeps_inbound_commands_and_old_release_preserves_new_queue() {
        let (channel, mut launcher, mut commands) = setup().await;
        let old = channel.endpoint(key()).await.unwrap();
        let mut old_socket = connect(&old).await.unwrap();
        event(
            &mut launcher,
            json!({"type":"detached","threadId":"thread","tabId":"tab"}),
        )
        .await;
        event(
            &mut launcher,
            json!({"type":"attached","threadId":"sentinel","tabId":"detached"}),
        )
        .await;
        assert!(
            channel
                .await_attached(
                    &TabKey {
                        thread_id: "sentinel".into(),
                        tab_id: "detached".into()
                    },
                    Duration::from_secs(1)
                )
                .await
        );
        old_socket
            .send(Message::Text("after detach".into()))
            .await
            .unwrap();
        assert_eq!(command(&mut commands).await["message"], "after detach");
        event(
            &mut launcher,
            json!({"type":"attached","threadId":"thread","tabId":"tab"}),
        )
        .await;
        assert!(channel.await_attached(&key(), Duration::from_secs(1)).await);
        let fresh = channel.endpoint(key()).await.unwrap();
        let mut fresh_socket = connect(&fresh).await.unwrap();
        old.close().await;
        assert_eq!(command(&mut commands).await["type"], "release");
        event(&mut launcher,json!({"type":"cdp","threadId":"thread","tabId":"tab","message":"fresh queue retained"})).await;
        let received = tokio::time::timeout(Duration::from_secs(1), fresh_socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(received.to_text().unwrap(), "fresh queue retained");
        fresh.close().await;
        assert_eq!(command(&mut commands).await["type"], "release");
    }

    #[tokio::test]
    async fn cancelled_close_still_completes_exactly_one_release() {
        let (channel, _launcher, mut commands) = setup().await;
        let endpoint = channel.endpoint(key()).await.unwrap();
        let active = channel.clone();
        let write = tokio::spawn(async move {
            active.turn(&"x".repeat(5000), true).await;
        });
        // Read one byte directly so the first command remains physically blocked.
        // BufReader would otherwise drain the pipe and let close finish before cancellation.
        let mut prefix = [0; 1];
        tokio::io::AsyncReadExt::read_exact(commands.get_mut(), &mut prefix)
            .await
            .unwrap();
        let close = tokio::spawn(endpoint.close());
        tokio::task::yield_now().await;
        close.abort();
        assert!(close.await.unwrap_err().is_cancelled());
        let mut tail = String::new();
        commands.read_line(&mut tail).await.unwrap();
        write.await.unwrap();
        assert_eq!(
            command(&mut commands).await,
            json!({"type":"release","threadId":"thread","tabId":"tab"})
        );
        let mut extra = String::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), commands.read_line(&mut extra))
                .await
                .is_err()
        );
    }
}
