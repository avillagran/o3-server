//! Private actual HTTP/WS admission; no microphone, live profile or paid provider.
#[path = "speech/http-store.rs"]
mod memory;
#[path = "speech/service-process.rs"]
mod process;
#[path = "support/sandbox.rs"]
mod sandbox;
use futures_util::{SinkExt, StreamExt};
use o3_server::{
    speech_service as candidate,
    speech_service::{Error, Loaded, Task},
    store::State,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Fixture {
    state: Arc<State>,
    memory: Arc<memory::Memory>,
    engine: Arc<process::Engine>,
}
impl Fixture {
    fn new(mode: &'static str) -> Self {
        sandbox::assert_isolated();
        let root = std::env::temp_dir().join(format!(
            "speech-http-{}",
            o3_server::store::random_id().unwrap()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let state = Arc::new(State::open(&root).unwrap());
        o3_server::settings::update(&state,&json!({"patch":{"speechModelId":"handy-computer/moonshine-tiny-gguf","speechAcceleration":"cpu","speechModelUnloadTimeout":"never"}})).unwrap();
        let memory = Arc::new(memory::Memory::new());
        let engine = process::Engine::new(mode);
        let service = candidate::Service::new(
            Arc::new(o3_server::speech_model_store::Store::new(
                memory.clone(),
                memory.clone(),
            )),
            engine.clone(),
            Arc::new(candidate::WallClock),
            None,
        );
        o3_server::speech_state::install(&state, service).unwrap();
        Self {
            state,
            memory,
            engine,
        }
    }
    fn token(&self, scopes: &[&str]) -> String {
        self.state
            .issue_session(
                "private-voice-fixture",
                &scopes.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
                3_600_000,
                "private-speech",
            )
            .unwrap()
    }
    async fn stop(&self) {
        self.state.begin_shutdown();
        o3_server::speech_state::shutdown(&self.state)
            .await
            .unwrap();
    }
}
struct Reply {
    status: u16,
    body: Vec<u8>,
}
impl Reply {
    fn value(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| json!(String::from_utf8_lossy(&self.body)))
    }
}
async fn request(
    state: Arc<State>,
    method: &str,
    path: &str,
    token: &str,
    content_type: Option<&str>,
    body: &[u8],
    length: Option<&str>,
) -> Reply {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let serving = tokio::spawn(o3_server::serve_io(server, state));
    let mut header=format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n",length.map(str::to_owned).unwrap_or_else(||body.len().to_string()));
    if let Some(value) = content_type {
        header.push_str(&format!("Content-Type: {value}\r\n"));
    }
    header.push_str("\r\n");
    client.write_all(header.as_bytes()).await.unwrap();
    client.write_all(body).await.unwrap();
    let mut response = vec![];
    tokio::time::timeout(Duration::from_secs(30), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let _ = serving.await.unwrap();
    let split = response
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8_lossy(&response[..split]);
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        body: response[split + 4..].to_vec(),
    }
}
fn normalize(mut value: Value) -> Value {
    if value.is_object() && value.get("traceId").is_some() {
        value["traceId"] = json!("<native-trace>");
    }
    value
}
fn options() -> Value {
    json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})
}
#[test]
fn original_http_schema_and_scope_order_reach_the_actual_connection_service() {
    if process::fresh("original_http_schema_and_scope_order_reach_the_actual_connection_service") {
        return;
    }
    process::runtime().block_on(async {
        tokio::spawn(async {
            process::assert_stack();
            let fixture = Fixture::new("text:fixture");
            let golden: Value =
                serde_json::from_str(include_str!("fixtures/speech-http-admission-4314.json"))
                    .unwrap();
            assert_eq!(golden["cases"].as_array().unwrap().len(), 111);
            let before = fs::read(fixture.state.dir.join("settings.json")).unwrap();
            let mut observed = 0;
            for row in golden["cases"].as_array().unwrap() {
                // The producer replaces admitted domain methods with one observed Service
                // failure. Only pre-domain rows are comparable to this real Service controller.
                if !row["calls"].as_array().unwrap().is_empty() {
                    continue;
                }
                let scopes = row["scopes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|scope| scope.as_str().unwrap())
                    .collect::<Vec<_>>();
                let token = fixture.token(&scopes);
                let input = &row["input"];
                let body = input
                    .get("raw")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| input.get("body").map(Value::to_string))
                    .unwrap_or_default();
                let reply = request(
                    fixture.state.clone(),
                    input["method"].as_str().unwrap_or("POST"),
                    input["path"].as_str().unwrap(),
                    &token,
                    row["actualRequestContentType"].as_str(),
                    body.as_bytes(),
                    input["contentLength"].as_str(),
                )
                .await;
                assert_eq!(
                    reply.status,
                    row["status"].as_u64().unwrap() as u16,
                    "{} {:?}",
                    row["name"],
                    scopes
                );
                assert_eq!(
                    normalize(reply.value()),
                    row["body"],
                    "{} {:?}",
                    row["name"],
                    scopes
                );
                observed += 1;
            }
            assert!(observed > 80);
            assert_eq!(
                fs::read(fixture.state.dir.join("settings.json")).unwrap(),
                before
            );
            assert!(fixture.engine.placements().is_empty());
            assert!(fixture.engine.events().is_empty());
            assert_eq!(fixture.memory.requests.load(Ordering::SeqCst), 0);
            fixture.stop().await;
        })
        .await
        .unwrap();
    });
}
async fn ws(
    state: Arc<State>,
    token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
    tokio_tungstenite::tungstenite::Error,
> {
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(o3_server::serve_io(server, state.clone()));
    let session = state.session(token).unwrap();
    let ticket = o3_server::auth::websocket_ticket(
        &session,
        &state.signing_key,
        o3_server::store::now_ms(),
        Some("media"),
    );
    tokio_tungstenite::client_async(format!("ws://localhost/ws/voice?wsTicket={ticket}"), client)
        .await
        .map(|(socket, _)| socket)
}
async fn next(socket: &mut tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}
#[test]
fn actual_media_socket_finishes_and_revoked_open_session_cannot_send_more_pcm() {
    if process::fresh("actual_media_socket_finishes_and_revoked_open_session_cannot_send_more_pcm")
    {
        return;
    }
    process::runtime().block_on(async {
        tokio::spawn(async {
            process::assert_stack();
            let fixture = Fixture::new("stream");
            let token = fixture.token(&["orchestration:operate"]);
            let mut socket = ws(fixture.state.clone(), &token).await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"start","options":options()})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await, json!({"type":"ready"}));
            socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    [0.25f32.to_le_bytes(); 512].concat().into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "update");
            let before = fixture
                .engine
                .events()
                .iter()
                .filter(|event| event["kind"] == "feed")
                .count();
            let sid = fixture.state.session(&token).unwrap()["sid"]
                .as_str()
                .unwrap()
                .to_owned();
            fixture.state.revoke_session(&sid).unwrap();
            let _ = socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    [0.25f32.to_le_bytes(); 512].concat().into(),
                ))
                .await;
            assert_eq!(
                next(&mut socket).await,
                json!({"type":"error","message":"Speech authorization ended."})
            );
            let close = socket.next().await.unwrap().unwrap();
            assert!(matches!(
                close,
                tokio_tungstenite::tungstenite::Message::Close(_)
            ));
            assert_eq!(
                fixture
                    .engine
                    .events()
                    .iter()
                    .filter(|event| event["kind"] == "feed")
                    .count(),
                before
            );
            let current = fixture.token(&["orchestration:operate"]);
            let mut socket = ws(fixture.state.clone(), &current).await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"start","options":options()})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "ready");
            socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    [0.5f32.to_le_bytes(); 512].concat().into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "update");
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"finish"}).to_string().into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "finished");
            let _ = socket.next().await;
            let current = fixture.token(&["orchestration:operate"]);
            let mut socket = ws(fixture.state.clone(), &current).await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"start","options":options()})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "ready");
            let sid = fixture.state.session(&current).unwrap()["sid"]
                .as_str()
                .unwrap()
                .to_owned();
            fixture
                .state
                .db
                .lock()
                .unwrap()
                .execute(
                    "UPDATE auth_sessions SET scopes='orchestration:read' WHERE session_id=?",
                    rusqlite::params![sid],
                )
                .unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"finish"}).to_string().into(),
                ))
                .await
                .unwrap();
            assert_eq!(
                next(&mut socket).await,
                json!({"type":"error","message":"Speech authorization ended."})
            );
            let _ = socket.next().await;
            fixture.stop().await;
        })
        .await
        .unwrap();
    });
}
#[test]
fn revoked_media_authority_fences_an_already_running_native_feed() {
    if process::fresh("revoked_media_authority_fences_an_already_running_native_feed") {
        return;
    }
    process::runtime().block_on(async {
        tokio::spawn(async {
            process::assert_stack();
            let fixture = Fixture::new("stream-hang");
            let token = fixture.token(&["orchestration:operate"]);
            let mut socket = ws(fixture.state.clone(), &token).await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"type":"start","options":options()})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            assert_eq!(next(&mut socket).await["type"], "ready");
            socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    [0.25f32.to_le_bytes(); 512].concat().into(),
                ))
                .await
                .unwrap();
            let pid = fixture.engine.entered().await;
            let exit = process::exit(pid);
            let sid = fixture.state.session(&token).unwrap()["sid"]
                .as_str()
                .unwrap()
                .to_owned();
            fixture.state.revoke_session(&sid).unwrap();
            assert_eq!(next(&mut socket).await["type"], "error");
            let _exit = tokio::time::timeout(Duration::from_secs(2), exit.readable())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                fixture
                    .engine
                    .events()
                    .iter()
                    .filter(|event| event["kind"] == "stream-feed-entered")
                    .count(),
                1
            );
            fixture.stop().await;
        })
        .await
        .unwrap();
    });
}

