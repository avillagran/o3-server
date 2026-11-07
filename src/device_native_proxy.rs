//! Authenticated media forwarding to the currently owned loopback device hub.
use crate::{
    body::{self, Body},
    store::State,
};
use http_body_util::{BodyExt, Limited, StreamBody};
use hyper::{
    body::{Frame, Incoming},
    Request, Response, StatusCode,
};
use regex::Regex;
use serde_json::Value;
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

static READ: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
 r"^/api/devices$",r"^/vendor/serve-sim/api$",r"^/vendor/serve-sim/api/screenshot$",r"^/vendor/serve-sim/api/event-log(/events)?$",r"^/vendor/serve-sim/helper/[^/]+/(stream\.mjpeg|stream\.avcc|config|health|ax|foreground)$",r"^/vendor/serve-sim/helper/[^/]+/panel/(1|3)/stream\.avcc$",r"^/vendor/serve-sim/helper/[^/]+/webrtc/(offer|close)$",r"^/vendor/serve-sim/appstate$",r"^/vendor/serve-emu/api$",r"^/vendor/serve-emu/api/(devices|screenshot|stream-mode|stream-settings|accessibility|fold)$",r"^/vendor/serve-emu/health$"
].map(|pattern|Regex::new(pattern).expect("native proxy path pattern")).into()
});
static MUTABLE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^/vendor/serve-sim/api/screenshot$",
        r"^/vendor/serve-sim/helper/[^/]+/webrtc/(offer|close)$",
        r"^/vendor/serve-emu/webrtc/(offer|close)$",
        r"^/vendor/serve-emu/api/(screenshot|stream-mode|stream-settings)$",
        r"^/vendor/serve-emu/api/fold$",
    ]
    .map(|pattern| Regex::new(pattern).expect("native proxy mutable pattern"))
    .into()
});
static SOCKETS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^/api/devices/ws$",
        r"^/vendor/serve-sim/helper/ws$",
        r"^/vendor/serve-emu/ws$",
    ]
    .map(|pattern| Regex::new(pattern).expect("native proxy websocket pattern"))
    .into()
});
#[derive(Clone, Debug)]
pub struct Plan {
    pub path: String,
    pub search: String,
    pub socket: bool,
    pub controls: bool,
}
pub fn plan(uri: &str, method: &str, socket: bool) -> Result<Plan, u16> {
    let url = reqwest::Url::parse(&format!("http://device.invalid{uri}")).map_err(|_| 400_u16)?;
    let path = url
        .path()
        .strip_prefix("/api/device-hub")
        .filter(|path| path.starts_with('/'))
        .ok_or(404_u16)?;
    let matches = |patterns: &[Regex]| patterns.iter().any(|pattern| pattern.is_match(path));
    if !(if socket {
        matches(&SOCKETS)
    } else {
        matches(&READ) || matches(&MUTABLE)
    }) {
        return Err(404);
    }
    let read = method == "GET" || method == "HEAD";
    if !socket && !matches(if read { &READ } else { &MUTABLE }) {
        return Err(405);
    }
    let mut search = reqwest::Url::parse("http://device.invalid").expect("static URL");
    {
        let mut pairs = search.query_pairs_mut();
        for (key, value) in url.query_pairs() {
            if key != "hostId" && key != "wsTicket" {
                pairs.append_pair(&key, &value);
            }
        }
    }
    let query = search
        .query()
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let controls = (socket && path != "/api/devices/ws")
        || (!read
            && (path.ends_with("/api/stream-mode")
                || path.ends_with("/api/stream-settings")
                || path.ends_with("/api/fold")));
    Ok(Plan {
        path: path.into(),
        search: query,
        socket,
        controls,
    })
}
fn dropped(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "connection"
            | "upgrade"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "sec-websocket-extensions"
            | "sec-websocket-protocol"
            | "cookie"
            | "authorization"
            | "dpop"
            | "content-length"
            | "accept-encoding"
            | "x-forwarded-proto"
            | "proxy-authorization"
            | "proxy-connection"
            | "transfer-encoding"
            | "te"
            | "trailer"
    )
}
pub fn forward_headers(
    source: &hyper::HeaderMap,
    origin: &str,
) -> Result<hyper::HeaderMap, hyper::header::InvalidHeaderValue> {
    let mut headers = hyper::HeaderMap::new();
    for (name, value) in source {
        if !dropped(name.as_str()) {
            headers.append(name.clone(), value.clone());
        }
    }
    if headers.contains_key("origin") {
        headers.insert("origin", origin.parse()?);
    }
    Ok(headers)
}
fn failure(status: u16, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("cache-control", "no-store")
        .body(body::full(message.to_owned()))
        .expect("static response")
}
fn valid(state: &State, session: &Value) -> bool {
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return false;
    }
    let Ok(settings) = crate::settings::read(state) else {
        return false;
    };
    if settings["enableDeviceSupport"] == false {
        return false;
    }
    session["sid"].as_str().is_some_and(|sid|state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?)",rusqlite::params![sid,crate::store::now_ms()],|row|row.get::<_,bool>(0)).unwrap_or(false))
}
pub async fn serve(
    mut request: Request<Incoming>,
    origin: String,
    state: Arc<State>,
    session: Value,
) -> Response<Body> {
    let connection_lease = request.extensions().get::<crate::http::Lease>().cloned();
    let socket = request
        .headers()
        .get("upgrade")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let plan = match plan(
        &request.uri().to_string(),
        request.method().as_str(),
        socket,
    ) {
        Ok(plan) => plan,
        Err(status) => return failure(status, "Device hub path or method is unavailable"),
    };
    // Origin comes only from the runtime owner. Revalidate here before opening any transport.
    let Ok(base) = reqwest::Url::parse(&origin) else {
        return failure(503, "Device hub is not running");
    };
    if base.scheme() != "http"
        || !base.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
        })
        || !base.username().is_empty()
        || base.password().is_some()
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return failure(503, "Invalid owned device hub origin");
    }
    let upstream = format!("{origin}{}{}", plan.path, plan.search);
    let headers = match forward_headers(request.headers(), &origin) {
        Ok(headers) => headers,
        Err(_) => return failure(503, "Invalid owned origin"),
    };
    if socket {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let Some(key) = request
            .headers()
            .get("sec-websocket-key")
            .and_then(|key| key.to_str().ok())
        else {
            return failure(400, "Missing websocket key");
        };
        let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
        let mut upstream_request = match upstream.replacen("http:", "ws:", 1).into_client_request()
        {
            Ok(request) => request,
            Err(_) => return failure(502, "Invalid hub websocket"),
        };
        for (name, value) in &headers {
            upstream_request
                .headers_mut()
                .insert(name.clone(), value.clone());
        }
        let connect = async {
            let stream = tokio::net::TcpStream::connect((
                base.host_str()
                    .unwrap_or("127.0.0.1")
                    .trim_matches(['[', ']']),
                base.port().unwrap_or(80),
            ))
            .await
            .map_err(std::io::Error::other)?;
            tokio_tungstenite::client_async_with_config(
                upstream_request,
                stream,
                Some(
                    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(16 * 1024 * 1024))
                        .max_frame_size(Some(16 * 1024 * 1024)),
                ),
            )
            .await
            .map_err(std::io::Error::other)
        };
        let mut upstream = match tokio::time::timeout(Duration::from_secs(10), connect).await {
            Ok(Ok((socket, _))) => socket,
            _ => return failure(502, "Device hub websocket did not connect"),
        };
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(async move {
            let _connection_lease = connection_lease;
            let Ok(io) = upgrade.await else {
                return;
            };
            let mut client = tokio_tungstenite::WebSocketStream::from_raw_socket(
                hyper_util::rt::TokioIo::new(io),
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                Some(
                    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(16 * 1024 * 1024))
                        .max_frame_size(Some(16 * 1024 * 1024)),
                ),
            )
            .await;
            let mut timer = tokio::time::interval(Duration::from_secs(2));
            loop {
                if !valid(&state, &session) {
                    break;
                }
                tokio::select! {
                 _=timer.tick()=>{},
                 message=client.next()=>{let Some(Ok(message))=message else{break;};if !matches!(tokio::time::timeout(Duration::from_secs(2),upstream.send(message)).await,Ok(Ok(()))){break;}},
                 message=upstream.next()=>{let Some(Ok(message))=message else{break;};if !matches!(tokio::time::timeout(Duration::from_secs(2),client.send(message)).await,Ok(Ok(()))){break;}},
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), upstream.close(None)).await;
        });
        return Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header("upgrade", "websocket")
            .header("connection", "Upgrade")
            .header("sec-websocket-accept", accept)
            .body(body::empty())
            .expect("websocket response");
    }
    let method = request.method().clone();
    let payload = if method == hyper::Method::GET || method == hyper::Method::HEAD {
        Vec::new()
    } else {
        match tokio::time::timeout(
            Duration::from_secs(10),
            Limited::new(request.into_body(), 1024 * 1024).collect(),
        )
        .await
        {
            Ok(Ok(body)) => body.to_bytes().to_vec(),
            _ => return failure(413, "Device request body exceeded its limit"),
        }
    };
    let http = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(http) => http,
        Err(_) => return failure(502, "Device hub transport unavailable"),
    };
    let response = match tokio::time::timeout(
        Duration::from_secs(20),
        http.request(method, upstream)
            .headers(headers)
            .body(payload)
            .send(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => return failure(502, "Device hub did not answer"),
    };
    let status = response.status();
    let mut output = Response::builder().status(status);
    for (name, value) in response.headers() {
        if !matches!(
            name.as_str(),
            "content-encoding"
                | "transfer-encoding"
                | "connection"
                | "set-cookie"
                | "content-length"
        ) {
            output = output.header(name, value);
        }
    }
    output.headers_mut().expect("response builder").insert(
        "cache-control",
        hyper::header::HeaderValue::from_static("no-store, no-transform"),
    );
    let continuous = plan.path.ends_with(".mjpeg")
        || plan.path.ends_with(".avcc")
        || plan.path.ends_with("event-log/events");
    let limit = if continuous {
        None
    } else {
        Some(if plan.path.ends_with("/screenshot") {
            64 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        })
    };
    if plan.path.ends_with("/screenshot") {
        output.headers_mut().expect("response builder").insert(
            "content-type",
            hyper::header::HeaderValue::from_static("image/png"),
        );
    }
    let stream = futures_util::stream::unfold((response, limit), move |(mut response, left)| {
        let state = state.clone();
        let session = session.clone();
        async move {
            if left == Some(0) || !valid(&state, &session) {
                return None;
            }
            match tokio::time::timeout(Duration::from_secs(20), response.chunk()).await {
                Ok(Ok(Some(bytes))) => {
                    if left.is_some_and(|left| bytes.len() > left) {
                        return Some((
                            Err(std::io::Error::other(
                                "Device response exceeded its size limit",
                            )),
                            (response, Some(0)),
                        ));
                    }
                    let remaining = left.map(|left| left - bytes.len());
                    Some((Ok(Frame::data(bytes)), (response, remaining)))
                }
                Ok(Ok(None)) => None,
                _ => Some((
                    Err(std::io::Error::other(
                        "Device response interrupted or timed out",
                    )),
                    (response, Some(0)),
                )),
            }
        }
    });
    output
        .body(StreamBody::new(stream).boxed_unsync())
        .expect("valid upstream response headers")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_native_proxy_routing_and_header_decisions() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-proxy-4288.json"
        ))
        .unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let expected = &case["expected"];
            let status = expected["status"].as_u64().unwrap();
            let result = plan(
                input["uri"].as_str().unwrap(),
                input["method"].as_str().unwrap(),
                false,
            );
            if [404, 405].contains(&status) {
                assert_eq!(result.err(), Some(status as u16), "case {index}");
                continue;
            }
            let plan = result.unwrap();
            if input["scopes"] == serde_json::json!(["orchestration:read"]) {
                assert_eq!(plan.controls, status == 403, "case {index}");
            }
            if status == 200 {
                assert_eq!(
                    format!("http://hub.test{}{}", plan.path, plan.search),
                    expected["requests"][0]["url"],
                    "case {index}"
                );
                let mut headers = hyper::HeaderMap::new();
                for (name, value) in input["headers"].as_object().unwrap() {
                    headers.insert(
                        hyper::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                        value.as_str().unwrap().parse().unwrap(),
                    );
                }
                let actual = forward_headers(&headers, "http://hub.test").unwrap();
                let actual = serde_json::to_value(
                    actual
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.to_str().unwrap()))
                        .collect::<std::collections::BTreeMap<_, _>>(),
                )
                .unwrap();
                let mut expected_headers = expected["requests"][0]["headers"].clone();
                for key in ["b3", "traceparent", "tracestate"] {
                    expected_headers.as_object_mut().unwrap().remove(key);
                }
                assert_eq!(actual, expected_headers, "case {index}");
            }
        }
    }
    #[tokio::test]
    async fn upgraded_media_retains_and_releases_the_existing_connection_lease() {
        exercise_viewer_socket(None).await;
    }
    #[tokio::test]
    async fn human_viewer_socket_closes_when_device_support_is_disabled() {
        exercise_viewer_socket(Some(false)).await;
    }
    #[tokio::test]
    async fn human_viewer_socket_closes_when_its_session_is_revoked() {
        exercise_viewer_socket(Some(true)).await;
    }
    async fn exercise_viewer_socket(close: Option<bool>) {
        use futures_util::{SinkExt, StreamExt};
        crate::test_sandbox::assert_isolated();
        let directory = std::env::temp_dir().join(format!(
            "private-media-lease-{}",
            crate::store::random_id().unwrap()
        ));
        let state = Arc::new(crate::store::State::open(&directory).unwrap());
        crate::settings::update(&state,&serde_json::json!({"patch":{"enableDeviceSupport":true,"enableAgentDeviceAccess":false}})).unwrap();
        let token = state
            .issue_session(
                "private-lease",
                &["orchestration:operate".into()],
                60_000,
                "Private fixture",
            )
            .unwrap();
        let session = state.session(&token).unwrap();
        let lifecycle_state = state.clone();
        let session_id = session["sid"].as_str().unwrap().to_owned();
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", upstream.local_addr().unwrap());
        let echo = tokio::spawn(async move {
            let (socket, _) = upstream.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                if message.is_close() {
                    break;
                }
                if socket.send(message).await.is_err() {
                    break;
                }
            }
        });
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let lease: crate::http::Lease = Arc::new((
            permits.clone().acquire_owned().await.unwrap(),
            None::<tokio::sync::OwnedSemaphorePermit>,
        ));
        let (client, server) = tokio::io::duplex(64 * 1024);
        let connection = tokio::spawn(async move {
            let service = hyper::service::service_fn(move |mut request: Request<Incoming>| {
                request.extensions_mut().insert(lease.clone());
                let state = state.clone();
                let session = session.clone();
                let origin = origin.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(serve(request, origin, state, session).await)
                }
            });
            hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(server), service)
                .with_upgrades()
                .await
                .unwrap();
        });
        let (mut socket, _) = tokio_tungstenite::client_async(
            "ws://localhost/api/device-hub/vendor/serve-sim/helper/ws",
            client,
        )
        .await
        .unwrap();
        connection.await.unwrap();
        assert_eq!(
            permits.available_permits(),
            0,
            "An upgraded media connection must retain its admission lease"
        );
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "private-lease".into(),
            ))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().to_text().unwrap(),
            "private-lease"
        );
        if let Some(revoke) = close {
            if revoke {
                lifecycle_state
                    .db
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE auth_sessions SET revoked_at=? WHERE session_id=?",
                        rusqlite::params![crate::store::now_ms(), session_id],
                    )
                    .unwrap();
            } else {
                crate::settings::update(
                    &lifecycle_state,
                    &serde_json::json!({"patch":{"enableDeviceSupport":false}}),
                )
                .unwrap();
            }
            let closed = tokio::time::timeout(Duration::from_secs(4), socket.next())
                .await
                .expect("A revoked or disabled viewer must close");
            assert!(
                closed.is_none()
                    || closed.as_ref().is_some_and(|value| value
                        .as_ref()
                        .map_or(true, |message| message.is_close()))
            );
        } else {
            socket.close(None).await.unwrap();
        }
        drop(socket);
        tokio::time::timeout(Duration::from_secs(3), async {
            while permits.available_permits() != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        echo.await.unwrap();
    }
}
