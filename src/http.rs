//! HTTP routes and the WebSocket upgrade, on bare hyper to stay small.
use crate::{auth, rpc, store::{now_ms, State}};
use base64::Engine;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{body::{Bytes, Incoming}, header, service::service_fn, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{convert::Infallible, rc::Rc};
use tokio::net::TcpListener;
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

type Reply = Response<Full<Bytes>>;
const AUTH_BODY_LIMIT: usize = 64 * 1024;

fn json_reply(status: StatusCode, body: String) -> Reply {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
    response
}

fn unauthorized() -> Reply {
    json_reply(StatusCode::UNAUTHORIZED, json!({ "code": "auth_invalid", "reason": "invalid_credential" }).to_string())
}

fn descriptor(state: &State) -> String {
    json!({
        "environmentId": state.environment_id,
        "label": state.label,
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "machine": "server" },
        "serverVersion": rpc::SERVER_VERSION,
        "orchestrationProtocolVersion": 2,
        "capabilities": {}
    }).to_string()
}

fn bearer(request: &Request<Incoming>) -> Option<&str> {
    request.headers().get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

fn query(request: &Request<Incoming>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(request.uri().query()?.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

async fn body(request: Request<Incoming>) -> Option<Bytes> {
    Limited::new(request.into_body(), AUTH_BODY_LIMIT).collect().await.ok().map(|b| b.to_bytes())
}

/// `POST /oauth/token`: RFC 8693 exchange of a pairing secret for a bearer session.
async fn token(state: &State, request: Request<Incoming>) -> Reply {
    let Some(form) = body(request).await else { return json_reply(StatusCode::BAD_REQUEST, r#"{"code":"invalid_request"}"#.into()) };
    let mut credential = None;
    let mut client = json!({});
    for (name, value) in url::form_urlencoded::parse(&form) {
        match name.as_ref() {
            "subject_token" => credential = Some(value.into_owned()),
            "client_label" | "client_device_type" | "client_os" => client[name.as_ref()] = value.as_ref().into(),
            _ => {}
        }
    }
    let Some(credential) = credential else { return json_reply(StatusCode::BAD_REQUEST, r#"{"code":"invalid_request"}"#.into()) };
    match state.exchange_pairing(&credential, &client) {
        Ok(Some(result)) => json_reply(StatusCode::OK, result.to_string()),
        Ok(None) => unauthorized(),
        Err(_) => json_reply(StatusCode::INTERNAL_SERVER_ERROR, r#"{"code":"internal_error"}"#.into()),
    }
}

fn websocket_ticket(state: &State, session: &Value, usage: Option<&str>) -> Reply {
    let now = now_ms();
    let ticket = auth::websocket_ticket(session, &state.signing_key, now, usage);
    json_reply(StatusCode::OK, json!({ "ticket": ticket, "expiresAt": crate::store::iso(now + auth::WEBSOCKET_TTL_MS) }).to_string())
}

fn upgrade(_state: Rc<State>, _session: Value, mut request: Request<Incoming>) -> Reply {
    let Some(key) = request.headers().get(header::SEC_WEBSOCKET_KEY).cloned() else {
        return json_reply(StatusCode::BAD_REQUEST, r#"{"error":"websocket_required"}"#.into());
    };
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(hash.finalize());
    let upgrading = hyper::upgrade::on(&mut request);
    tokio::task::spawn_local(async move {
        if let Ok(upgraded) = upgrading.await {
            rpc::run(WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await).await;
        }
    });
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(header::CONNECTION, header::HeaderValue::from_static("Upgrade"));
    headers.insert(header::UPGRADE, header::HeaderValue::from_static("websocket"));
    headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept.parse().expect("base64 is a valid header"));
    response
}

async fn route(state: Rc<State>, request: Request<Incoming>) -> Result<Reply, Infallible> {
    let path = request.uri().path().to_owned();
    Ok(match (request.method().clone(), path.as_str()) {
        (Method::GET, "/.well-known/t3/environment") => json_reply(StatusCode::OK, descriptor(&state)),
        (Method::POST, "/oauth/token") => token(&state, request).await,
        (_, "/ws") => {
            let ticket = query(&request, "wsTicket").and_then(|t| state.ticket_session(&t, "rpc"));
            match ticket.or_else(|| bearer(&request).and_then(|t| state.session(t))) {
                Some(session) => upgrade(state, session, request),
                None => unauthorized(),
            }
        }
        _ => {
            let Some(session) = bearer(&request).and_then(|t| state.session(t)) else { return Ok(unauthorized()) };
            match (request.method().clone(), path.as_str()) {
                (Method::POST, "/api/auth/websocket-ticket") => websocket_ticket(&state, &session, None),
                _ => json_reply(StatusCode::NOT_FOUND, r#"{"code":"not_found"}"#.into()),
            }
        }
    })
}

pub async fn serve(listener: TcpListener, state: Rc<State>) -> std::io::Result<()> {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        loop {
            let (socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            let state = state.clone();
            tokio::task::spawn_local(async move {
                let service = service_fn(move |request| route(state.clone(), request));
                let connection = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(socket), service)
                    .with_upgrades();
                let _ = connection.await;
            });
        }
    }).await
}
