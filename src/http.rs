//! HTTP routes and the WebSocket upgrade, on bare hyper to stay small.
use crate::{
    auth, rpc,
    store::{now_ms, State},
};
use base64::Engine;
use crate::body::{self, Body};
use http_body_util::{BodyExt, Limited};
use hyper::{
    body::{Bytes, Incoming},
    header,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{convert::Infallible, sync::Arc};
use tokio::net::TcpListener;
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

type Reply = Response<Body>;
const AUTH_BODY_LIMIT: usize = 64 * 1024;

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
    Limited::new(request.into_body(), AUTH_BODY_LIMIT)
        .collect()
        .await
        .ok()
        .map(|b| b.to_bytes())
}

/// `POST /oauth/token`: RFC 8693 exchange of a pairing secret for a bearer session.
async fn token(state: &State, request: Request<Incoming>) -> Reply {
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
    let upgrading = hyper::upgrade::on(&mut request);
    tokio::task::spawn(async move {
        if let Ok(upgraded) = upgrading.await {
            rpc::run(
                WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    Role::Server,
                    Some(
                        tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                            .max_message_size(Some(8 * 1024 * 1024))
                            .max_frame_size(Some(8 * 1024 * 1024)),
                    ),
                )
                .await,
                state,
                session,
            )
            .await;
        }
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

async fn route(state: Arc<State>, request: Request<Incoming>) -> Result<Reply, Infallible> {
    let path = request.uri().path().to_owned();
    Ok(match (request.method().clone(), path.as_str()) {
        (Method::GET, "/.well-known/t3/environment") => json_reply(
            StatusCode::OK,
            crate::settings::descriptor(&state).to_string(),
        ),
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
                    match crate::projects::mutate(&state, &mutation) {
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
                    result_reply(crate::events::history(
                        &state,
                        tid,
                        &query(&request, "cursor").unwrap_or_default(),
                    ))
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
    crate::scheduled::ensure_started(&state);
    let service = service_fn(move |request| route(state.clone(), request));
    hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades()
        .await
        .map_err(std::io::Error::other)
}
pub async fn serve(listener: TcpListener, state: Arc<State>) -> std::io::Result<()> {
    crate::recovery::recover(&mut state.db.lock().unwrap()).map_err(std::io::Error::other)?;
    crate::scheduled::ensure_started(&state);
    loop {
        let (socket, _) = listener.accept().await?;
        let _ = socket.set_nodelay(true);
        let state = state.clone();
        tokio::spawn(async move {
            let _ = serve_io(socket, state).await;
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
            match s.issue_pairing(p["label"].as_str(), 300000) {
                Ok(v) => {
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
                    let _ = s.db.lock().unwrap().execute(
                        "UPDATE auth_pairing_links SET scopes=? WHERE id=?",
                        rusqlite::params![scopes, v["id"].as_str()],
                    );
                    json_reply(StatusCode::OK, v.to_string())
                }
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
