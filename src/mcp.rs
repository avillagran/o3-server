//! Streamable HTTP MCP, protocol 2025-06-18. Authentication precedes parsing and every call.
use crate::{
    body::{self, Body},
    store::{now_ms, random_id, State},
};
use http_body_util::{BodyExt, Limited};
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
const PROTOCOL: &str = "2025-06-18";
const IDLE_MS: i64 = 30 * 60 * 1000;
const BODY_LIMIT: usize = 1024 * 1024;
pub(crate) struct Sessions {
    sessions: Mutex<HashMap<String, Session>>,
    calls: Arc<tokio::sync::Semaphore>,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            calls: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
}
struct Session {
    owner: String,
    touched: i64,
}
fn response(code: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(
            if code == StatusCode::ACCEPTED || code == StatusCode::NO_CONTENT {
                body::empty()
            } else {
                body::full(value.to_string())
            },
        )
        .unwrap()
}
fn rpc_error(code: StatusCode, id: &Value, number: i64, message: &str) -> Response<Body> {
    response(
        code,
        json!({"jsonrpc":"2.0","id":id,"error":{"code":number,"message":message}}),
    )
}
fn header<'a>(r: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    r.headers().get(name)?.to_str().ok()
}
pub(crate) async fn handle(state: Arc<State>, req: Request<Incoming>) -> Response<Body> {
    let token = header(&req, "authorization")
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim();
    let Some(claims) = state.mcp_session(token) else {
        let mut r = response(
            StatusCode::UNAUTHORIZED,
            json!({"error":"invalid_mcp_credential","message":"A valid T3 Code MCP credential is required."}),
        );
        r.headers_mut().insert(
            "www-authenticate",
            if token.is_empty() {
                "Bearer"
            } else {
                "Bearer error=\"invalid_token\""
            }
            .parse()
            .unwrap(),
        );
        return r;
    };
    if req.headers().contains_key("origin") {
        return response(StatusCode::FORBIDDEN, json!({"error":"origin_not_allowed"}));
    }
    if header(&req, "mcp-protocol-version").is_some_and(|v| v != PROTOCOL) {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            &Value::Null,
            -32600,
            "Unsupported MCP protocol version",
        );
    }
    let owner = claims["sid"].as_str().unwrap_or("");
    let session_id = header(&req, "mcp-session-id").map(str::to_owned);
    if let Some(id) = &session_id {
        let mut sessions = state.mcp.sessions.lock().unwrap();
        sessions.retain(|_, v| now_ms().saturating_sub(v.touched) < IDLE_MS);
        let Some(session) = sessions.get_mut(id).filter(|v| v.owner == owner) else {
            return response(
                StatusCode::NOT_FOUND,
                json!({"error":"mcp_session_not_found"}),
            );
        };
        session.touched = now_ms();
        if req.method() == Method::DELETE {
            sessions.remove(id);
            return response(StatusCode::NO_CONTENT, Value::Null);
        }
    }
    if req.method() != Method::POST {
        let mut r = response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"error":"method_not_allowed"}),
        );
        r.headers_mut()
            .insert("allow", "POST, DELETE".parse().unwrap());
        return r;
    }
    if !header(&req, "content-type").is_some_and(|v| {
        v.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/json")
    }) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            json!({"error":"json_required"}),
        );
    }
    if header(&req, "accept").is_some_and(|v| {
        !v.split(',')
            .any(|v| matches!(v.trim().split(';').next(), Some("application/json" | "*/*")))
    }) {
        return response(
            StatusCode::NOT_ACCEPTABLE,
            json!({"error":"json_response_required"}),
        );
    }
    let Ok(permit) = state.mcp.calls.clone().try_acquire_owned() else {
        return response(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":"mcp_request_limit"}),
        );
    };
    let bytes = match tokio::time::timeout(
        Duration::from_secs(10),
        Limited::new(req.into_body(), BODY_LIMIT).collect(),
    )
    .await
    {
        Ok(Ok(b)) => b.to_bytes(),
        _ => {
            return response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"error":"request_body_limit"}),
            )
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return rpc_error(StatusCode::BAD_REQUEST, &Value::Null, -32700, "Parse error"),
    };
    let id = &value["id"];
    if !value.is_object()
        || value["jsonrpc"] != "2.0"
        || !value["method"].is_string()
        || (value.get("id").is_some() && !(id.is_string() || id.is_i64() || id.is_u64()))
    {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            &Value::Null,
            -32600,
            "Invalid Request",
        );
    }
    let method = value["method"].as_str().unwrap();
    if method == "initialize" {
        if session_id.is_some() || value.get("id").is_none() {
            return rpc_error(
                StatusCode::BAD_REQUEST,
                id,
                -32600,
                "Invalid initialization request",
            );
        }
        if !value["params"]["protocolVersion"].is_string()
            || !value["params"]["capabilities"].is_object()
            || !value["params"]["clientInfo"]["name"].is_string()
            || !value["params"]["clientInfo"]["version"].is_string()
        {
            return rpc_error(
                StatusCode::OK,
                id,
                -32602,
                "Invalid initialization parameters",
            );
        }
        let Ok(sid) = random_id() else {
            return rpc_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                id,
                -32603,
                "Internal error",
            );
        };
        {
            let mut sessions = state.mcp.sessions.lock().unwrap();
            sessions.retain(|_, v| now_ms().saturating_sub(v.touched) < IDLE_MS);
            if sessions.len() >= 128 || sessions.values().filter(|v| v.owner == owner).count() >= 8
            {
                return response(
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({"error":"mcp_session_limit"}),
                );
            }
            sessions.insert(
                sid.clone(),
                Session {
                    owner: owner.to_owned(),
                    touched: now_ms(),
                },
            );
        }
        let mut r = response(
            StatusCode::OK,
            json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":PROTOCOL,"capabilities":{"tools":{}},"serverInfo":{"name":"T3 Code","version":crate::rpc::SERVER_VERSION}}}),
        );
        r.headers_mut()
            .insert("mcp-session-id", sid.parse().unwrap());
        return r;
    }
    if session_id.is_none() {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            id,
            -32600,
            "Mcp-Session-Id is required",
        );
    }
    if value.get("id").is_none() {
        // JSON-RPC notifications never receive a response, including unknown notifications.
        return response(StatusCode::ACCEPTED, Value::Null);
    }
    let result = match method {
        "ping" => json!({}),
        "tools/list" => {
            if value["params"].get("cursor").is_some() {
                return rpc_error(StatusCode::OK, id, -32602, "Invalid tool cursor");
            }
            json!({"tools":crate::mcp_tools::catalog()})
        }
        "tools/call" => {
            let Some(name) = value["params"]["name"].as_str() else {
                return rpc_error(StatusCode::OK, id, -32602, "Tool name is required");
            };
            let args = value["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            if let Err(message) = crate::mcp_tools::validate(name, &args) {
                return rpc_error(StatusCode::OK, id, -32602, &message);
            }
            // Database and settings reads stay off Tokio I/O workers.
            let s = state.clone();
            let name = name.to_owned();
            let claims = claims.clone();
            let result = if name == "t3_thread_wait" {
                let _permit = permit;
                crate::mcp_mutations::wait(s, claims, args).await
            } else {
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    if crate::mcp_mutations::handles(&name) {
                        crate::mcp_mutations::call(&s, &claims, &name, &args)
                    } else {
                        crate::mcp_tools::call(&s, &name, &args)
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Err(crate::mcp_tools::failure(
                        "orchestration_error",
                        "Unable to complete the operation.",
                    ))
                })
            };
            let (content, is_error) = match result {
                Ok(v) => (v, false),
                Err(v) => (v, true),
            };
            json!({"content":[{"type":"text","text":content.to_string()}],"structuredContent":content,"isError":is_error})
        }
        _ => return rpc_error(StatusCode::OK, id, -32601, "Method not found"),
    };
    response(
        StatusCode::OK,
        json!({"jsonrpc":"2.0","id":id,"result":result}),
    )
}
