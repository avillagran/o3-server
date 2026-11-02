//! Per-run, bearer-authenticated loopback MCP endpoint. Never persisted in settings.
use crate::{preview, store::State};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    Request, Response, StatusCode,
};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;

pub struct Broker {
    task: tokio::task::JoinHandle<()>,
    pub config: Value,
    scope: Arc<Scope>,
}
struct Scope {
    state: Arc<State>,
    thread: String,
    project: String,
    profile: String,
    tabs: Mutex<Vec<String>>,
    revoked: AtomicBool,
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.scope.revoked.store(true, Ordering::SeqCst);
        self.task.abort();
        let scope = self.scope.clone();
        tokio::spawn(async move {
            scope.cleanup().await;
        });
    }
}
pub fn enabled(s: &State, project: &str) -> bool {
    crate::settings::read(s).is_ok_and(|v| {
        v["enableAgentBrowserAccess"] == true
            && v["projectSettingsOverrides"][project]["enableAgentBrowserAccess"] != false
    }) && crate::server_browser::executable().is_some()
}
impl Broker {
    pub async fn start(state: Arc<State>, thread: &str) -> Option<Self> {
        let project = {
            let db = state.db.lock().unwrap();
            crate::events::get(&db, "thread", thread).ok()?["projectId"]
                .as_str()?
                .to_owned()
        };
        if !enabled(&state, &project) {
            return None;
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .ok()?;
        let endpoint = format!("http://{}/mcp", listener.local_addr().ok()?);
        let token = crate::store::random_id().ok()?;
        let scope = Arc::new(Scope {
            state,
            thread: thread.into(),
            project,
            profile: format!("agent:{}", crate::store::random_id().ok()?),
            revoked: AtomicBool::new(false),
            tabs: Mutex::new(Vec::new()),
        });
        let auth = format!("Bearer {token}");
        let task_scope = scope.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            let lifetime = tokio::time::sleep(Duration::from_secs(1800));
            tokio::pin!(lifetime);
            loop {
                tokio::select! {
                    _=&mut lifetime=>break,
                    Some(_)=connections.join_next()=>{},
                    next=listener.accept()=>{
                        let Ok((socket,peer))=next else {break};
                        if !peer.ip().is_loopback() || connections.len()>=8 {continue;}
                        let scope=task_scope.clone();let auth=auth.clone();
                        connections.spawn(async move {
                            let service=hyper::service::service_fn(move |req| handle(req,scope.clone(),auth.clone()));
                            let connection=hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(socket),service);
                            let _=tokio::time::timeout(Duration::from_secs(60),connection).await;
                        });
                    }
                }
            }
        });
        Some(Self {
            task,
            config: json!({"url":endpoint,"headers":{"Authorization":format!("Bearer {token}")}}),
            scope,
        })
    }
    pub async fn close(&self) {
        self.scope.revoked.store(true, Ordering::SeqCst);
        self.task.abort();
        // Abort the listener and all in-flight operations before draining ownership.
        // A single scope mutex serializes calls with cleanup.
        self.scope.cleanup().await;
    }
}
fn response(status: StatusCode, value: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(value.to_string())))
        .unwrap()
}
async fn handle(
    req: Request<Incoming>,
    scope: Arc<Scope>,
    auth: String,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if req.uri().path() != "/mcp"
        || req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some(auth.as_str())
        || req.headers().contains_key("origin")
        || !enabled(&scope.state, &scope.project)
    {
        return Ok(response(StatusCode::NOT_FOUND, json!({})));
    }
    if req.method() != hyper::Method::POST {
        return Ok(response(StatusCode::METHOD_NOT_ALLOWED, json!({})));
    }
    let bytes = match tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(req.into_body(), 65536).collect(),
    )
    .await
    {
        Ok(Ok(b)) => b.to_bytes(),
        _ => return Ok(response(StatusCode::BAD_REQUEST, json!({}))),
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(response(StatusCode::BAD_REQUEST, json!({})));
    };
    let id = v["id"].clone();
    if v["method"] == "notifications/initialized" {
        return Ok(response(StatusCode::ACCEPTED, Value::Null));
    }
    let result = match v["method"].as_str() {
        Some("initialize") => Ok(
            json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"t3-code","version":env!("CARGO_PKG_VERSION")}}),
        ),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({"tools":tools()})),
        Some("tools/call") => {
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                scope.call(
                    v["params"]["name"].as_str().unwrap_or(""),
                    &v["params"]["arguments"],
                ),
            )
            .await;
            let (value, error) = match result {
                Ok(Ok(v)) => (v, false),
                Ok(Err(e)) => (e, true),
                Err(_) => (json!({"message":"Browser operation timed out"}), true),
            };
            Ok(json!({"content":[{"type":"text","text":value.to_string()}],"isError":error}))
        }
        _ => Err(json!({"code":-32601,"message":"Unknown method"})),
    };
    Ok(response(
        StatusCode::OK,
        match result {
            Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
            Err(e) => json!({"jsonrpc":"2.0","id":id,"error":e}),
        },
    ))
}
fn tools() -> Vec<Value> {
    [
        ("preview_status","List this session's browser tabs and their state"),
        ("preview_open","Open a private server browser tab; HTTP(S) only"),
        ("preview_navigate","Navigate a tab to a HTTP(S) URL"),
        ("preview_close","Close this session's tab"),
        ("preview_resize","Set viewport width and height"),
        ("preview_evaluate","Evaluate JavaScript; returns a bounded JSON value"),
        ("preview_snapshot","Read page text and title"),
        ("preview_click","Click a CSS locator"),
        ("preview_type","Fill a CSS locator with text"),
        ("preview_press","Press a key"),
    ].into_iter().map(|(name,description)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":{"tabId":{"type":"string"},"url":{"type":"string"},"expression":{"type":"string"},"locator":{"type":"string"},"text":{"type":"string"},"key":{"type":"string"},"width":{"type":"integer"},"height":{"type":"integer"}},"additionalProperties":false}})).collect()
}
impl Scope {
    async fn cleanup(&self) {
        let mut tabs = self.tabs.lock().await;
        for tab in tabs.drain(..) {
            let _ = preview::call(
                &self.state,
                "preview.close",
                &json!({"threadId":self.thread,"tabId":tab}),
            )
            .await;
        }
    }
    async fn call(&self, name: &str, p: &Value) -> Result<Value, Value> {
        let mut tabs = self.tabs.lock().await;
        if self.revoked.load(Ordering::SeqCst) || !enabled(&self.state, &self.project) {
            return Err(json!({"message":"Agent browser access disabled"}));
        }
        if name == "preview_status" {
            let list = preview::call(
                &self.state,
                "preview.list",
                &json!({"threadId":self.thread}),
            )
            .await?;
            return Ok(
                json!({"available":true,"tabs":list["sessions"].as_array().into_iter().flatten().filter(|s|s["tabId"].as_str().is_some_and(|id|tabs.iter().any(|t|t==id))).cloned().collect::<Vec<_>>() }),
            );
        }
        if name == "preview_open" {
            if tabs.len() >= 4 {
                return Err(json!({"message":"Close a tab before opening another (limit 4)"}));
            }
            let result=preview::call(&self.state,"preview.open",&json!({"threadId":self.thread,"runtime":"server","profileId":format!("{}:{}",self.profile,crate::store::random_id().map_err(|_|json!({"message":"Cannot allocate context"}))?),"url":p["url"]})).await?;
            tabs.push(result["tabId"].as_str().unwrap().into());
            return Ok(result);
        }
        let tab = p["tabId"]
            .as_str()
            .or_else(|| tabs.last().map(String::as_str))
            .ok_or_else(|| json!({"message":"Open a browser tab first"}))?
            .to_owned();
        if !tabs.contains(&tab) {
            return Err(json!({"message":"Tab does not belong to this agent session"}));
        }
        if name != "preview_snapshot"
            && self
                .state
                .streams
                .p2
                .preview
                .browser
                .human_controls(&self.thread, &tab)
        {
            return Err(json!({"message":"Browser tab is controlled by a human"}));
        }
        let mut args = p.clone();
        if !args.is_object() {
            args = json!({});
        }
        args["threadId"] = json!(self.thread);
        args["tabId"] = json!(tab);
        match name {
            "preview_close" => {
                let result = preview::call(&self.state, "preview.close", &args).await?;
                tabs.retain(|t| t != &tab);
                Ok(result)
            }
            "preview_navigate" => preview::call(&self.state, "preview.navigate", &args).await,
            "preview_resize" => {
                args["viewport"] = json!({"_tag":"freeform","width":p["width"].as_u64().unwrap_or(1280).clamp(240,3840),"height":p["height"].as_u64().unwrap_or(800).clamp(240,2160)});
                preview::call(&self.state, "preview.resize", &args).await
            }
            "preview_press" => {
                let key = p["key"].as_str().unwrap_or("Enter");
                for event in ["keyDown", "keyUp"] {
                    self.state
                        .streams
                        .p2
                        .preview
                        .browser
                        .command(
                            &self.thread,
                            &tab,
                            "Input.dispatchKeyEvent",
                            json!({"type":event,"key":key}),
                        )
                        .await?;
                }
                Ok(json!({"success":true}))
            }
            "preview_evaluate" | "preview_snapshot" | "preview_click" | "preview_type" => {
                let locator = serde_json::to_string(p["locator"].as_str().unwrap_or("")).unwrap();
                let text = serde_json::to_string(p["text"].as_str().unwrap_or("")).unwrap();
                let expression=match name {
                    "preview_snapshot"=>"({url:location.href,title:document.title,text:document.body.innerText.slice(0,20000)})".into(),
                    "preview_click"=>format!("(()=>{{const e=document.querySelector({locator});if(!e)throw Error('No matching element');e.click();return true}})()"),
                    "preview_type"=>format!("(()=>{{const e=document.querySelector({locator});if(!e)throw Error('No matching element');e.focus();e.value={text};e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));return true}})()"),
                    _=>p["expression"].as_str().unwrap_or("").into(),
                };
                let result = self
                    .state
                    .streams
                    .p2
                    .preview
                    .browser
                    .command(
                        &self.thread,
                        &tab,
                        "Runtime.evaluate",
                        json!({"expression":expression,"returnByValue":true,"timeout":3000}),
                    )
                    .await?;
                if result.get("exceptionDetails").is_some() {
                    return Err(json!({"message":"Page expression failed"}));
                }
                let value = result["result"]["value"].clone();
                if value.to_string().len() > 65536 {
                    return Err(json!({"message":"Evaluation result exceeds 64 KiB"}));
                }
                Ok(json!({"value":value}))
            }
            _ => Err(json!({"message":"Unsupported browser tool"})),
        }
    }
}
