//! Provider-owned browser scope exposed through the shared native MCP transport.
use crate::{preview, store::State};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub(crate) struct Access(std::sync::Weak<Scope>);
impl Access {
    pub(crate) fn available(&self) -> bool {
        self.0.upgrade().is_some_and(|scope| {
            !scope.revoked.load(Ordering::SeqCst)
                && scope
                    .state
                    .upgrade()
                    .is_some_and(|s| enabled(&s, &scope.project))
        })
    }
    pub(crate) async fn call(
        &self,
        name: &str,
        p: &Value,
        authority: &crate::mcp_access::Authority,
    ) -> Result<Value, Value> {
        let scope = self
            .0
            .upgrade()
            .ok_or_else(|| json!({"message":"Browser session closed"}))?;
        scope.call(name, p, authority).await
    }
}
pub struct Broker {
    scope: Arc<Scope>,
    runtime: tokio::runtime::Handle,
}
struct Scope {
    state: std::sync::Weak<State>,
    thread: String,
    project: String,
    profile: String,
    tabs: Mutex<Vec<String>>,
    revoked: AtomicBool,
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.scope.revoked.store(true, Ordering::SeqCst);
        let scope = self.scope.clone();
        self.runtime.spawn(async move {
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
    pub(crate) fn access(&self) -> Access {
        Access(Arc::downgrade(&self.scope))
    }
    pub(crate) async fn finish_turn(&self) {
        self.scope.cleanup().await;
    }
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
        let scope = Arc::new(Scope {
            state: Arc::downgrade(&state),
            thread: thread.into(),
            project,
            profile: format!("agent:{}", crate::store::random_id().ok()?),
            revoked: AtomicBool::new(false),
            tabs: Mutex::new(Vec::new()),
        });
        Some(Self {
            scope,
            runtime: tokio::runtime::Handle::current(),
        })
    }
}

pub(crate) fn tools() -> Vec<Value> {
    [
        ("preview_status","List this session's browser tabs and their state"),
        ("preview_open","Open a private server browser tab; HTTP(S) only"),
        ("preview_navigate","Navigate a tab to a HTTP(S) URL"),
        ("preview_close","Close this session's tab"),
        ("preview_resize","Set viewport width and height"),
        ("preview_evaluate","Evaluate JavaScript; returns a bounded JSON value"),
        ("preview_snapshot","Read page text, title and bounded network diagnostics"),
        ("preview_click","Click a CSS locator"),
        ("preview_type","Fill a CSS locator with text"),
        ("preview_press","Press a key"),
    ].into_iter().map(|(name,description)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":{"tabId":{"type":"string"},"url":{"type":"string"},"expression":{"type":"string"},"locator":{"type":"string"},"text":{"type":"string"},"key":{"type":"string"},"width":{"type":"integer"},"height":{"type":"integer"}},"additionalProperties":false}})).collect()
}
impl Scope {
    async fn cleanup(&self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut tabs = self.tabs.lock().await;
        for tab in tabs.drain(..) {
            let _ = preview::call(
                &state,
                "preview.close",
                &json!({"threadId":self.thread,"tabId":tab}),
            )
            .await;
        }
    }
    async fn call(
        &self,
        name: &str,
        p: &Value,
        authority: &crate::mcp_access::Authority,
    ) -> Result<Value, Value> {
        let state = self
            .state
            .upgrade()
            .ok_or_else(|| json!({"message":"Browser owner closed"}))?;
        let mut tabs = self.tabs.lock().await;
        // Check after waiting for tab ownership, so terminal-turn cleanup cannot be
        // followed by a queued operation admitted while the parent was still active.
        authority.browser_admission(
            &state,
            !matches!(name, "preview_status" | "preview_snapshot"),
        )?;
        if self.revoked.load(Ordering::SeqCst) || !enabled(&state, &self.project) {
            return Err(json!({"message":"Agent browser access disabled"}));
        }
        if name == "preview_status" {
            let list =
                preview::call(&state, "preview.list", &json!({"threadId":self.thread})).await?;
            return Ok(
                json!({"available":true,"tabs":list["sessions"].as_array().into_iter().flatten().filter(|s|s["tabId"].as_str().is_some_and(|id|tabs.iter().any(|t|t==id))).cloned().collect::<Vec<_>>() }),
            );
        }
        if name == "preview_open" {
            if tabs.len() >= 4 {
                return Err(json!({"message":"Close a tab before opening another (limit 4)"}));
            }
            let result=preview::call(&state,"preview.open",&json!({"threadId":self.thread,"runtime":"server","profileId":format!("{}:{}",self.profile,crate::store::random_id().map_err(|_|json!({"message":"Cannot allocate context"}))?),"url":p["url"]})).await?;
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
            && state
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
                let result = preview::call(&state, "preview.close", &args).await?;
                tabs.retain(|t| t != &tab);
                Ok(result)
            }
            "preview_navigate" => preview::call(&state, "preview.navigate", &args).await,
            "preview_resize" => {
                args["viewport"] = json!({"_tag":"freeform","width":p["width"].as_u64().unwrap_or(1280).clamp(240,3840),"height":p["height"].as_u64().unwrap_or(800).clamp(240,2160)});
                preview::call(&state, "preview.resize", &args).await
            }
            "preview_press" => {
                let key = p["key"].as_str().unwrap_or("Enter");
                for event in ["keyDown", "keyUp"] {
                    state
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
                let result = state
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
                let mut value = result["result"]["value"].clone();
                if name == "preview_snapshot" {
                    if let Some(snapshot) = value.as_object_mut() {
                        snapshot.insert(
                            "networkEntries".into(),
                            json!(
                                state
                                    .streams
                                    .p2
                                    .preview
                                    .browser
                                    .network_entries(&self.thread, &tab)
                                    .await?
                            ),
                        );
                    }
                }
                if value.to_string().len() > 65536 {
                    return Err(json!({"message":"Evaluation result exceeds 64 KiB"}));
                }
                Ok(json!({"value":value}))
            }
            _ => Err(json!({"message":"Unsupported browser tool"})),
        }
    }
}
