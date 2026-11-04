//! HTTP routes and the WebSocket upgrade, on bare hyper to stay small.
use crate::body::{self, Body};
use crate::{
    auth, rpc,
    store::{now_ms, State},
};
use base64::Engine;
use http_body_util::{BodyExt, Limited};
use hyper::{
    body::{Bytes, Incoming},
    header,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{convert::Infallible, sync::Arc};
use tokio::net::TcpListener;
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

type Reply = Response<Body>;
const AUTH_BODY_LIMIT: usize = 64 * 1024;

pub(crate) struct Limits {
    connections: Arc<tokio::sync::Semaphore>,
    peers: std::sync::Mutex<
        std::collections::HashMap<std::net::IpAddr, std::sync::Weak<tokio::sync::Semaphore>>,
    >,
    hook_window: std::sync::Mutex<(
        std::time::Instant,
        usize,
        std::collections::HashMap<std::net::IpAddr, usize>,
    )>,
    token_window: std::sync::Mutex<(std::time::Instant, usize)>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: Arc::new(tokio::sync::Semaphore::new(128)),
            peers: Default::default(),
            hook_window: std::sync::Mutex::new((std::time::Instant::now(), 0, Default::default())),
            token_window: std::sync::Mutex::new((std::time::Instant::now(), 0)),
        }
    }
}
type Lease = Arc<(
    tokio::sync::OwnedSemaphorePermit,
    Option<tokio::sync::OwnedSemaphorePermit>,
)>;

fn json_reply(status: StatusCode, body: String) -> Reply {
    let mut response = Response::new(body::full(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

fn unauthorized() -> Reply {
    json_reply(StatusCode::UNAUTHORIZED, json!({ "_tag":"EnvironmentAuthInvalidError", "code": "auth_invalid", "reason": "invalid_credential", "traceId":crate::store::random_id().unwrap_or_default() }).to_string())
}

fn bearer(request: &Request<Incoming>) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn query(request: &Request<Incoming>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(request.uri().query()?.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

async fn body(request: Request<Incoming>) -> Option<Bytes> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Limited::new(request.into_body(), AUTH_BODY_LIMIT).collect(),
    )
    .await
    .ok()?
    .ok()
    .map(|b| b.to_bytes())
}

/// `POST /oauth/token`: RFC 8693 exchange of a pairing secret for a bearer session.
async fn token(state: &State, request: Request<Incoming>) -> Reply {
    {
        let mut window = state.http.token_window.lock().unwrap();
        if window.0.elapsed() >= std::time::Duration::from_secs(1) {
            *window = (std::time::Instant::now(), 0);
        }
        if window.1 >= 30 {
            return json_reply(StatusCode::TOO_MANY_REQUESTS, "{}".into());
        }
        window.1 += 1;
    }
    let Some(form) = body(request).await else {
        return json_reply(
            StatusCode::BAD_REQUEST,
            r#"{"code":"invalid_request"}"#.into(),
        );
    };
    let mut credential = None;
    let mut client = json!({});
    for (name, value) in url::form_urlencoded::parse(&form) {
        match name.as_ref() {
            "subject_token" => credential = Some(value.into_owned()),
            "client_label" | "client_device_type" | "client_os" => {
                client[name.as_ref()] = value.as_ref().into()
            }
            _ => {}
        }
    }
    let Some(credential) = credential else {
        return json_reply(
            StatusCode::BAD_REQUEST,
            r#"{"code":"invalid_request"}"#.into(),
        );
    };
    match state.exchange_pairing(&credential, &client) {
        Ok(Some(result)) => json_reply(StatusCode::OK, result.to_string()),
        Ok(None) => unauthorized(),
        Err(_) => json_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"code":"internal_error"}"#.into(),
        ),
    }
}

fn websocket_ticket(state: &State, session: &Value, usage: Option<&str>) -> Reply {
    let now = now_ms();
    let ticket = auth::websocket_ticket(session, &state.signing_key, now, usage);
    json_reply(
        StatusCode::OK,
        json!({ "ticket": ticket, "expiresAt": crate::store::iso(now + auth::WEBSOCKET_TTL_MS) })
            .to_string(),
    )
}

fn upgrade(state: Arc<State>, session: Value, mut request: Request<Incoming>) -> Reply {
    let Some(key) = request.headers().get(header::SEC_WEBSOCKET_KEY).cloned() else {
        return json_reply(
            StatusCode::BAD_REQUEST,
            r#"{"error":"websocket_required"}"#.into(),
        );
    };
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(hash.finalize());
    let preview = if request.uri().path() == "/api/preview-stream/ws" {
        Some((
            query(&request, "threadId").unwrap_or_default(),
            query(&request, "tabId").unwrap_or_default(),
            query(&request, "interactive").as_deref() != Some("false"),
        ))
    } else {
        None
    };
    let lease = request.extensions().get::<Lease>().cloned();
    let upgrading = hyper::upgrade::on(&mut request);
    tokio::task::spawn(async move {
        let _lease = lease;
        if let Ok(upgraded) = upgrading.await {
            let socket = WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                Some(
                    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(8 * 1024 * 1024))
                        .max_frame_size(Some(8 * 1024 * 1024)),
                ),
            )
            .await;
            if let Some((thread, tab, interactive)) = preview {
                crate::server_browser::stream(socket, state, session, thread, tab, interactive)
                    .await;
            } else {
                rpc::run(socket, state, session).await;
            }
        }
        crate::heap::collect();
    });
    let mut response = Response::new(body::empty());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("Upgrade"),
    );
    headers.insert(
        header::UPGRADE,
        header::HeaderValue::from_static("websocket"),
    );
    headers.insert(
        header::SEC_WEBSOCKET_ACCEPT,
        accept.parse().expect("base64 is a valid header"),
    );
    response
}

