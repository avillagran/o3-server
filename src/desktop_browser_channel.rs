//! Scoped, cancellable browser input and serialized control writes.
//! Launcher descriptor adoption and preview/CDP endpoint ownership remain separate consumers.
use crate::desktop_browser::{decode_event, Decoder, Kernel, TabKey};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

#[cfg(unix)]
mod descriptor;
mod endpoint;
pub use endpoint::Endpoint;

pub struct Change {
    pub key: TabKey,
    pub attached: bool,
}
struct State {
    kernel: Kernel,
    input_faulted: bool,
    next_subscription: u64,
    changes: HashMap<u64, mpsc::UnboundedSender<Change>>,
    inbound: HashMap<String, Arc<endpoint::Inbound>>,
}
// Client event encoders reserve one byte for LF inside their 256 MiB wire budget.
const MAX_EVENT_JSON_BYTES: usize = 256 * 1024 * 1024 - 1;
type Shared = Arc<Mutex<State>>;
struct Command {
    line: String,
    complete: oneshot::Sender<()>,
}
struct Scope {
    state: Shared,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
        let mut state = self.state.lock().unwrap();
        state.kernel.stream_closed();
        state.changes.clear();
        for inbound in state.inbound.values() {
            inbound.shutdown();
        }
        state.inbound.clear();
    }
}
struct ReaderGuard(Shared);
impl Drop for ReaderGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().kernel.stream_closed();
    }
}

pub struct Subscription {
    id: u64,
    state: Shared,
    receiver: mpsc::UnboundedReceiver<Change>,
}
impl Subscription {
    pub async fn next(&mut self) -> Option<Change> {
        self.receiver.recv().await
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.state.lock().unwrap().changes.remove(&self.id);
    }
}

