//! Provider-owned browser scope exposed through the shared native MCP transport.
use crate::{preview, store::State};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Mutex;

pub(crate) struct BrowserOutput {
    pub value: Value,
    pub text_override: Option<String>,
}
impl From<Value> for BrowserOutput {
    fn from(value: Value) -> Self {
        Self {
            value,
            text_override: None,
        }
    }
}
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
    ) -> Result<BrowserOutput, Value> {
        let scope = self
            .0
            .upgrade()
            .ok_or_else(|| json!({"message":"Browser session closed"}))?;
        scope.call(name, p, authority).await
    }
}
pub struct Broker {
    scope: Arc<Scope>,
}
struct Scope {
    state: std::sync::Weak<State>,
    thread: String,
    project: String,
    profile: String,
    owner: std::sync::OnceLock<String>,
    ordinary: Mutex<()>,
    revoked: AtomicBool,
}
struct Used {
    state: Arc<State>,
    thread: String,
    agent: String,
    tab: Option<String>,
}
impl Drop for Used {
    fn drop(&mut self) {
        self.state.streams.p2.preview.browser.mark_used(
            &self.thread,
            &self.agent,
            self.tab.as_deref(),
        );
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        self.scope.revoked.store(true, Ordering::SeqCst);
    }
}
pub fn enabled(s: &State, project: &str) -> bool {
    crate::settings::read(s).is_ok_and(|v| {
        v["enableAgentBrowserAccess"] == true
            && v["projectSettingsOverrides"][project]["enableAgentBrowserAccess"] != false
    }) && (s.streams.p2.preview.desktop_available()
        || crate::server_browser::executable().is_some())
}
impl Broker {
    pub(crate) fn access(&self) -> Access {
        Access(Arc::downgrade(&self.scope))
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
            owner: std::sync::OnceLock::new(),
            revoked: AtomicBool::new(false),
            ordinary: Mutex::new(()),
        });
        state.streams.p2.preview.start_idle_worker(&state);
        Some(Self { scope })
    }
}