async fn route(state: Arc<State>, mut request: Request<Incoming>) -> Result<Reply, Infallible> {
    let path = request.uri().path().to_owned();
    Ok(match (request.method().clone(), path.as_str()) {
        (_, "/mcp") => crate::mcp::handle(state, request).await,
        (Method::GET, "/.well-known/t3/environment") => json_reply(
            StatusCode::OK,
            crate::settings::descriptor(&state).to_string(),
        ),
        (Method::POST, p) if p.starts_with("/api/hooks/") => {
            let parts: Vec<_> = p.trim_start_matches("/api/hooks/").split('/').collect();
            if parts.len() != 2 {
                return Ok(json_reply(StatusCode::NOT_FOUND, "{}".into()));
            }
            let (id, token) = (parts[0].to_owned(), parts[1].to_owned());
            // Every rejection looks the same and costs one keyed lookup: no body is read
            // without the hook's token, so unauthenticated traffic cannot crowd out its sender.
            let check_state = state.clone();
            let (check_id, check_token) = (id.clone(), token.clone());
            if !tokio::task::spawn_blocking(move || {
                crate::webhook::admits(&check_state, &check_id, &check_token)
            })
            .await
            .unwrap_or(false)
            {
                return Ok(json_reply(StatusCode::NOT_FOUND, "{}".into()));
            }
            {
                let mut window = state.http.hook_window.lock().unwrap();
                if window.0.elapsed() >= std::time::Duration::from_secs(1) {
                    *window = (std::time::Instant::now(), 0, Default::default());
                }
                if window.1 >= 60 {
                    return Ok(json_reply(StatusCode::TOO_MANY_REQUESTS, "{}".into()));
                }
                window.1 += 1;
            }
            let headers = request.headers().clone();
            let query = request.uri().query().unwrap_or("").to_owned();
            let bytes = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                Limited::new(request.into_body(), 1024 * 1024).collect(),
            )
            .await;
            let Ok(Ok(bytes)) = bytes else {
                return Ok(json_reply(StatusCode::PAYLOAD_TOO_LARGE, "{}".into()));
            };
            let bytes = bytes.to_bytes();
            let result = tokio::task::spawn_blocking(move || {
                crate::webhook::accept(&state, &id, &token, &headers, &query, &bytes)
            })
            .await
            .unwrap_or((503, "error"));
            if result.0 == 404 {
                return Ok(json_reply(StatusCode::NOT_FOUND, "{}".into()));
            }
            let mut response = json_reply(
                StatusCode::from_u16(result.0).unwrap(),
                json!({"outcome":result.1}).to_string(),
            );
            response
                .headers_mut()
                .insert("x-t3-hook-outcome", result.1.parse().unwrap());
            response
        }
        (Method::POST, "/oauth/token") => token(&state, request).await,
        (Method::POST, p) if p.starts_with(crate::assets::UPLOAD_PREFIX) => {
            let token = p[crate::assets::UPLOAD_PREFIX.len()..].to_owned();
            if token.is_empty() {
                return Ok(json_reply(StatusCode::NOT_FOUND, "Not Found".into()));
            }
            Box::pin(crate::assets::upload(&state, &token, request)).await
        }
        (Method::GET | Method::HEAD, p) if p.starts_with(crate::assets::ASSET_PREFIX) => {
            Box::pin(crate::assets::serve(&state, &request)).await
        }
        (Method::GET, "/api/preview-stream/ws") => {
            let session = query(&request, "wsTicket")
                .and_then(|t| state.ticket_session(&t, "media"))
                .or_else(|| bearer(&request).and_then(|t| state.session(t)));
            let Some(session) = session else {
                return Ok(unauthorized());
            };
            if let Some(reply) = scope_reply(&session, "orchestration:read") {
                return Ok(reply);
            }
            upgrade(state, session, request)
        }
        (_, p) if p.starts_with("/api/device-hub/") => {
            let session = query(&request, "wsTicket")
                .and_then(|t| state.ticket_session(&t, "media"))
                .or_else(|| bearer(&request).and_then(|t| state.session(t)));
            let Some(session) = session else {
                return Ok(unauthorized());
            };
            let socket = path == "/api/device-hub/vendor/serve-emu/ws";
            if let Some(r) = scope_reply(&session, "orchestration:operate") {
                return Ok(r);
            }
            let settings = crate::settings::read(&state).unwrap_or_default();
            if settings["enableDeviceSupport"] == false
                || settings["enableAgentDeviceAccess"] == false
            {
                return Ok(json_reply(StatusCode::FORBIDDEN, "{}".into()));
            }
            let target = json!({"deviceId":query(&request,"device").unwrap_or_default(),"hostId":query(&request,"hostId").unwrap_or_else(||"local".into())});
            let serial = match crate::device::id(&target) {
                Ok(s) => s.to_string(),
                Err(e) => return Ok(json_reply(StatusCode::BAD_REQUEST, e.to_string())),
            };
            if socket && request.method() == Method::GET {
                let Some(key) = request
                    .headers()
                    .get("sec-websocket-key")
                    .and_then(|v| v.to_str().ok())
                else {
                    return Ok(json_reply(StatusCode::BAD_REQUEST, "{}".into()));
                };
                let accept =
                    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
                let lease = request.extensions().get::<Lease>().cloned();
                let upgrade = hyper::upgrade::on(&mut request);
                let device_state = state.clone();
                let device_session = session.clone();
                tokio::spawn(async move {
                    let _lease = lease;
                    use futures_util::{SinkExt, StreamExt};
                    if let Ok(io) = upgrade.await {
                        let mut ws = WebSocketStream::from_raw_socket(
                            TokioIo::new(io),
                            Role::Server,
                            Some(
                                tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default(
                                )
                                .max_message_size(Some(4096))
                                .max_frame_size(Some(4096)),
                            ),
                        )
                        .await;
                        use tokio::io::AsyncReadExt;
                        let mut recorder: Option<tokio::process::Child> = None;
                        let mut video = vec![0u8; 64 * 1024];
                        let mut restarts: std::collections::VecDeque<std::time::Instant> =
                            Default::default();
                        let mut timer = tokio::time::interval(std::time::Duration::from_secs(2));
                        loop {
                            let settings = crate::settings::read(&device_state).unwrap_or_default();
                            if settings["enableDeviceSupport"]==false || settings["enableAgentDeviceAccess"]==false || device_session["sid"].as_str().is_none_or(|sid|!device_state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?)",rusqlite::params![sid,now_ms()],|r|r.get::<_,bool>(0)).unwrap_or(false)){break;}
                            tokio::select! {
                                _=timer.tick()=>{},
                                read=async {
                                    match recorder.as_mut().and_then(|p|p.stdout.as_mut()) {
                                        Some(out)=>out.read(&mut video).await,
                                        None=>std::future::pending().await,
                                    }
                                }=>{
                                    let Ok(n)=read else{break;};
                                    if n==0 {
                                        if let Some(mut child)=recorder.take(){let _=child.start_kill();let _=child.wait().await;}
                                        // Bytes before each exit must not reset the budget, or a failing recorder loops forever.
                                        restarts.retain(|t| t.elapsed() < std::time::Duration::from_secs(60));
                                        restarts.push_back(std::time::Instant::now());
                                        if restarts.len() > 3 { break; }
                                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                        match crate::device::recorder(&serial) {
                                            Ok(child)=>recorder=Some(child),
                                            Err(_)=>break,
                                        }
                                        continue;
                                    }
                                    let mut packet=vec![0u8;16];packet[..4].copy_from_slice(b"SEMU");packet[4]=1;packet[5]=1;
                                    packet.extend_from_slice(&video[..n]);
                                    let send=ws.send(tokio_tungstenite::tungstenite::Message::Binary(packet.into()));
                                    if !matches!(tokio::time::timeout(std::time::Duration::from_secs(2),send).await,Ok(Ok(()))){break;}
                                },
                                message=ws.next()=>{
                                    let Some(Ok(message))=message else{break;};
                                    if message.is_close(){break;}
                                    if let Ok(text)=message.to_text(){
                                        if text.len()>4096{break;}
                                        if let Ok(v)=serde_json::from_str::<Value>(text){
                                            if v["type"]=="reset-video" {
                                                if let Some(mut child)=recorder.take(){let _=child.start_kill();let _=child.wait().await;}
                                                match crate::device::recorder(&serial) {
                                                    Ok(child)=>recorder=Some(child),
                                                    Err(_)=>break,
                                                }
                                            }else if let Err(e)=crate::device::input(&serial,&v).await{
                                                let _=ws.send(tokio_tungstenite::tungstenite::Message::Text(e.to_string().into())).await;
                                            }
                                        }
                                    }
                                },
                            }
                        }
                        if let Some(mut child) = recorder {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                        }
                    }
                });
                Response::builder()
                    .status(StatusCode::SWITCHING_PROTOCOLS)
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade")
                    .header("sec-websocket-accept", accept)
                    .body(body::full(""))
                    .unwrap()
            } else if path == "/api/device-hub/vendor/serve-emu/api/screenshot"
                && request.method() == Method::POST
            {
                match crate::device::adb(&["-s", &serial, "exec-out", "screencap", "-p"]).await {
                    Ok(png) if png.starts_with(b"\x89PNG\r\n\x1a\n") => Response::builder()
                        .header("content-type", "image/png")
                        .header("cache-control", "no-store")
                        .body(body::full(png))
                        .unwrap(),
                    _ => json_reply(
                        StatusCode::BAD_GATEWAY,
                        json!({"reason":"ADB screenshot failed"}).to_string(),
                    ),
                }
            } else {
                json_reply(StatusCode::NOT_IMPLEMENTED,json!({"reason":"Only local Android screenshots and input are supported; H264 video and iOS are unavailable"}).to_string())
            }
        }
        (_, "/ws") => {
            if query(&request, "orchestrationProtocol").as_deref() != Some("2") {
                return Ok(protocol_error());
            }
            let ticket = query(&request, "wsTicket").and_then(|t| state.ticket_session(&t, "rpc"));
            match ticket.or_else(|| bearer(&request).and_then(|t| state.session(t))) {
                Some(session) => upgrade(state, session, request),
                None => unauthorized(),
            }
        }
        (Method::GET, "/api/auth/session") => {
            let mut result =
                json!({"authenticated":false,"auth":crate::settings::auth_descriptor()});
            if let Some(session) = bearer(&request).and_then(|t| state.session(t)) {
                result["authenticated"] = json!(true);
                result["scopes"] = session["scopes"].clone();
                result["permissions"] = session["scopes"].clone();
                result["sessionMethod"] = json!("bearer-access-token");
                result["expiresAt"] = json!(crate::store::iso(
                    session["exp"].as_i64().unwrap_or(now_ms())
                ));
            }
            json_reply(StatusCode::OK, result.to_string())
        }
        _ => {
            if path.starts_with("/api/orchestration/")
                && request
                    .headers()
                    .get("x-t3-orchestration-protocol")
                    .and_then(|v| v.to_str().ok())
                    != Some("2")
            {
                return Ok(protocol_error());
            }
            let Some(session) = bearer(&request).and_then(|t| state.session(t)) else {
                return Ok(unauthorized());
            };
            match (request.method().clone(), path.as_str()) {
                (Method::POST, "/api/auth/websocket-ticket") => {
                    websocket_ticket(&state, &session, None)
                }
                (Method::POST, "/api/auth/websocket-ticket/rpc") => {
                    websocket_ticket(&state, &session, Some("rpc"))
                }
                (Method::POST, "/api/auth/websocket-ticket/media") => {
                    websocket_ticket(&state, &session, Some("media"))
                }
                _ if path.starts_with("/api/auth/") => auth_route(&state, &session, request).await,
                (Method::GET, "/api/projects") => {
                    if let Some(r) = scope_reply(&session, "orchestration:read") {
                        return Ok(r);
                    }
                    result_reply(crate::projects::snapshot(&state))
                }
                (Method::POST, "/api/projects/mutate") => {
                    let Some(b) = body(request).await else {
                        return Ok(json_reply(StatusCode::BAD_REQUEST, "{}".into()));
                    };
                    let Ok(mutation) = serde_json::from_slice::<Value>(&b) else {
                        return Ok(json_reply(StatusCode::BAD_REQUEST, "{}".into()));
                    };
                    if let Err(e) = crate::rpc::authorize(&session, "projects.mutate", &mutation) {
                        return Ok(json_reply(StatusCode::FORBIDDEN, json!({"_tag":"EnvironmentScopeRequiredError","code":"insufficient_scope","requiredScope":e["requiredScope"],"traceId":crate::store::random_id().unwrap_or_default()}).to_string()));
                    }
                    match crate::projects::mutate_resolved(&state, &mutation).await {
                        Ok(project) => json_reply(StatusCode::OK, project.to_string()),
                        Err(_) => json_reply(StatusCode::BAD_REQUEST, json!({"_tag":"EnvironmentRequestInvalidError","code":"invalid_request","reason":"invalid_command","traceId":crate::store::random_id().unwrap_or_default()}).to_string()),
                    }
                }
                (Method::GET, "/api/orchestration/shell") => {
                    if let Some(r) = scope_reply(&session, "orchestration:read") {
                        return Ok(r);
                    }
                    stream_reply(&state, None, crate::snapshot::Options::default())
                }
                (Method::GET, p)
                    if p.starts_with("/api/orchestration/threads/") && p.ends_with("/history") =>
                {
                    if let Some(r) = scope_reply(&session, "orchestration:read") {
                        return Ok(r);
                    }
                    let tid = p
                        .trim_start_matches("/api/orchestration/threads/")
                        .trim_end_matches("/history");
                    let cursor = query(&request, "cursor").unwrap_or_default();
                    if cursor.parse::<i64>().is_err() {
                        return Ok(json_reply(
                            StatusCode::BAD_REQUEST,
                            serde_json::json!({"error":"invalid history cursor"}).to_string(),
                        ));
                    }
                    result_reply(crate::events::history(&state, tid, &cursor))
                }
                (Method::GET, p)
                    if p.starts_with("/api/orchestration/threads/") && p.ends_with("/bounded") =>
                {
                    if let Some(r) = scope_reply(&session, "orchestration:read") {
                        return Ok(r);
                    }
                    let tid = p
                        .trim_start_matches("/api/orchestration/threads/")
                        .trim_end_matches("/bounded");
                    stream_reply(
                        &state,
                        Some(tid.to_owned()),
                        crate::snapshot::Options {
                            bounded: true,
                            dedupe: query(&request, "dedupeBodies").as_deref() == Some("1"),
                            compact_items: query(&request, "compactTurnItems").as_deref()
                                == Some("1"),
                            compact_ids: query(&request, "compactIds").as_deref() == Some("1"),
                            resolved_roots: Vec::new(),
                        },
                    )
                }
                _ => json_reply(StatusCode::NOT_FOUND, r#"{"code":"not_found"}"#.into()),
            }
        }
    })
}