#[test]
fn local_http_controls_and_pcm_use_the_installed_owner_and_private_settings() {
    if process::fresh("local_http_controls_and_pcm_use_the_installed_owner_and_private_settings") {
        return;
    }
    process::runtime().block_on(async {
        tokio::spawn(async {
            process::assert_stack();
            let fixture = Fixture::new("text:fixture");
            let token = fixture.token(&["orchestration:operate", "settings:write"]);
            let status = request(
                fixture.state.clone(),
                "GET",
                "/api/voice/status",
                &token,
                None,
                &[],
                None,
            )
            .await;
            assert_eq!(status.status, 200);
            assert_eq!(status.value()["state"], "ready");
            let models = request(
                fixture.state.clone(),
                "GET",
                "/api/voice/models?benchmarks=1",
                &token,
                None,
                &[],
                None,
            )
            .await;
            assert_eq!(models.status, 200);
            assert_eq!(
                models.value()["models"].as_array().unwrap().len(),
                o3_server::speech_catalog::all().len()
            );
            let prepared = request(
                fixture.state.clone(),
                "POST",
                "/api/voice/models/prepare",
                &token,
                None,
                &[],
                None,
            )
            .await;
            assert_eq!(prepared.status, 204);
            assert!(prepared.body.is_empty());
            let encoded = options().to_string().into_bytes();
            let mut body = (encoded.len() as u32).to_le_bytes().to_vec();
            body.extend(encoded);
            body.extend(0.25f32.to_le_bytes());
            let transcribed = request(
                fixture.state.clone(),
                "POST",
                "/api/voice/transcribe",
                &token,
                Some("application/octet-stream"),
                &body,
                None,
            )
            .await;
            assert_eq!(transcribed.status, 200);
            assert_eq!(transcribed.value(), json!({"text":"fixture"}));
            assert_eq!(fixture.engine.placements(), vec!["cpu"]);
            for (path, payload, field, expected) in [
                (
                    "/api/voice/acceleration",
                    json!({"acceleration":"auto"}),
                    "acceleration",
                    "auto",
                ),
                (
                    "/api/voice/model-unload-timeout",
                    json!({"timeout":"min_2"}),
                    "modelUnloadTimeout",
                    "min_2",
                ),
            ] {
                let reply = request(
                    fixture.state.clone(),
                    "POST",
                    path,
                    &token,
                    Some("application/json"),
                    payload.to_string().as_bytes(),
                    None,
                )
                .await;
                assert_eq!(reply.status, 200);
                assert_eq!(reply.value()[field], expected);
            }
            let default = o3_server::speech_catalog::DEFAULT_MODEL_ID;
            let payload = json!({"modelId":default}).to_string();
            let selected = request(
                fixture.state.clone(),
                "POST",
                "/api/voice/models/select",
                &token,
                Some("application/json"),
                payload.as_bytes(),
                None,
            )
            .await;
            assert_eq!(selected.status, 200);
            assert_eq!(
                request(
                    fixture.state.clone(),
                    "POST",
                    "/api/voice/models/prepare",
                    &token,
                    None,
                    &[],
                    None
                )
                .await
                .value()["reason"],
                "invalid_command"
            );
            let downloaded = request(
                fixture.state.clone(),
                "POST",
                "/api/voice/models/download",
                &token,
                Some("application/json"),
                payload.as_bytes(),
                None,
            )
            .await;
            assert_eq!(downloaded.status, 500);
            assert_eq!(downloaded.value()["_tag"], "EnvironmentInternalError");
            assert_eq!(fixture.memory.requests.load(Ordering::SeqCst), 1);
            assert_eq!(
                *fixture.memory.urls.lock().unwrap(),
                vec![o3_server::speech_catalog::get(default).unwrap().url()]
            );
            let payload = json!({"modelId":"handy-computer/moonshine-tiny-gguf"}).to_string();
            assert_eq!(
                request(
                    fixture.state.clone(),
                    "POST",
                    "/api/voice/models/select",
                    &token,
                    Some("application/json"),
                    payload.as_bytes(),
                    None
                )
                .await
                .status,
                200
            );
            assert_eq!(
                request(
                    fixture.state.clone(),
                    "POST",
                    "/api/voice/models/cancel",
                    &token,
                    Some("application/json"),
                    payload.as_bytes(),
                    None
                )
                .await
                .status,
                200
            );
            assert_eq!(
                request(
                    fixture.state.clone(),
                    "POST",
                    "/api/voice/models/remove",
                    &token,
                    Some("application/json"),
                    payload.as_bytes(),
                    None
                )
                .await
                .status,
                200
            );
            assert!(!fixture
                .memory
                .ready
                .lock()
                .unwrap()
                .contains("handy-computer/moonshine-tiny-gguf"));
            assert_eq!(
                o3_server::settings::read(&fixture.state).unwrap()["speechModelId"],
                "handy-computer/moonshine-tiny-gguf"
            );
            fixture.stop().await;
        })
        .await
        .unwrap();
    });
}
async fn raw_ws(
    state: Arc<State>,
    query: &str,
    bearer: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (client, server) = tokio::io::duplex(64 * 1024);
    tokio::spawn(o3_server::serve_io(server, state));
    let mut request = format!("ws://localhost/ws/voice{query}")
        .into_client_request()
        .unwrap();
    if let Some(token) = bearer {
        request.headers_mut().insert(
            hyper::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
    }
    tokio_tungstenite::client_async(request, client)
        .await
        .map(|(socket, _)| socket)
}
#[test]
fn media_ticket_audience_mcp_exclusion_and_precedence_are_enforced_before_owner_start() {
    if process::fresh(
        "media_ticket_audience_mcp_exclusion_and_precedence_are_enforced_before_owner_start",
    ) {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
  process::assert_stack();let fixture=Fixture::new("stream");let operator=fixture.token(&["orchestration:operate"]);let reader=fixture.token(&["orchestration:read"]);let mcp=fixture.state.issue_mcp_session("full-access",3_600_000,"private").unwrap();
  for (token,expected) in [(&reader,403),(&mcp,401)] {
   let error=match raw_ws(fixture.state.clone(),"",Some(token)).await {Err(error)=>error,Ok(_)=>panic!("Credential must not start a voice owner")};assert!(matches!(error,tokio_tungstenite::tungstenite::Error::Http(reply) if reply.status().as_u16()==expected));
  }
  let session=fixture.state.session(&operator).unwrap();let wrong=o3_server::auth::websocket_ticket(&session,&fixture.state.signing_key,o3_server::store::now_ms(),Some("rpc"));let right=o3_server::auth::websocket_ticket(&session,&fixture.state.signing_key,o3_server::store::now_ms(),Some("media"));
  for query in [format!("?wsTicket={wrong}"),format!("?wsTicket={right}&wsTicket={wrong}")] {
   let error=match raw_ws(fixture.state.clone(),&query,Some(&operator)).await {Err(error)=>error,Ok(_)=>panic!("Invalid media ticket must not fall back to bearer")};assert!(matches!(error,tokio_tungstenite::tungstenite::Error::Http(reply) if reply.status().as_u16()==401));
  }
  drop(raw_ws(fixture.state.clone(),&format!("?wsTicket={wrong}&wsTicket={right}"),None).await.unwrap());drop(raw_ws(fixture.state.clone(),"?wsTicket=%20%20",Some(&operator)).await.unwrap());assert!(fixture.engine.placements().is_empty());fixture.stop().await;
 }).await.unwrap();});
}