pub(crate) fn tools() -> Vec<Value> {
    let mut tools: Vec<Value> = [
        ("preview_status","List this thread's browser tabs and their state"),
        ("preview_open","Open or reuse a server browser tab; HTTP(S) only"),
        ("preview_navigate","Navigate a tab to a HTTP(S) URL"),
        ("preview_close","Close this session's tab"),
        ("preview_resize","Set viewport width and height"),
        ("preview_evaluate","Evaluate JavaScript; returns a bounded JSON value"),
        ("preview_snapshot","Read page text, title and bounded network diagnostics"),
        ("preview_click","Click a CSS locator"),
        ("preview_type","Fill a CSS locator with text"),
        ("preview_press","Press a key"),
    ].into_iter().map(|(name,description)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":{"tabId":{"type":"string"},"url":{"type":"string"},"expression":{"type":"string"},"locator":{"type":"string"},"text":{"type":"string"},"key":{"type":"string"},"width":{"type":"integer"},"height":{"type":"integer"}},"additionalProperties":false}})).collect();
    if let Some(open) = tools.iter_mut().find(|tool| tool["name"] == "preview_open") {
        for flag in ["reuseExistingTab", "open", "show"] {
            open["inputSchema"]["properties"][flag] = json!({"type":"boolean"});
        }
    }
    let native: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/native-preview-dialog-tool-4288.json"
    ))
    .expect("Pinned dialog tool metadata");
    tools.push(native["toolMetadata"].clone());
    tools
}
impl Scope {
    async fn status(
        &self,
        state: &State,
        owned: &[String],
        tab: Option<&str>,
    ) -> Result<BrowserOutput, Value> {
        let list = state.streams.p2.preview.snapshot_list(&self.thread);
        let sessions = list["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|session| {
                session["tabId"]
                    .as_str()
                    .is_some_and(|id| owned.iter().any(|tab| tab == id))
            })
            .collect::<Vec<_>>();
        let selected = tab.and_then(|tab| sessions.iter().find(|session| session["tabId"] == tab));
        let mut exact_dialog = None;
        let mut status = if let Some(selected) = selected {
            let dialog = state
                .streams
                .p2
                .preview
                .browser
                .dialog_state(&self.thread, selected["tabId"].as_str().unwrap())?;
            exact_dialog = Some(dialog.clone());
            let dialog = dialog.status();
            json!({"available":true,"visible":state.streams.p2.preview.browser.visible(&self.thread,tab.unwrap()),"tabId":selected["tabId"],"url":if selected["navStatus"]["url"]=="about:blank"{Value::Null}else{selected["navStatus"]["url"].clone()},"title":if dialog.is_null(){selected["navStatus"]["title"].as_str().filter(|title|!title.is_empty()).map_or(Value::Null,|title|json!(title))}else{Value::Null},"loading":selected["navStatus"]["_tag"]=="Loading","dialog":dialog})
        } else {
            json!({"available":false,"visible":false,"tabId":null,"url":null,"title":null,"loading":false})
        };
        if status["available"] == true && !status["url"].is_null() {
            let (dialog, title) = state
                .streams
                .p2
                .preview
                .browser
                .title_status_exact(&self.thread, status["tabId"].as_str().unwrap())
                .await?;
            status["dialog"] = dialog.status();
            exact_dialog = Some(dialog);
            status["title"] = title;
        }
        status["tabs"]=json!(sessions.iter().map(|session|json!({"tabId":session["tabId"],"url":if session["navStatus"]["url"]=="about:blank"{Value::Null}else{session["navStatus"]["url"].clone()},"visible":session["tabId"].as_str().is_some_and(|tab|state.streams.p2.preview.browser.visible(&self.thread,tab))})).collect::<Vec<_>>());
        use crate::utf16_json::JsonText;
        let text = JsonText::Object(
            status
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| {
                    let encoded = if key == "dialog" {
                        exact_dialog
                            .as_ref()
                            .map_or(JsonText::Value(Value::Null), |dialog| dialog.json_text())
                    } else if let Some(array) = value.as_array() {
                        JsonText::Array(array.iter().cloned().map(JsonText::Value).collect())
                    } else {
                        JsonText::Value(value.clone())
                    };
                    (key.clone(), encoded)
                })
                .collect(),
        )
        .stringify();
        Ok(BrowserOutput {
            value: status,
            text_override: Some(text),
        })
    }
    async fn call(
        self: &Arc<Self>,
        name: &str,
        p: &Value,
        authority: &crate::mcp_access::Authority,
    ) -> Result<BrowserOutput, Value> {
        let scope = self.clone();
        let name = name.to_owned();
        let input = p.clone();
        let authority = authority.clone();
        // Native invocation finalizers belong to the real operation, including
        // its activity timestamp; losing an HTTP observer does not finish it.
        tokio::spawn(async move {scope.call_inner(&name,&input,&authority).await}).await
            .map_err(|_|json!({"_tag":"BrowserControlInterrupted","reason":"closed","message":"Browser invocation worker ended."}))?
    }
    async fn call_inner(
        self: &Arc<Self>,
        name: &str,
        p: &Value,
        authority: &crate::mcp_access::Authority,
    ) -> Result<BrowserOutput, Value> {
        if !p.is_object() {
            return Err(json!({"message":"Expected browser input object"}));
        }
        let state = self
            .state
            .upgrade()
            .ok_or_else(|| json!({"message":"Browser owner closed"}))?;
        let _ordinary = if name == "preview_open" {
            Some(self.ordinary.lock().await)
        } else {
            None
        };
        // Check after waiting for ordinary work, so owner revocation cannot be
        // followed by a queued operation admitted while the parent was active.
        authority.browser_admission(
            &state,
            !matches!(name, "preview_status" | "preview_snapshot"),
        )?;
        if self.revoked.load(Ordering::SeqCst) || !enabled(&state, &self.project) {
            return Err(json!({"message":"Agent browser access disabled"}));
        }
        let owner = self
            .owner
            .get_or_init(|| authority.session_id.clone())
            .clone();
        if owner != authority.session_id {
            return Err(
                json!({"_tag":"BrowserControlInterrupted","reason":"agentMismatch","message":"This tab belongs to another agent."}),
            );
        }
        let mut used = Used {
            state: state.clone(),
            thread: self.thread.clone(),
            agent: owner.clone(),
            tab: p["tabId"].as_str().map(str::to_owned),
        };
        let tabs = state.streams.p2.preview.browser.thread_tabs(&self.thread);
        if name == "preview_status" {
            if !p.is_object() {
                return Err(json!({"message":"Expected status input object"}));
            }
            // Status and dialog use the same native PreviewTabId field codec.
            let target = if let Some(tab) = p.get("tabId") {
                json!({"accept":true,"tabId":tab})
            } else {
                json!({"accept":true})
            };
            let target = crate::server_browser::dialog::Input::decode(&target)
                .map_err(|message| json!({"message":message}))?;
            let tab = target.tab.or_else(|| {
                state
                    .streams
                    .p2
                    .preview
                    .browser
                    .latest_tab(&self.thread, Some(&owner))
            });
            used.tab = tab.clone();
            return self.status(&state, &tabs, tab.as_deref()).await;
        }
        let dialog_input = if name == "preview_dialog" {
            Some(
                crate::server_browser::dialog::Input::decode(p)
                    .map_err(|message| json!({"message":message}))?,
            )
        } else {
            None
        };
        if name == "preview_open" {
            // Native open rejects its URL before selecting, sweeping or reclaiming tabs.
            if let Some(url) = p.get("url") {
                preview::normalize_url(
                    url.as_str()
                        .ok_or_else(|| json!({"message":"Expected string url"}))?,
                )?;
            }
            for flag in ["reuseExistingTab", "open", "show"] {
                if p.get(flag).is_some_and(|value| !value.is_boolean()) {
                    return Err(json!({"message":format!("Expected boolean {flag}")}));
                }
            }
            let selector = crate::server_browser::dialog::Input::decode(&match p.get("tabId") {
                Some(tab) => json!({"accept":true,"tabId":tab}),
                None => json!({"accept":true}),
            })
            .map_err(|message| json!({"message":message}))?;
            let reuse = p["reuseExistingTab"].as_bool().unwrap_or(true);
            if !reuse && selector.tab.is_some() {
                return Err(
                    json!({"message":"tabId cannot be combined with reuseExistingTab=false"}),
                );
            }
            let owned = state
                .streams
                .p2
                .preview
                .browser
                .owned_tabs(&self.thread, &owner);
            if reuse && selector.tab.is_none() && owned.len() > 1 {
                return Err(
                    json!({"_tag":"BrowserControlInterrupted","reason":"tabRequired","message":"Multiple tabs are open. Pass tabId or reuseExistingTab=false."}),
                );
            }
            let target = if reuse {
                selector.tab.clone().or_else(|| owned.first().cloned())
            } else {
                None
            };
            let existing = if let Some(tab) = &target {
                Some(preview::agent_page(&state, &self.thread, tab).await?)
            } else {
                None
            };
            let tab = if let Some(page) = existing {
                if !page.dialog_status()?.is_null() {
                    return Err(
                        json!({"_tag":"BrowserControlInterrupted","reason":"dialogPending","message":"A browser dialog is pending. Read preview_status and use preview_dialog first."}),
                    );
                }
                let tab = page.key().1.to_owned();
                let control = page.control();
                let scope = self.clone();
                let state = state.clone();
                let authority = authority.clone();
                let input = p.clone();
                let selected = tab.clone();
                control.agent(&owner,None,move||async move {
                    authority.browser_admission(&state,true)?;
                    if scope.revoked.load(Ordering::SeqCst)||!enabled(&state,&scope.project){return Err(json!({"message":"Agent browser access disabled"}));}
                    if !state.streams.p2.preview.browser.current(&page){return Err(json!({"_tag":"BrowserControlInterrupted","reason":"closed","message":"This browser tab is closed."}));}
                    if !page.dialog_status()?.is_null(){return Err(json!({"_tag":"BrowserControlInterrupted","reason":"dialogPending","message":"A browser dialog is pending. Read preview_status and use preview_dialog first."}));}
                    if input.get("url").is_some() {scope.execute(&state,"preview_navigate",&input,&selected,&page).await} else {Ok(Value::Null)}
                }).wait().await?;
                tab
            } else {
                let result=preview::open_agent(&state,&json!({"threadId":self.thread,"runtime":"server","profileId":format!("{}:{}",self.profile,crate::store::random_id().map_err(|_|json!({"message":"Cannot allocate context"}))?),"url":p["url"]}),&owner).await?;
                result["tabId"]
                    .as_str()
                    .ok_or_else(|| json!({"message":"Browser did not return a tab"}))?
                    .to_owned()
            };
            used.tab = Some(tab.clone());
            if p["open"].as_bool().or_else(|| p["show"].as_bool()) != Some(false) {
                preview::request_reveal(
                    &state,
                    &self.thread,
                    &tab,
                    p["open"].as_bool().or_else(|| p["show"].as_bool()) == Some(true),
                )?;
            }
            let tabs = state.streams.p2.preview.browser.thread_tabs(&self.thread);
            return self.status(&state, &tabs, Some(&tab)).await;
        }
        let explicit = dialog_input
            .as_ref()
            .and_then(|input| input.tab.as_deref())
            .or_else(|| p["tabId"].as_str());
        if explicit.is_none()
            && state
                .streams
                .p2
                .preview
                .browser
                .owned_tabs(&self.thread, &owner)
                .len()
                > 1
        {
            return Err(
                json!({"_tag":"BrowserControlInterrupted","reason":"tabRequired","message":"Multiple tabs belong to this agent session. Pass a tabId from preview_open or preview_status tabs."}),
            );
        }
        let tab = explicit
            .map(str::to_owned)
            .or_else(|| {
                state
                    .streams
                    .p2
                    .preview
                    .browser
                    .latest_tab(&self.thread, Some(&owner))
            })
            .ok_or_else(|| json!({"message":"Open a browser tab first"}))?;
        used.tab = Some(tab.clone());
        let page = preview::agent_page(&state, &self.thread, &tab).await?;
        if name != "preview_snapshot"
            && page
                .control()
                .snapshot()
                .agent_id
                .as_deref()
                .is_some_and(|creator| creator != owner)
        {
            return Err(
                json!({"_tag":"BrowserControlInterrupted","reason":"agentMismatch","message":"This tab belongs to another agent session. You can read it, but open your own tab to act."}),
            );
        }
        if name != "preview_snapshot"
            && state
                .streams
                .p2
                .preview
                .browser
                .human_controls(&self.thread, &tab)
        {
            return Err(
                json!({"_tag":"BrowserControlInterrupted","reason":"humanControl","message":"A human controls this tab."}),
            );
        }
        if let Some(input) = dialog_input {
            state
                .streams
                .p2
                .preview
                .browser
                .answer_dialog(&self.thread, &tab, input.accept, input.prompt.as_deref())
                .await?;
            return self.status(&state, &tabs, Some(&tab)).await;
        }
        if name != "preview_close"
            && !state
                .streams
                .p2
                .preview
                .browser
                .dialog_status(&self.thread, &tab)?
                .is_null()
        {
            return Err(
                json!({"_tag":"BrowserControlInterrupted","reason":"dialogPending","message":"A browser dialog is pending. Read preview_status and use preview_dialog first."}),
            );
        }
        if name == "preview_close" {
            let result = preview::call(
                &state,
                "preview.close",
                &json!({"threadId":self.thread,"tabId":tab}),
            )
            .await?;
            return Ok(result.into());
        }
        let control = page.control();
        let scope = self.clone();
        let state = state.clone();
        let name = name.to_owned();
        let input = p.clone();
        let authority = authority.clone();
        let owner = owner.clone();
        let snapshot = control.snapshot();
        let observe = name == "preview_snapshot"
            && snapshot.agent_id.as_deref() != Some(&owner)
            && (snapshot.agent_id.is_some() || snapshot.controller.is_some());
        let work = move || async move {
            authority.browser_admission(&state, name != "preview_snapshot")?;
            if scope.revoked.load(Ordering::SeqCst) || !enabled(&state, &scope.project) {
                return Err(json!({"message":"Agent browser access disabled"}));
            }
            if !state.streams.p2.preview.browser.current(&page) {
                return Err(
                    json!({"_tag":"BrowserControlInterrupted","reason":"closed","message":"This browser tab is closed."}),
                );
            }
            if !page.dialog_status()?.is_null() {
                return Err(
                    json!({"_tag":"BrowserControlInterrupted","reason":"dialogPending","message":"A browser dialog is pending. Read preview_status and use preview_dialog first."}),
                );
            }
            scope.execute(&state, &name, &input, &tab, &page).await
        };
        let call = if observe {
            control.observe(work)
        } else {
            control.agent(&owner, None, work)
        };
        call.wait().await.map(BrowserOutput::from)
    }
    async fn execute(
        &self,
        state: &State,
        name: &str,
        p: &Value,
        tab: &str,
        page: &crate::server_browser::Page,
    ) -> Result<Value, Value> {
        let mut args = p.clone();
        if !args.is_object() {
            args = json!({});
        }
        args["threadId"] = json!(self.thread);
        args["tabId"] = json!(tab);
        match name {
            "preview_navigate" => preview::call_page(state, "preview.navigate", &args, page).await,
            "preview_resize" => {
                args["viewport"] = json!({"_tag":"freeform","width":p["width"].as_u64().unwrap_or(1280).clamp(240,3840),"height":p["height"].as_u64().unwrap_or(800).clamp(240,2160)});
                preview::call_page(state, "preview.resize", &args, page).await
            }
            "preview_press" => {
                let key = p["key"].as_str().unwrap_or("Enter");
                for event in ["keyDown", "keyUp"] {
                    page.command("Input.dispatchKeyEvent", json!({"type":event,"key":key}))
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
                let result = page
                    .command(
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
                        snapshot.insert("networkEntries".into(), json!(page.network_entries()?));
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