/// TCP and embedded transports use exactly this connection service, including upgrades.
pub async fn serve_io<I>(io: I, state: Arc<State>) -> std::io::Result<()>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    serve_connection(io, state, None, None).await
}
async fn serve_connection<I>(
    io: I,
    state: Arc<State>,
    peer: Option<tokio::sync::OwnedSemaphorePermit>,
    address: Option<std::net::IpAddr>,
) -> std::io::Result<()>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let permit = state
        .http
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(|_| std::io::Error::other("connection limit"))?;
    let lease = Arc::new((permit, peer));
    state.start_checkpointer();
    crate::scheduled::ensure_started(&state);
    crate::projects::ensure_identity_watcher(&state);
    let service = service_fn(move |mut request: Request<Incoming>| {
        request.extensions_mut().insert(lease.clone());
        if let Some(ip) = address {
            request.extensions_mut().insert(ip);
        }
        route(state.clone(), request)
    });
    hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(5))
        .max_buf_size(32 * 1024)
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades()
        .await
        .map_err(std::io::Error::other)
}
pub async fn serve(listener: TcpListener, state: Arc<State>) -> std::io::Result<()> {
    state.native_mcp.listen(listener.local_addr()?);
    crate::checkpoint::recover_rollbacks(&state).map_err(std::io::Error::other)?;
    crate::recovery::recover(&mut state.db.lock().unwrap()).map_err(std::io::Error::other)?;
    state
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA shrink_memory")
        .map_err(std::io::Error::other)?;
    crate::recovery::resume(&state).map_err(std::io::Error::other)?;
    crate::heap::collect();
    state.start_checkpointer();
    crate::scheduled::ensure_started(&state);
    crate::projects::ensure_identity_watcher(&state);
    loop {
        let (socket, address) = listener.accept().await?;
        let peer = {
            let mut peers = state.http.peers.lock().unwrap();
            peers.retain(|_, v| v.strong_count() > 0);
            let semaphore = peers
                .get(&address.ip())
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let sem = Arc::new(tokio::sync::Semaphore::new(16));
                    peers.insert(address.ip(), Arc::downgrade(&sem));
                    sem
                });
            match semaphore.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => continue,
            }
        };
        let _ = socket.set_nodelay(true);
        let state = state.clone();
        tokio::spawn(async move {
            let _ = serve_connection(socket, state, Some(peer), Some(address.ip())).await;
            crate::heap::collect();
        });
    }
}