#[derive(Clone)]
pub struct Channel {
    state: Shared,
    commands: mpsc::UnboundedSender<Command>,
    _scope: Arc<Scope>,
}
impl Channel {
    /// Own both streams until the last channel handle is dropped. Input shutdown
    /// clears visibility but preserves attachment state, as in the native channel.
    pub fn start<R, W>(input: R, control: W) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::start_limited(input, control, MAX_EVENT_JSON_BYTES)
    }
    fn start_limited<R, W>(mut input: R, mut control: W, max_json_bytes: usize) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let state = Arc::new(Mutex::new(State {
            kernel: Kernel::new(true, true),
            input_faulted: false,
            next_subscription: 0,
            changes: HashMap::new(),
            inbound: HashMap::new(),
        }));
        let (commands, mut receiver) = mpsc::unbounded_channel::<Command>();
        let writer = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                if command.complete.is_closed() {
                    continue;
                }
                // Native writes never fail the caller. Do not log an opaque CDP
                // payload or private endpoint when the launcher closes its pipe.
                let _ = control.write_all(command.line.as_bytes()).await;
                let _ = command.complete.send(());
            }
        });
        let shared = state.clone();
        let reader = tokio::spawn(async move {
            let _guard = ReaderGuard(shared.clone());
            let mut decoder = Decoder::bounded(max_json_bytes);
            let mut buffer = vec![0; 16 * 1024];
            loop {
                let batch = match input.read(&mut buffer).await {
                    Ok(0) => decoder.finish(),
                    Ok(count) => {
                        let batch = decoder.feed(&buffer[..count]);
                        Self::events(&shared, batch.values);
                        if let Some(error) = batch.error {
                            if error.reason == "FrameTooLarge" {
                                let mut state = shared.lock().unwrap();
                                state.input_faulted = true;
                                for inbound in state.inbound.values() {
                                    inbound.fault();
                                }
                            }
                            return;
                        }
                        continue;
                    }
                    Err(_) => return,
                };
                Self::events(&shared, batch.values);
                return;
            }
        });
        Self {
            commands,
            _scope: Arc::new(Scope {
                state: state.clone(),
                reader,
                writer,
            }),
            state,
        }
    }
    fn events(shared: &Shared, values: Vec<Value>) {
        let mut state = shared.lock().unwrap();
        for value in values {
            let Some(event) = decode_event(&value) else {
                continue;
            };
            if let Some(frame) = state.kernel.accept(&event) {
                if let Some(inbound) = state.inbound.get(&frame.key.id()) {
                    inbound.offer(frame.message);
                }
            }
            let kind = event["type"].as_str().unwrap();
            if matches!(kind, "attached" | "detached") {
                if kind == "detached" {
                    let key = TabKey {
                        thread_id: event["threadId"].as_str().unwrap().into(),
                        tab_id: event["tabId"].as_str().unwrap().into(),
                    };
                    if let Some(inbound) = state.inbound.get(&key.id()) {
                        inbound.shutdown();
                    }
                }
                state.changes.retain(|_, sender| {
                    sender
                        .send(Change {
                            key: TabKey {
                                thread_id: event["threadId"].as_str().unwrap().into(),
                                tab_id: event["tabId"].as_str().unwrap().into(),
                            },
                            attached: kind == "attached",
                        })
                        .is_ok()
                });
            }
        }
    }
    pub fn subscribe(&self) -> Subscription {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut state = self.state.lock().unwrap();
        let id = state.next_subscription;
        state.next_subscription = id
            .checked_add(1)
            .expect("Browser subscription IDs exhausted");
        state.changes.insert(id, sender);
        Subscription {
            id,
            state: self.state.clone(),
            receiver,
        }
    }
    pub fn is_attached(&self, key: &TabKey) -> bool {
        self.state.lock().unwrap().kernel.is_attached(key)
    }
    pub fn is_shown(&self, key: &TabKey) -> bool {
        self.state.lock().unwrap().kernel.is_shown(key)
    }
    pub async fn await_attached(&self, key: &TabKey, timeout: Duration) -> bool {
        // Register before checking: an attach between these operations is queued.
        let mut subscription = self.subscribe();
        if self.is_attached(key) {
            return true;
        }
        tokio::time::timeout(timeout, async {
            while let Some(change) = subscription.next().await {
                if change.attached
                    && change.key.thread_id == key.thread_id
                    && change.key.tab_id == key.tab_id
                {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
    async fn command(&self, value: Value) {
        let line = self.state.lock().unwrap().kernel.command(&value);
        if let Some(line) = line {
            let (complete, result) = oneshot::channel();
            if self.commands.send(Command { line, complete }).is_ok() {
                let _ = result.await;
            }
        }
    }
    pub async fn turn(&self, thread_id: &str, active: bool) {
        self.command(json!({"type":"turn","threadId":thread_id,"active":active}))
            .await;
    }
    pub async fn pointer(&self, key: &TabKey, phase: &str, x: f64, y: f64) {
        self.command(json!({"type":"pointer","threadId":key.thread_id,"tabId":key.tab_id,"phase":phase,"x":x,"y":y}))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_state_traces_cross_actual_async_streams() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/desktop-browser-4288.json"))
                .unwrap();
        for case in fixtures["stateCases"].as_array().unwrap() {
            if case["expected"]["available"] != true {
                continue;
            }
            let (mut launcher, input) = tokio::io::duplex(256);
            let (control, _commands) = tokio::io::duplex(4096);
            let channel = Channel::start(input, control);
            let key = TabKey {
                thread_id: "thread".into(),
                tab_id: "tab".into(),
            };
            let other = TabKey {
                thread_id: "other".into(),
                tab_id: "tab".into(),
            };
            for (index, event) in case["events"].as_array().unwrap().iter().enumerate() {
                launcher
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .unwrap();
                let sentinel = TabKey {
                    thread_id: "sentinel".into(),
                    tab_id: index.to_string(),
                };
                launcher.write_all(format!("{}\n", json!({"type":"attached","threadId":sentinel.thread_id,"tabId":sentinel.tab_id})).as_bytes()).await.unwrap();
                assert!(
                    channel
                        .await_attached(&sentinel, Duration::from_secs(1))
                        .await
                );
                let actual = json!({"attached":channel.is_attached(&key),"shown":channel.is_shown(&key),"otherAttached":channel.is_attached(&other),"otherShown":channel.is_shown(&other)});
                assert_eq!(actual, case["expected"]["snapshots"][index + 1]);
            }
            drop(launcher);
            tokio::task::yield_now().await;
            assert_eq!(
                channel.is_attached(&key),
                case["expected"]["afterClose"]["attached"]
            );
            assert!(channel.is_shown(&key));
        }
    }

    #[tokio::test]
    async fn actual_control_stream_serializes_writes_and_survives_broken_pipe() {
        let (_launcher, input) = tokio::io::duplex(64);
        let (control, mut commands) = tokio::io::duplex(4096);
        let channel = Channel::start(input, control);
        tokio::join!(channel.turn("thread", true), channel.turn("thread", false));
        let mut bytes = vec![0; 4096];
        let count = commands.read(&mut bytes).await.unwrap();
        let lines: Vec<Value> = std::str::from_utf8(&bytes[..count])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                json!({"type":"turn","threadId":"thread","active":true}),
                json!({"type":"turn","threadId":"thread","active":false})
            ]
        );
        drop(commands);
        tokio::time::timeout(Duration::from_secs(1), channel.turn("thread", true))
            .await
            .unwrap();
        assert!(
            !channel
                .await_attached(
                    &TabKey {
                        thread_id: "absent".into(),
                        tab_id: "tab".into()
                    },
                    Duration::from_millis(1)
                )
                .await
        );
    }

    #[tokio::test]
    async fn scope_drop_closes_input_and_subscriptions() {
        let (mut launcher, input) = tokio::io::duplex(64);
        let (control, _commands) = tokio::io::duplex(64);
        let channel = Channel::start(input, control);
        let mut subscription = channel.subscribe();
        drop(channel);
        assert!(subscription.next().await.is_none());
        tokio::task::yield_now().await;
        assert!(launcher.write_all(b"closed").await.is_err());
    }

    #[tokio::test]
    async fn cancelled_waiting_command_is_not_written_later() {
        use tokio::io::AsyncBufReadExt;
        let (_launcher, input) = tokio::io::duplex(64);
        let (control, mut commands) = tokio::io::duplex(64);
        let channel = Channel::start(input, control);
        let first = channel.clone();
        let active = tokio::spawn(async move { first.turn(&"a".repeat(200), true).await });
        let mut prefix = [0; 64];
        commands.read_exact(&mut prefix).await.unwrap();
        let waiting = channel.clone();
        let cancelled = tokio::spawn(async move { waiting.turn("cancelled", true).await });
        tokio::task::yield_now().await;
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        let mut reader = tokio::io::BufReader::new(commands);
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await.unwrap();
        let first: Value = serde_json::from_slice(&[prefix.as_slice(), &line].concat()).unwrap();
        assert_eq!(first["threadId"], "a".repeat(200));
        active.await.unwrap();
        let next = channel.clone();
        let next_write = tokio::spawn(async move { next.turn("next", false).await });
        line.clear();
        reader.read_until(b'\n', &mut line).await.unwrap();
        let next: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(next["threadId"], "next");
        next_write.await.unwrap();
    }
    #[tokio::test]
    async fn configured_record_overflow_fences_the_owned_loopback_endpoint() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message;
        crate::test_sandbox::assert_isolated();
        let (mut launcher, input) = tokio::io::duplex(4096);
        let (control, _commands) = tokio::io::duplex(4096);
        let channel = Channel::start_limited(input, control, 128);
        let key = TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        };
        launcher
            .write_all(b"{\"type\":\"attached\",\"threadId\":\"thread\",\"tabId\":\"tab\"}\n")
            .await
            .unwrap();
        assert!(channel.await_attached(&key, Duration::from_secs(1)).await);
        let endpoint = channel.endpoint(key).await.unwrap();
        let address = url::Url::parse(endpoint.url()).expect("invalid owned endpoint");
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", address.port().unwrap()))
            .await
            .expect("owned endpoint unavailable");
        let Ok(Ok((mut socket, _))) = tokio::time::timeout(
            Duration::from_secs(2),
            tokio_tungstenite::client_async(endpoint.url(), stream),
        )
        .await
        else {
            panic!("private endpoint connection failed");
        };
        launcher.write_all(&[b'x'; 129]).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("oversized input did not fence endpoint");
        assert!(matches!(
            closed,
            None | Some(Ok(Message::Close(_))) | Some(Err(_))
        ));
        assert!(
            channel
                .endpoint(TabKey {
                    thread_id: "thread".into(),
                    tab_id: "replacement".into()
                })
                .await
                .is_err(),
            "faulted input cannot admit a replacement endpoint"
        );
        endpoint.close().await;
    }
}
