//! HTTP routes and the WebSocket upgrade, on bare hyper to stay small.
use crate::rpc;
use base64::Engine;
use http_body_util::Full;
use hyper::{body::{Bytes, Incoming}, header, service::service_fn, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use sha1::{Digest, Sha1};
use std::convert::Infallible;
use tokio::net::TcpListener;
use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

type Reply = Response<Full<Bytes>>;

fn json_reply(status: StatusCode, body: String) -> Reply {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
    response
}

fn descriptor() -> String {
    json!({
        "environmentId": std::env::var("O3_ENVIRONMENT_ID").unwrap_or_else(|_| "o3-dev".into()),
        "label": "o3-server",
        "platform": { "os": std::env::consts::OS, "arch": "x64", "machine": "server" },
        "serverVersion": rpc::SERVER_VERSION,
        "orchestrationProtocolVersion": 2,
        "capabilities": {}
    }).to_string()
}

fn upgrade(mut request: Request<Incoming>) -> Reply {
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

async fn route(request: Request<Incoming>) -> Result<Reply, Infallible> {
    Ok(match request.uri().path() {
        "/.well-known/t3/environment" => json_reply(StatusCode::OK, descriptor()),
        "/ws" => upgrade(request),
        _ => json_reply(StatusCode::NOT_FOUND, r#"{"error":"not_found"}"#.into()),
    })
}

pub async fn serve(listener: TcpListener) -> std::io::Result<()> {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        loop {
            let (socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            tokio::task::spawn_local(async move {
                let connection = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(socket), service_fn(route))
                    .with_upgrades();
                let _ = connection.await;
            });
        }
    }).await
}