fn protocol_error() -> Reply {
    json_reply(StatusCode::UPGRADE_REQUIRED,json!({"code":"orchestration_protocol_incompatible","message":"Update this client to one that supports orchestration protocol 2.","orchestrationProtocolVersion":2}).to_string())
}
fn result_reply(v: Result<Value, String>) -> Reply {
    match v {Ok(v)=>json_reply(StatusCode::OK,v.to_string()),Err(_)=>json_reply(StatusCode::NOT_FOUND,json!({"_tag":"EnvironmentNotFoundError","code":"not_found","reason":"thread_not_found","traceId":crate::store::random_id().unwrap_or_default()}).to_string())}
}
fn scope_reply(session: &Value, scope: &str) -> Option<Reply> {
    if session["scopes"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == scope))
    {
        None
    } else {
        Some(json_reply(StatusCode::FORBIDDEN,json!({"_tag":"EnvironmentScopeRequiredError","code":"insufficient_scope","requiredScope":scope,"traceId":crate::store::random_id().unwrap_or_default()}).to_string()))
    }
}
async fn auth_route(s: &State, session: &Value, request: Request<Incoming>) -> Reply {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let scope = if method == Method::GET {
        "access:read"
    } else {
        "access:write"
    };
    if let Some(r) = scope_reply(session, scope) {
        return r;
    }
    let p: Value = if method == Method::POST {
        let Some(b) = body(request).await else {
            return json_reply(StatusCode::BAD_REQUEST, "{}".into());
        };
        match if b.is_empty() {
            Ok(json!({}))
        } else {
            serde_json::from_slice(&b)
        } {
            Ok(v) => v,
            Err(_) => return json_reply(StatusCode::BAD_REQUEST, "{}".into()),
        }
    } else {
        json!({})
    };
    match (method, path.as_str()) {
        (Method::POST, "/api/auth/pairing-token") => {
            if p.get("scopes").is_some()
                && !p["scopes"].as_array().is_some_and(|a| {
                    !a.is_empty()
                        && a.iter().all(|v| v.is_string())
                        && a.iter().enumerate().all(|(i, v)| !a[..i].contains(v))
                })
            {
                return json_reply(StatusCode::BAD_REQUEST,json!({"_tag":"EnvironmentRequestInvalidError","code":"invalid_request","reason":"invalid_scopes","traceId":crate::store::random_id().unwrap_or_default()}).to_string());
            }
            if let Some(scopes) = p["scopes"].as_array() {
                for scope in scopes {
                    if let Some(r) = scope_reply(session, scope.as_str().unwrap_or("")) {
                        return r;
                    }
                }
            }
            let scopes = p
                .get("scopes")
                .unwrap_or(&session["scopes"])
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            match s.issue_pairing_scoped(
                p["label"].as_str(),
                300000,
                &scopes,
                session["sub"].as_str().unwrap_or("client"),
            ) {
                Ok(v) => json_reply(StatusCode::OK, v.to_string()),
                Err(_) => json_reply(StatusCode::INTERNAL_SERVER_ERROR, "{}".into()),
            }
        }

        (Method::GET, "/api/auth/clients") => {
            let db = s.db.lock().unwrap();
            let mut q=db.prepare("SELECT session_id,subject,scopes,client,issued_at,expires_at FROM auth_sessions WHERE revoked_at IS NULL AND expires_at>?").unwrap();
            let rows=q.query_map([now_ms()],|r| {let sid:String=r.get(0)?;let scopes:String=r.get(2)?;let c:String=r.get(3)?;let c:Value=serde_json::from_str(&c).unwrap_or(json!({}));Ok(json!({"sessionId":sid,"subject":r.get::<_,String>(1)?,"scopes":scopes.split_whitespace().collect::<Vec<_>>(),"client":{"label":c["client_label"],"deviceType":c.get("client_device_type").cloned().unwrap_or(json!("unknown")),"os":c["client_os"]},"current":session["sid"]==sid,"method":"bearer-access-token","connected":false,"lastConnectedAt":null,"issuedAt":crate::store::iso(r.get(4)?),"expiresAt":crate::store::iso(r.get(5)?)}))}).unwrap();
            let clients: Vec<_> = rows.filter_map(Result::ok).collect();
            json_reply(StatusCode::OK, json!(clients).to_string())
        }
        (Method::GET, "/api/auth/pairing-links") => {
            let db = s.db.lock().unwrap();
            let mut q=db.prepare("SELECT id,label,created_at,expires_at,scopes,subject FROM auth_pairing_links WHERE revoked_at IS NULL AND consumed_at IS NULL AND expires_at>?").unwrap();
            let rows=q.query_map([now_ms()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"label":r.get::<_,Option<String>>(1)?,"createdAt":crate::store::iso(r.get(2)?),"expiresAt":crate::store::iso(r.get(3)?),"scopes":r.get::<_,String>(4)?.split_whitespace().collect::<Vec<_>>(),"subject":r.get::<_,String>(5)?}))).unwrap();
            let links: Vec<_> = rows.filter_map(Result::ok).collect();
            json_reply(StatusCode::OK, json!(links).to_string())
        }
        (Method::POST, "/api/auth/clients/revoke") => {
            if p["sessionId"] == session["sid"] {
                return json_reply(
                    StatusCode::FORBIDDEN,
                    json!({"code":"operation_forbidden"}).to_string(),
                );
            }
            result_reply(
                s.revoke_session(p["sessionId"].as_str().unwrap_or(""))
                    .map(|revoked| json!({"revoked":revoked})),
            )
        }
        (Method::POST, "/api/auth/clients/revoke-others") => {
            let n=s.db.lock().unwrap().execute("UPDATE auth_sessions SET revoked_at=? WHERE session_id!=? AND revoked_at IS NULL",rusqlite::params![now_ms(),session["sid"].as_str()]).unwrap_or(0);
            s.notify();
            json_reply(StatusCode::OK, json!({"revokedCount":n}).to_string())
        }
        (Method::POST, "/api/auth/pairing-links/revoke") => {
            let n = s
                .db
                .lock()
                .unwrap()
                .execute(
                    "UPDATE auth_pairing_links SET revoked_at=? WHERE id=? AND revoked_at IS NULL",
                    rusqlite::params![now_ms(), p["id"].as_str()],
                )
                .unwrap_or(0);
            json_reply(StatusCode::OK, json!({"revoked":n>0}).to_string())
        }
        _ => json_reply(StatusCode::NOT_FOUND, "{}".into()),
    }
}

fn stream_reply(s: &State, tid: Option<String>, opts: crate::snapshot::Options) -> Reply {
    match crate::snapshot::start(s, tid, opts, None) {
        Err(e) => result_reply(Err(e)),
        Ok((_, rx)) => {
            let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|part| {
                    (
                        part.map(|b| hyper::body::Frame::data(Bytes::from(b)))
                            .map_err(std::io::Error::other),
                        rx,
                    )
                })
            });
            let mut r = Response::new(http_body_util::StreamBody::new(stream).boxed_unsync());
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            r
        }
    }
}
