//! Media-ticket speech socket with a separately supervised current-authority lease.
use super::*;
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::{
    tungstenite::{
        self,
        protocol::{frame::coding::CloseCode, CloseFrame, Role},
        Message,
    },
    WebSocketStream,
};
type Socket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;
type Writer = Arc<Mutex<futures_util::stream::SplitSink<Socket, Message>>>;
#[derive(Clone, Copy)]
enum End {
    Authority,
    Peer,
    Overflow,
    Lifetime,
}
enum Wait<T> {
    Result(Result<T, speech::Error>),
    End(End),
}
async fn current<T>(
    future: impl std::future::Future<Output = Result<T, speech::Error>>,
    revoked: &mut watch::Receiver<Option<Denied>>,
    closed: &mut watch::Receiver<Option<End>>,
    deadline: tokio::time::Instant,
) -> Wait<T> {
    if revoked.borrow().is_some() {
        return Wait::End(End::Authority);
    }
    if let Some(end) = *closed.borrow() {
        return Wait::End(end);
    }
    tokio::pin!(future);
    tokio::select! {biased;
        _=revoked.changed()=>Wait::End(End::Authority),
        _=closed.changed()=>Wait::End(closed.borrow().unwrap_or(End::Peer)),
        _=tokio::time::sleep_until(deadline)=>Wait::End(End::Lifetime),
        result=future.as_mut()=>Wait::Result(result),
    }
}
async fn event(writer: &Writer, event: Value) -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        writer
            .lock()
            .await
            .send(Message::Text(event.to_string().into()))
            .await
    })
    .await
    .is_ok_and(|result| result.is_ok())
}
async fn close(writer: &Writer, code: CloseCode) {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        writer
            .lock()
            .await
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: if code == CloseCode::Policy {
                    "Speech stream ended".into()
                } else {
                    "".into()
                },
            })))
            .await
    })
    .await;
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
fn ticket(request: &Request<Incoming>) -> Option<String> {
    url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .filter(|(key, _)| key == "wsTicket")
        .map(|(_, value)| value.into_owned())
        .last()
}
pub(super) async fn upgrade(state: Arc<State>, mut request: Request<Incoming>) -> Reply {
    if request.method() != Method::GET || !crate::speech_state::available(&state) {
        return empty(StatusCode::NOT_FOUND);
    }
    let session =
        match ticket(&request).filter(|ticket| !crate::native_goal::trim(ticket).is_empty()) {
            Some(ticket) => state.ticket_session(&ticket, "media"),
            None => crate::http::bearer(&request).and_then(|token| state.session(token)),
        };
    // This backend advertises bearer sessions only, never a silent DPoP downgrade.
    let Some(session) = session.filter(|session| session["jkt"].is_null()) else {
        return empty(StatusCode::UNAUTHORIZED);
    };
    let authority = Authority::new(state.clone(), session, "orchestration:operate");
    if authority.check().is_err() {
        return empty(if matches!(authority.denied(), Some(Denied::Scope(_))) {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        });
    }
    let mut handshake = Request::new(());
    *handshake.method_mut() = request.method().clone();
    *handshake.uri_mut() = request.uri().clone();
    *handshake.version_mut() = request.version();
    *handshake.headers_mut() = request.headers().clone();
    let response = match tungstenite::handshake::server::create_response(&handshake) {
        Ok(response) => response.map(|_| body::empty()),
        Err(_) => return empty(StatusCode::BAD_REQUEST),
    };
    let lease = request.extensions().get::<crate::http::Lease>().cloned();
    let upgraded = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let _lease = lease;
        if let Ok(io) = upgraded.await {
            // Application limits are128k text/64k PCM. This bounded transport also
            // admits their immediate oversize boundary so the native error is sent.
            let socket = WebSocketStream::from_raw_socket(
                TokioIo::new(io),
                Role::Server,
                Some(
                    tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(256 * 1024))
                        .max_frame_size(Some(256 * 1024)),
                ),
            )
            .await;
            run(socket, state, authority).await;
        }
    });
    response
}
async fn run(socket: Socket, state: Arc<State>, authority: Authority) {
    let (sink, mut reader) = socket.split();
    let writer = Arc::new(Mutex::new(sink));
    let (messages, mut pending) = mpsc::channel(2);
    let (closed_tx, mut closed) = watch::channel(None);
    let reader_writer = writer.clone();
    let reader_task = tokio::spawn(async move {
        let mut reason = End::Peer;
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Ping(bytes)) => {
                    if !tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        reader_writer.lock().await.send(Message::Pong(bytes)).await
                    })
                    .await
                    .is_ok_and(|result| result.is_ok())
                    {
                        break;
                    }
                }
                Ok(Message::Pong(_)) => {}
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(message) => {
                    if messages.try_send(message).is_err() {
                        reason = End::Overflow;
                        break;
                    }
                }
            }
        }
        closed_tx.send_replace(Some(reason));
    });
    let (revoked_tx, mut revoked) = watch::channel(None);
    let monitor_authority = authority.clone();
    let monitor = tokio::spawn(async move {
        let mut changes = state.changed.subscribe();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            if monitor_authority.check().is_err() {
                revoked_tx
                    .send_replace(Some(monitor_authority.denied().unwrap_or(Denied::Invalid)));
                break;
            }
            tokio::select! {_=changes.changed()=>{},_=tick.tick()=>{}}
        }
    });
    let _tasks = Tasks(vec![reader_task, monitor]);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
    let mut stream: Option<speech::Stream> = None;
    let mut failure = None;
    let mut end = End::Peer;
    loop {
        let message = current(
            async { pending.recv().await.ok_or(speech::Error::Cancelled) },
            &mut revoked,
            &mut closed,
            deadline,
        )
        .await;
        let message = match message {
            Wait::Result(Ok(message)) => message,
            Wait::End(reason) => {
                end = reason;
                break;
            }
            Wait::Result(Err(_)) => break,
        };
        if authority.check().is_err() {
            end = End::Authority;
            break;
        }
        match message {
            Message::Text(text) => {
                if text.len() > crate::speech_request::MAX_OPTIONS_BYTES {
                    failure = Some("Invalid speech stream command.");
                    break;
                }
                let command = serde_json::from_str::<Value>(&text).ok();
                let Some(command) = command.as_ref().filter(|value| value.is_object()) else {
                    failure = Some("Live transcription failed. Please try recording again.");
                    break;
                };
                match command["type"].as_str() {
                    Some("start") => {
                        let options = command.get("options").cloned().and_then(|value| {
                            crate::speech_request::TranscriptionOptions::decode(value).ok()
                        });
                        let Some(options) = options else {
                            failure =
                                Some("Live transcription failed. Please try recording again.");
                            break;
                        };
                        if stream.is_some() {
                            failure = Some("Speech stream already started.");
                            break;
                        }
                        let call = match authority.bind() {
                            Ok(call) => call,
                            Err(_) => {
                                end = End::Authority;
                                break;
                            }
                        };
                        let mut opening: crate::speech_service::Pending = match call
                            .opening_stream(options)
                        {
                            Ok(opening) => opening,
                            Err(_) => {
                                failure =
                                    Some("Live transcription failed. Please try recording again.");
                                break;
                            }
                        };
                        match current(opening.ready(), &mut revoked, &mut closed, deadline).await {
                            Wait::Result(Ok(opened)) => stream = Some(opened),
                            Wait::Result(Err(_)) => {
                                if authority.denied().is_some() {
                                    end = End::Authority;
                                } else {
                                    failure = Some(
                                        "Live transcription failed. Please try recording again.",
                                    );
                                }
                                break;
                            }
                            Wait::End(reason) => {
                                end = reason;
                                opening.cancel();
                                if let Ok(opened) = opening.ready().await {
                                    let _ = opened.close().await;
                                }
                                break;
                            }
                        }
                        if authority.check().is_err() {
                            end = End::Authority;
                            break;
                        }
                        if !event(&writer, json!({"type":"ready"})).await {
                            break;
                        }
                    }
                    Some("finish") => {
                        let Some(active) = stream.as_ref() else {
                            failure = Some("Start speech transcription before finishing.");
                            break;
                        };
                        match current(active.finish(), &mut revoked, &mut closed, deadline).await {
                            Wait::Result(Ok(text)) => {
                                if authority.check().is_err() {
                                    end = End::Authority;
                                    break;
                                }
                                if !event(&writer, json!({"type":"finished","text":text})).await {
                                    break;
                                }
                                break;
                            }
                            Wait::Result(Err(_)) => {
                                failure =
                                    Some("Live transcription failed. Please try recording again.");
                                break;
                            }
                            Wait::End(reason) => {
                                end = reason;
                                break;
                            }
                        }
                    }
                    _ => {
                        failure = Some("Live transcription failed. Please try recording again.");
                        break;
                    }
                }
            }
            Message::Binary(bytes) => {
                if bytes.len() > 64_000 {
                    failure = Some("Speech audio chunk is too large.");
                    break;
                }
                let Some(active) = stream.as_ref() else {
                    failure = Some("Start speech transcription before sending audio.");
                    break;
                };
                match current(
                    active.feed(bytes.to_vec()),
                    &mut revoked,
                    &mut closed,
                    deadline,
                )
                .await
                {
                    Wait::Result(Ok(update)) => {
                        if authority.check().is_err() {
                            end = End::Authority;
                            break;
                        }
                        if !event(&writer,json!({"type":"update","revision":update["revision"],"text":update["text"]})).await {break;}
                    }
                    Wait::Result(Err(_)) => {
                        failure = Some("Live transcription failed. Please try recording again.");
                        break;
                    }
                    Wait::End(reason) => {
                        end = reason;
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(active) = stream {
        let _ = active.close().await;
    }
    if matches!(end, End::Authority) {
        failure = Some("Speech authorization ended.");
    }
    if matches!(end, End::Overflow) {
        failure = Some("Live transcription failed. Please try recording again.");
    }
    if let Some(message) = failure {
        let _ = event(&writer, json!({"type":"error","message":message})).await;
        close(&writer, CloseCode::Policy).await;
    } else {
        close(&writer, CloseCode::Normal).await;
    }
}
