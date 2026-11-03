//! Codex 0.166 app-server: one process, bounded multiplexing, no retained history.
use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin, sync::Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot, watch},
};

const MAX_FRAME: usize = 1024 * 1024;
/// Notifications queued for one Codex thread while its turn loop is busy (writing events,
/// awaiting an RPC). Bounded by bytes, not count: a burst of parallel tool calls emits many
/// small frames, and dropping any of them would corrupt the turn.
const ROUTE_BACKLOG_BYTES: usize = 8 * 1024 * 1024;
type Frames = mpsc::UnboundedReceiver<(Value, usize)>;
struct Route {
    tx: mpsc::UnboundedSender<(Value, usize)>,
    queued: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Route {
    /// Never blocks the shared reader; false when the consumer is gone or over budget.
    fn push(&self, frame: Value, size: usize) -> bool {
        use std::sync::atomic::Ordering;
        if self.queued.fetch_add(size, Ordering::AcqRel) + size > ROUTE_BACKLOG_BYTES {
            return false;
        }
        self.tx.send((frame, size)).is_ok()
    }
}
struct Inbox {
    rx: Frames,
    queued: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Inbox {
    fn pair() -> (Route, Inbox) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Route {
                tx,
                queued: queued.clone(),
            },
            Inbox { rx, queued },
        )
    }
    async fn recv(&mut self) -> Option<Value> {
        let (frame, size) = self.rx.recv().await?;
        self.queued
            .fetch_sub(size, std::sync::atomic::Ordering::AcqRel);
        Some(frame)
    }
}
type Reply = oneshot::Sender<Result<Value, String>>;
enum Op {
    Call(String, Value, Reply),
    Reply(Value),
    Subscribe(String, Route, oneshot::Sender<Result<(), String>>),
    Unsubscribe(String),
    Shutdown(oneshot::Sender<()>),
}
#[derive(Clone)]
struct Client(mpsc::Sender<Op>);
impl Client {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(Op::Call(method.into(), params, tx))
            .await
            .map_err(|_| "Codex process closed")?;
        tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| format!("Codex {method} timed out"))?
            .map_err(|_| "Codex process closed".to_owned())?
    }
}

pub struct CodexProvider {
    executable: PathBuf,
    home: Option<PathBuf>,
    environment: Vec<(String, String)>,
    launch_args: Vec<String>,
    idle_timeout: std::time::Duration,
    client: tokio::sync::Mutex<Option<Client>>,
    descriptor: std::sync::Arc<Mutex<Value>>,
}
fn codex_home(home: Option<&std::path::Path>) -> Option<PathBuf> {
    home.map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
}
/// Clients hide a provider's models until it is `ready`; this needs no app-server.
fn credentials_present(home: Option<&std::path::Path>) -> bool {
    std::env::var_os("OPENAI_API_KEY").is_some()
        || codex_home(home).is_some_and(|d| d.join("auth.json").is_file())
}
/// Codex's own catalog cache, so selecting a model never needs the 400 MB app-server.
fn model_capabilities(m: &Value) -> Value {
    let efforts = m["supportedReasoningEfforts"]
        .as_array()
        .or_else(|| m["supported_reasoning_levels"].as_array());
    let current = m
        .get("defaultReasoningEffort")
        .or_else(|| m.get("default_reasoning_level"))
        .cloned()
        .unwrap_or(Value::Null);
    let options = efforts
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let id = e["reasoningEffort"]
                .as_str()
                .or_else(|| e["effort"].as_str())?;
            Some(json!({"id":id,"label":id,"isDefault":current==id}))
        })
        .collect::<Vec<_>>();
    let mut descriptors = Vec::new();
    if !options.is_empty() {
        descriptors.push(json!({"id":"reasoningEffort","label":"Reasoning","type":"select","options":options,"currentValue":current}));
    }
    let tiers = m
        .get("serviceTiers")
        .or_else(|| m.get("service_tiers"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            m["additionalSpeedTiers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|id| json!({"id":id,"name":id}))
                .collect()
        });
    if !tiers.is_empty() {
        let default = m["defaultServiceTier"].as_str().unwrap_or("default");
        let mut options =
            vec![json!({"id":"default","label":"Standard","isDefault":default=="default"})];
        options.extend(
            tiers
                .iter()
                .map(|t| json!({"id":t["id"],"label":t["name"],"isDefault":t["id"]==default})),
        );
        descriptors.push(json!({"id":"serviceTier","label":"Service Tier","type":"select","options":options,"currentValue":default}));
    }
    json!({"optionDescriptors":descriptors})
}
fn cached_models(home: Option<&std::path::Path>) -> Value {
    let cache = codex_home(home)
        .and_then(|d| std::fs::read(d.join("models_cache.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let mut models: Vec<&Value> = cache
        .as_ref()
        .and_then(|c| c["models"].as_array())
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list" && m["slug"].is_string())
        .collect();
    models.sort_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX));
    Value::Array(
        models
            .iter()
            .enumerate()
            .map(|(i, m)| json!({"slug":m["slug"],"name":m["display_name"],"isCustom":false,"isDefault":i==0,"capabilities":model_capabilities(m)}))
            .collect(),
    )
}
impl Default for CodexProvider {
    fn default() -> Self {
        Self::new(
            crate::executable::resolve(std::path::Path::new("codex"))
                .unwrap_or_else(|| "codex".into()),
            None,
        )
    }
}
impl CodexProvider {
    pub fn new(executable: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            executable: crate::executable::resolve(&executable).unwrap_or(executable),
            home,
            environment: Vec::new(),
            launch_args: Vec::new(),
            idle_timeout: std::time::Duration::from_secs(60),
            client: tokio::sync::Mutex::new(None),
            descriptor: std::sync::Arc::new(Mutex::new(json!({
                "instanceId":"codex", "driver":"codex", "displayName":"Codex", "enabled":true,
                "installed":false,"version":null,"status":"warning","auth":{"status":"unknown"},
                "checkedAt":crate::store::iso(crate::store::now_ms()),"models":[],"slashCommands":[],"skills":[],
                "message":"Codex has not been probed", "supportsConversationRollback":false,
                "supportedRuntimeModes":["full-access","approval-required"],"reportsContextWindow":true
            }))),
        }
    }
    pub fn with_launch_args(mut self, args: Vec<String>) -> Self {
        self.launch_args = args;
        self
    }
    pub fn with_environment(mut self, environment: Vec<(String, String)>) -> Self {
        self.environment = environment;
        self
    }
    pub async fn probe(&self) -> Result<(), String> {
        if self
            .client
            .lock()
            .await
            .as_ref()
            .is_some_and(|c| !c.0.is_closed())
        {
            return Ok(());
        }
        self.probe_cached()
    }
    pub fn probe_cached(&self) -> Result<(), String> {
        let installed = crate::executable::resolve(&self.executable).is_some();
        let mut d = self.descriptor.lock().unwrap();
        d["installed"] = json!(installed);
        d["status"] = json!(
            match (installed, credentials_present(self.home.as_deref())) {
                (false, _) => "disabled",
                (true, true) => "ready",
                (true, false) => "warning",
            }
        );
        d["auth"] = json!({"status":"unknown"});
        d["message"] = json!(if installed {
            "Codex available; starts on first turn, authentication checked then"
        } else {
            "Codex binary not found"
        });
        d["checkedAt"] = json!(crate::store::iso(crate::store::now_ms()));
        if d["models"].as_array().is_none_or(|m| m.is_empty()) {
            d["models"] = cached_models(self.home.as_deref());
        }
        if installed {
            Ok(())
        } else {
            Err("Codex binary not found".into())
        }
    }
    pub async fn shutdown(&self) {
        if let Some(c) = self.client.lock().await.take() {
            let (tx, rx) = oneshot::channel();
            if c.0.send(Op::Shutdown(tx)).await.is_ok() {
                let _ = rx.await;
            }
        }
    }
    async fn connection(&self) -> Result<Client, String> {
        let mut guard = self.client.lock().await;
        if let Some(c) = guard.as_ref().filter(|c| !c.0.is_closed()) {
            return Ok(c.clone());
        }
        let result = self.launch().await;
        match &result {
            Ok(c) => *guard = Some(c.clone()),
            Err(e) => {
                let mut d = self.descriptor.lock().unwrap();
                d["installed"] = json!(crate::executable::resolve(&self.executable).is_some());
                d["status"] = json!("error");
                d["message"] = json!(e);
                d["models"] = json!([]);
            }
        }
        result
    }
    async fn launch(&self) -> Result<Client, String> {
        let mut cmd = Command::new(&self.executable);
        cmd.envs(self.environment.iter().cloned());
        cmd.arg("app-server")
            .arg("--stdio")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = &self.home {
            cmd.env("CODEX_HOME", home);
        }
        cmd.args(&self.launch_args);
        let child = cmd
            .spawn()
            .map_err(|e| format!("Cannot spawn Codex: {e}"))?;
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(actor(child, rx, self.descriptor.clone(), self.idle_timeout));
        let c = Client(tx);
        let init=c.call("initialize",json!({"clientInfo":{"name":"o3-server","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        c.0.send(Op::Reply(json!({"method":"initialized","params":{}})))
            .await
            .map_err(|_| "Codex closed")?;
        let models = c.call("model/list", json!({"limit":100})).await?;
        let auth = c
            .call("account/read", json!({"refreshToken":false}))
            .await?;
        let mut d = self.descriptor.lock().unwrap();
        d["installed"] = json!(true);
        d["version"] = json!(init["userAgent"]
            .as_str()
            .and_then(|a| a.split('/').nth(1))
            .and_then(|a| a.split_whitespace().next()));
        let authenticated = !auth["account"].is_null() || auth["requiresOpenaiAuth"] == false;
        d["auth"] = json!({"status":if authenticated{"authenticated"}else{"unauthenticated"}});
        for key in ["email", "workspaceId"] {
            if let Some(value) = auth["account"].get(key).filter(|v| v.is_string()) {
                d["auth"][key] = value.clone();
            }
        }
        d["status"] = json!(if authenticated { "ready" } else { "warning" });
        d["message"] = json!(if authenticated {
            "Codex app-server initialized"
        } else {
            "Codex requires authentication"
        });
        d["checkedAt"] = json!(crate::store::iso(crate::store::now_ms()));
        d["models"]=json!(models["data"].as_array().unwrap_or(&vec![]).iter().map(|m|json!({"slug":m["model"],"name":m["displayName"],"isCustom":false,"isDefault":m["isDefault"],"capabilities":model_capabilities(m)})).collect::<Vec<_>>());
        Ok(c)
    }
    async fn run(
        &self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        mut control: mpsc::Receiver<ProviderControl>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<(), String> {
        if *cancel.borrow() || output.is_closed() {
            return Ok(());
        }
        if !matches!(
            input.runtime_mode.as_str(),
            "approval-required" | "full-access"
        ) {
            return Err("Codex runtime mode is not implemented".into());
        }
        let content = crate::provider_input::content(&input, false)?;
        let c = self.connection().await?;
        let mut params = json!({"cwd":input.cwd,"model":input.model_selection["model"],"approvalPolicy":if input.runtime_mode=="full-access"{"never"}else{"on-request"},"sandbox":if input.runtime_mode=="full-access"{"danger-full-access"}else{"workspace-write"}});
        // Explicitly clear the per-run server when browser access was withheld.
        params["config"] = json!({"mcp_servers.t3-code":if input.model_selection["o3BrowserMcp"].is_object() {
            json!({"url":input.model_selection["o3BrowserMcp"]["url"],"http_headers":input.model_selection["o3BrowserMcp"]["headers"]})
        } else { json!({"enabled":false}) }});
        let response = if let Some(id) = input.model_selection["nativeForkSource"].as_str() {
            params["threadId"] = json!(id);
            c.call("thread/fork", params).await?
        } else if let Some(id) = input.native_session_id.as_ref() {
            params["threadId"] = json!(id);
            params["excludeTurns"] = json!(true);
            c.call("thread/resume", params).await?
        } else {
            c.call("thread/start", params).await?
        };
        let tid = response["thread"]["id"]
            .as_str()
            .ok_or("Codex thread id missing")?
            .to_owned();
        let (tx, mut frames) = Inbox::pair();
        let (ack, ready) = oneshot::channel();
        c.0.send(Op::Subscribe(tid.clone(), tx, ack))
            .await
            .map_err(|_| "Codex closed")?;
        ready.await.map_err(|_| "Codex closed")??;
        let mut active_turn = None;
        let mut terminal_received = false;
        let result=async {
            output.send(ProviderEvent::Codex(json!({"method":"o3/binding","params":{"threadId":tid,"cwd":input.cwd}}))).await.map_err(|_|"receiver closed")?;
            let effort=input.model_selection["options"].as_array().and_then(|a|a.iter().find(|v|v["id"]=="reasoningEffort")).map(|v|v["value"].clone());
            let mut turn_params=json!({"threadId":tid,"input":content,"model":input.model_selection["model"],"effort":effort});
            if let Some(tier) = input.model_selection["options"].as_array().and_then(|a| a.iter().find(|v| v["id"] == "serviceTier")).and_then(|v| v["value"].as_str()) {
                turn_params["serviceTier"] = if tier == "default" { Value::Null } else { json!(tier) };
            }
            if input.interaction_mode=="plan" {turn_params["collaborationMode"]=json!({"mode":"plan","settings":{"model":input.model_selection["model"],"reasoning_effort":effort,"developer_instructions":null}});}
            // A pre-start interrupt can arrive while thread initialization is
            // awaiting its native acknowledgement. Never start a late turn.
            if *cancel.borrow() || output.is_closed() || matches!(control.try_recv(), Ok(ProviderControl::Interrupt)) { return Ok(()); }
            let response=c.call("turn/start",turn_params).await?;
            let turn=response["turn"]["id"].as_str().ok_or("Codex turn id missing")?.to_owned();
            active_turn=Some(turn.clone());
            output.send(ProviderEvent::Codex(json!({"method":"turn/started","params":{"threadId":tid,"turn":response["turn"]}}))).await.map_err(|_|"receiver closed")?;
            let mut pending=HashMap::<String,(Value,String)>::new();
            let mut owned=std::collections::HashSet::<String>::new();
            let mut terminal=None;
            loop {
                if *cancel.borrow() { let _=c.call("turn/interrupt",json!({"threadId":tid,"turnId":turn})).await; return Ok(()); }
                tokio::select! {
                    _=output.closed()=> { let _=c.call("turn/interrupt",json!({"threadId":tid,"turnId":turn})).await; return Ok(()); },
                    _=cancel.changed()=>continue,
                    action=control.recv()=>match action {
                        Some(ProviderControl::Interrupt)=>{ if terminal.is_some() { terminal=Some(ProviderEvent::Interrupted); } c.call("turn/interrupt",json!({"threadId":tid,"turnId":turn})).await?; },
                        None=>return Err("provider control channel closed".into()),
                        Some(ProviderControl::SteerContent(content))=>{ c.call("turn/steer",json!({"threadId":tid,"expectedTurnId":turn,"input":content})).await?; },
                        Some(ProviderControl::Steer(text))=>{ c.call("turn/steer",json!({"threadId":tid,"expectedTurnId":turn,"input":[{"type":"text","text":text,"text_elements":[]}]})).await?; },
                        Some(ProviderControl::Respond{request_id,decision,answers,..})=>{
                            let (id,method)=pending.remove(&request_id).ok_or("Unknown Codex native request")?;
                            let result=native_response(&method,&decision,answers)?;
                            c.0.send(Op::Reply(json!({"id":id,"result":result}))).await.map_err(|_|"Codex closed")?;
                        }
                    },
                    frame=frames.recv()=>{
                        let mut frame=frame.ok_or("Codex notification channel closed or overflowed")?;
                        if frame["params"]["turnId"].as_str().or_else(||frame["params"]["turn"]["id"].as_str()).is_some_and(|id|id!=turn){continue;}
                        if let Some(id)=frame.get("id").cloned() {
                            if pending.len()>=16 { return Err("Too many pending Codex requests".into()); }
                            let method=frame["method"].as_str().unwrap_or("").to_owned();
                            if !matches!(method.as_str(),"item/commandExecution/requestApproval"|"item/fileChange/requestApproval"|"item/tool/requestUserInput"|"mcpServer/elicitation/request") {
                                c.0.send(Op::Reply(json!({"id":id,"error":{"code":-32601,"message":"Unsupported server request"}}))).await.map_err(|_|"Codex closed")?;continue;
                            }
                            let request_id=format!("codex-{}-{}",input.run_id,id);
                            pending.insert(request_id.clone(),(id,method));
                            frame["params"]["o3RequestId"]=json!(request_id);
                        }
                        if frame["method"]=="account/rateLimits/updated" {
                            self.descriptor.lock().unwrap()["usageLimits"]=usage_limits(&frame["params"]["rateLimits"]);
                        }
                        let failure_message=frame["params"]["turn"]["error"]["message"].as_str().unwrap_or("Codex turn failed").to_owned();
                        let done=frame["method"]=="turn/completed";
                        if let Some(id)=frame["params"]["item"]["id"].as_str() {
                            if frame["method"]=="item/started" && matches!(frame["params"]["item"]["type"].as_str(),Some("commandExecution"|"collabAgentToolCall")) {
                                if owned.len()>=64 {return Err("Codex owned item limit".into());}
                                owned.insert(id.to_owned());
                            }
                            if frame["method"]=="item/completed" {owned.remove(id);}
                        }
                        let failed=frame["params"]["turn"]["status"]=="failed";
                        let interrupted=frame["params"]["turn"]["status"]=="interrupted";
                        let text=frame["params"]["delta"].as_str().unwrap_or("").to_owned();
                        let commands=command_events(&frame,&input.run_id);
                        let event=if frame["method"]=="item/agentMessage/delta" {ProviderEvent::TextDelta(text)}else{ProviderEvent::Codex(frame)};
                        tokio::select! { r=output.send(event)=>r.map_err(|_|"receiver closed")?, _=cancel.changed()=>continue, _=output.closed()=>return Ok(()) }
                        for command in commands {output.send(ProviderEvent::Command(command)).await.map_err(|_|"receiver closed")?;}
                        if done {terminal=Some(if failed {ProviderEvent::Failed(failure_message)}else if interrupted{ProviderEvent::Interrupted}else{ProviderEvent::Completed});}
                        if terminal.is_some() && owned.is_empty() { terminal_received=true; output.send(terminal.take().unwrap()).await.map_err(|_|"receiver closed")?; return Ok(()); }
                    }
                }
            }
        }.await;
        if result.is_err() || !terminal_received {
            if let Some(turn) = active_turn {
                let _ = c
                    .call("turn/interrupt", json!({"threadId":tid,"turnId":turn}))
                    .await;
            }
        }
        let _ = c.0.send(Op::Unsubscribe(tid)).await;
        result
    }
}
impl Provider for CodexProvider {
    fn discover<'a>(
        &'a self,
        cwd: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let discovery = Self::new(self.executable.clone(), self.home.clone())
                .with_environment(self.environment.clone())
                .with_launch_args(self.launch_args.clone());
            let result = async {
                let c = discovery.connection().await?;
                if let Ok(skills) = c.call("skills/list", json!({"cwds":[cwd]})).await {
                    let values: Vec<Value> = skills["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|r| r["skills"].as_array().into_iter().flatten())
                        .cloned()
                        .collect();
                    discovery.descriptor.lock().unwrap()["skills"] = json!(values);
                }
                if let Ok(config) = c.call("config/read", json!({"includeLayers":false})).await {
                    if let Some(tier) = config["config"]["service_tier"].as_str() {
                        let mut descriptor = discovery.descriptor.lock().unwrap();
                        for model in descriptor["models"].as_array_mut().into_iter().flatten() {
                            for option in model["capabilities"]["optionDescriptors"]
                                .as_array_mut()
                                .into_iter()
                                .flatten()
                                .filter(|d| d["id"] == "serviceTier")
                            {
                                let target = if tier == "fast"
                                    && option["options"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .any(|o| o["id"] == "priority")
                                {
                                    "priority"
                                } else {
                                    tier
                                };
                                if option["options"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .any(|o| o["id"] == target)
                                {
                                    option["currentValue"] = json!(target);
                                    for value in
                                        option["options"].as_array_mut().into_iter().flatten()
                                    {
                                        value["isDefault"] = json!(value["id"] == target);
                                    }
                                }
                            }
                        }
                    }
                }
                Ok::<_, String>(())
            }
            .await;
            discovery.shutdown().await;
            if self
                .client
                .lock()
                .await
                .as_ref()
                .is_none_or(|c| c.0.is_closed())
            {
                *self.descriptor.lock().unwrap() = discovery.descriptor.lock().unwrap().clone();
            }
            result
        })
    }

    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(self.probe())
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            // Quota readers do not keep the inference actor alive between refreshes.
            let probe = Self::new(self.executable.clone(), self.home.clone())
                .with_environment(self.environment.clone())
                .with_launch_args(self.launch_args.clone());
            let result = async {
                let c = probe.connection().await?;
                let response = c.call("account/rateLimits/read", json!({})).await?;
                self.descriptor.lock().unwrap()["usageLimits"] =
                    usage_limits(&response["rateLimits"]);
                Ok::<_, String>(())
            }
            .await;
            probe.shutdown().await;
            result
        })
    }
    fn config(&self) -> Vec<Value> {
        if self.descriptor.lock().unwrap()["message"] == "Codex has not been probed" {
            let _ = self.probe_cached();
        }
        let d = self.descriptor.lock().unwrap().clone();
        vec![d]
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        control: mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if input.runtime_mode == "text-only" {
                // Neither shell_tool nor apply_patch_freeform disables the entire
                // native tool surface. Fail closed before submitting untrusted data.
                return Err(
                    "Codex text generation unavailable: no verified tool-free transport".into(),
                );
            }
            self.run(input, output, control, cancel).await
        })
    }
}
fn native_response(
    method: &str,
    decision: &Value,
    answers: Option<Value>,
) -> Result<Value, String> {
    let d = decision.as_str().unwrap_or("cancel");
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => Ok(
            json!({"decision":match d {"accept"=>"accept","acceptForSession"|"acceptAlways"=>"acceptForSession","decline"=>"decline",_=>"cancel"}}),
        ),
        "item/tool/requestUserInput" => Ok(
            json!({"answers":answers.unwrap_or(json!({})).as_object().map(|a|a.iter().map(|(k,v)|(k.clone(),if v.get("answers").is_some(){v.clone()}else{json!({"answers":if v.is_array(){v.clone()}else{json!([v])}})})).collect::<serde_json::Map<_,_>>()).unwrap_or_default()}),
        ),
        "mcpServer/elicitation/request" => Ok(
            json!({"action":if d=="accept"{"accept"}else if d=="decline"{"decline"}else{"cancel"},"content":answers}),
        ),
        _ => Err(format!("Unsupported Codex server request: {method}")),
    }
}
// read_until would allocate without bound before checking the size. Read from the
// fixed-capacity BufReader, checking each slice before extending the frame.
#[cfg(test)]
async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    frame: &mut Vec<u8>,
) -> Result<Option<Value>, String> {
    Ok(read_sized_frame(reader, frame).await?.map(|(v, _)| v))
}
/// Cancellation-safe: a partial line stays in `frame` for the next call.
async fn read_sized_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    frame: &mut Vec<u8>,
) -> Result<Option<(Value, usize)>, String> {
    loop {
        let buf = reader.fill_buf().await.map_err(|e| e.to_string())?;
        if buf.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("Truncated Codex frame".into())
            };
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(buf.len());
        if frame.len() + n > MAX_FRAME {
            return Err("Codex frame exceeds 1 MiB".into());
        }
        frame.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if frame.last() == Some(&b'\n') {
            let size = frame.len();
            let result = serde_json::from_slice(frame)
                .map(|v| Some((v, size)))
                .map_err(|e| e.to_string());
            frame.clear();
            return result;
        }
    }
}
async fn actor(
    mut child: tokio::process::Child,
    mut ops: mpsc::Receiver<Op>,
    descriptor: std::sync::Arc<Mutex<Value>>,
    idle_timeout: std::time::Duration,
) {
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::with_capacity(8192, child.stdout.take().unwrap());
    let mut calls = HashMap::<u64, Reply>::new();
    let mut routes = HashMap::<String, Route>::new();
    let mut next = 0u64;
    let mut buffer = Vec::new();
    let mut shutdown = None;
    let mut idle = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut last_operation = tokio::time::Instant::now();
    let result:Result<(),String>=async {
        loop { tokio::select! {
            _=idle.tick(), if routes.is_empty() && calls.is_empty()=>{
                if last_operation.elapsed()>=idle_timeout {return Ok(());}
            },
            op=ops.recv()=>{
                last_operation=tokio::time::Instant::now();
                let frame=match op {
                    None=>return Ok(()),
                    Some(Op::Subscribe(id,tx,ack))=>{let result=if routes.len()>=32||routes.contains_key(&id){Err("Codex thread already active or capacity reached".into())}else{routes.insert(id,tx);Ok(())};let _=ack.send(result);continue;},
                    Some(Op::Shutdown(ack))=>{shutdown=Some(ack);return Ok(());},
                    Some(Op::Unsubscribe(id))=>{routes.remove(&id);continue;},
                    Some(Op::Reply(frame))=>frame,
                    Some(Op::Call(method,params,reply))=>{
                        if calls.len()>=32 {let _=reply.send(Err("Codex RPC capacity reached".into()));continue;}
                        next+=1;calls.insert(next,reply);json!({"id":next,"method":method,"params":params})
                    }
                };
                let bytes=serde_json::to_vec(&frame).map_err(|e|e.to_string())?;
                if bytes.len()>MAX_FRAME{return Err("Codex outgoing frame exceeds limit".into());}
                tokio::time::timeout(std::time::Duration::from_secs(10),async {stdin.write_all(&bytes).await?;stdin.write_all(b"\n").await}).await.map_err(|_|"Codex write timed out")?.map_err(|e|e.to_string())?;
            },
            frame=read_sized_frame(&mut reader,&mut buffer)=>{
                let (frame,size)=frame?.ok_or("Codex stdout closed")?;
                if frame.get("method").is_none(){if let Some(id)=frame["id"].as_u64(){if let Some(reply)=calls.remove(&id){let _=reply.send(if frame.get("error").is_some(){Err(frame["error"].to_string())}else{Ok(frame["result"].clone())});}}continue;}
                if frame["method"]=="account/rateLimits/updated" {descriptor.lock().unwrap()["usageLimits"]=usage_limits(&frame["params"]["rateLimits"]);}
                let tid=frame["params"]["threadId"].as_str();
                if let Some(tid)=tid {
                    if let Some(route)=routes.get(tid){if !route.push(frame.clone(),size){routes.remove(tid);}}
                    else if let Some(id)=frame.get("id"){let bytes=serde_json::to_vec(&json!({"id":id,"error":{"code":-32601,"message":"No active thread"}})).unwrap();stdin.write_all(&bytes).await.map_err(|e|e.to_string())?;stdin.write_all(b"\n").await.map_err(|e|e.to_string())?;}
                }else if frame["method"]=="account/rateLimits/updated"{
                    routes.retain(|_,route|route.push(frame.clone(),size));
                }
            }
        }}
    }.await;
    let failed = result.is_err();
    let reason = result
        .err()
        .unwrap_or_else(|| "Codex idle or stopped; starts on next turn".into());
    {
        let mut d = descriptor.lock().unwrap();
        if failed {
            d["status"] = json!("error");
        }
        d["message"] = json!(reason);
    }
    for (_, reply) in calls {
        let _ = reply.send(Err(reason.clone()));
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    if let Some(ack) = shutdown {
        let _ = ack.send(());
    }
}

fn usage_limits(limits: &Value) -> Value {
    let mut windows = Vec::new();
    for (id, label, kind) in [
        ("primary", "Session", "session"),
        ("secondary", "Weekly", "weekly"),
    ] {
        let w = &limits[id];
        if !w.is_object() {
            continue;
        }
        let mut window = json!({"id":id,"kind":kind,"label":label,"usedPercent":w["usedPercent"].as_f64().unwrap_or(0.0).clamp(0.0,100.0)});
        if let Some(mins) = w["windowDurationMins"].as_u64() {
            window["windowDurationMins"] = json!(mins);
        }
        if let Some(reset) = w["resetsAt"]
            .as_i64()
            .and_then(|s| s.checked_mul(1000))
            .filter(|m| time::OffsetDateTime::from_unix_timestamp(*m / 1000).is_ok())
        {
            window["resetsAt"] = json!(crate::store::iso(reset));
        }
        windows.push(window);
    }
    json!({"checkedAt":crate::store::iso(crate::store::now_ms()),"windows":windows})
}

fn command_events(frame: &Value, rid: &str) -> Vec<crate::command_output::CommandEvent> {
    use crate::command_output::CommandEvent as E;
    let p = &frame["params"];
    let item = &p["item"];
    let native = p["itemId"]
        .as_str()
        .or_else(|| item["id"].as_str())
        .unwrap_or("");
    let id = format!("codex-item-{rid}-{native}");
    match frame["method"].as_str().unwrap_or("") {
        "item/started" if item["type"] == "commandExecution" => vec![E::Started {
            item_id: id,
            input: item["command"].as_str().unwrap_or("").into(),
        }],
        "item/completed" if item["type"] == "commandExecution" => vec![E::Finished {
            item_id: id,
            exit_code: item["exitCode"].as_i64().map(|n| n as i32),
            output: item["aggregatedOutput"].as_str().map(str::to_owned),
        }],
        "item/commandExecution/outputDelta" => vec![E::Output {
            item_id: id,
            chunk: p["delta"].as_str().unwrap_or("").into(),
        }],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(thread: &str, resume: Option<String>) -> RunInput {
        RunInput {
            provider_instance_id: "codex".into(),
            thread_id: thread.into(),
            run_id: format!("run-{thread}"),
            native_session_id: resume,
            cwd: std::env::temp_dir(),
            text: "fixture".into(),
            model_selection: json!({"model":"gpt-6.1-sol"}),
            runtime_mode: "approval-required".into(),
            interaction_mode: "default".into(),
            attachment_root: std::env::temp_dir().join("unused-attachments"),
            attachments: json!([]),
            context: None,
        }
    }
    #[tokio::test]
    async fn signed_in_lazy_provider_is_selectable_by_clients_without_spawning() {
        let dir = std::env::temp_dir().join(format!("o3-codex-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache =
            json!({"models":[{"slug":"a","display_name":"A","visibility":"list","priority":1}]});
        std::fs::write(dir.join("models_cache.json"), cache.to_string()).unwrap();
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/mock-server.py");
        let provider = CodexProvider::new(script, Some(dir.clone()));
        provider.probe().await.unwrap();
        // HOME and OPENAI_API_KEY can sign a developer in, so only assert the file-based branch.
        if std::env::var_os("OPENAI_API_KEY").is_none() {
            assert_eq!(provider.config()[0]["status"], "warning");
        }
        std::fs::write(dir.join("auth.json"), "{}").unwrap();
        provider.probe().await.unwrap();
        assert!(
            provider.client.lock().await.is_none(),
            "readiness spawned app-server"
        );
        let config = provider.config();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(config[0]["status"], "ready");
        // The real client rule: only `ready` providers offer their models.
        assert_eq!(config[0]["models"][0]["slug"], "a");
    }
    #[test]
    fn models_come_from_the_cache_without_spawning() {
        let dir = std::env::temp_dir().join(format!("o3-codex-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = json!({"models":[
            {"slug":"b","display_name":"B","visibility":"list","priority":2},
            {"slug":"hidden","display_name":"H","visibility":"hide","priority":0},
            {"slug":"a","display_name":"A","visibility":"list","priority":1}]});
        std::fs::write(dir.join("models_cache.json"), cache.to_string()).unwrap();
        let models = cached_models(Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(models.as_array().unwrap().len(), 2);
        assert_eq!(
            (&models[0]["slug"], &models[0]["isDefault"]),
            (&json!("a"), &json!(true))
        );
    }
    #[tokio::test]
    async fn discovery_is_lazy_and_idle_actor_exits_then_restarts() {
        let mut provider = CodexProvider::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/mock-server.py"),
            None,
        );
        // Long enough to outlive the startup handshake on a loaded machine.
        provider.idle_timeout = std::time::Duration::from_millis(500);
        provider.probe().await.unwrap();
        assert!(
            provider.client.lock().await.is_none(),
            "discovery spawned app-server"
        );
        assert_eq!(provider.config()[0]["version"], Value::Null);
        let c = provider.connection().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !c.0.is_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let next = provider.connection().await.unwrap();
        assert!(!next.0.is_closed());
        provider.shutdown().await;
    }
    #[tokio::test]
    async fn shared_process_routes_native_approvals_steering_and_resume() {
        let provider = std::sync::Arc::new(CodexProvider::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/mock-server.py"),
            None,
        ));
        provider.connection().await.unwrap();
        assert_eq!(provider.config()[0]["auth"]["status"], "unauthenticated");
        assert!(!provider.config()[0]["models"]
            .as_array()
            .unwrap()
            .is_empty());
        let (out1, mut events1) = mpsc::channel(16);
        let (out2, mut events2) = mpsc::channel(16);
        let (tx1, rx1) = mpsc::channel(8);
        let (tx2, rx2) = mpsc::channel(8);
        let (_cancel, cancel) = watch::channel(false);
        let a = provider.clone();
        let c1 = cancel.clone();
        let first = tokio::spawn(async move { a.run(input("app-1", None), out1, rx1, c1).await });
        let a = provider.clone();
        let second =
            tokio::spawn(async move { a.run(input("app-2", None), out2, rx2, cancel).await });
        let mut bindings = Vec::new();
        for (events, tx) in [(&mut events1, &tx1), (&mut events2, &tx2)] {
            let mut saw_text = false;
            loop {
                match tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
                {
                    ProviderEvent::Codex(f) if f["method"] == "o3/binding" => {
                        bindings.push(f["params"]["threadId"].as_str().unwrap().to_owned())
                    }
                    ProviderEvent::TextDelta(t) => {
                        assert!(t.starts_with("hello thread-"));
                        saw_text = true;
                    }
                    ProviderEvent::Codex(f) if f.get("id").is_some() => {
                        assert!(saw_text);
                        assert!(f["id"].is_number());
                        tx.send(ProviderControl::Steer("more".into()))
                            .await
                            .unwrap();
                        tx.send(ProviderControl::Respond {
                            request_id: f["params"]["o3RequestId"].as_str().unwrap().into(),
                            decision: json!("accept"),
                            feedback: None,
                            answers: None,
                        })
                        .await
                        .unwrap();
                    }
                    ProviderEvent::Completed => break,
                    _ => {}
                }
            }
        }
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        bindings.sort();
        assert_eq!(bindings, vec!["thread-1", "thread-2"]);
        // Drop the native app-server connection too: the next run must resume after restart.
        provider.shutdown().await;
        let (out, mut events) = mpsc::channel(16);
        let (tx, rx) = mpsc::channel(8);
        let (_cancel, cancel) = watch::channel(false);
        let a = provider.clone();
        let third = tokio::spawn(async move {
            a.run(input("app-1", Some("thread-1".into())), out, rx, cancel)
                .await
        });
        while let Some(e) = events.recv().await {
            if matches!(e, ProviderEvent::TextDelta(_)) {
                tx.send(ProviderControl::Interrupt).await.unwrap();
            }
        }
        third.await.unwrap().unwrap();
        provider.shutdown().await;
    }
    #[tokio::test]
    async fn notification_burst_with_a_slow_consumer_keeps_every_frame() {
        let provider = std::sync::Arc::new(CodexProvider::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/mock-server.py"),
            None,
        ));
        let (out, mut events) = mpsc::channel(1);
        let (_tx, rx) = mpsc::channel(8);
        let (_cancel, cancel) = watch::channel(false);
        let mut run = input("burst", None);
        run.text = "burst".into();
        let a = provider.clone();
        let task = tokio::spawn(async move { a.run(run, out, rx, cancel).await });
        // Hold the consumer while the whole burst is produced, as a busy writer would.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let mut text = String::new();
        let mut completed = false;
        while let Some(e) = events.recv().await {
            match e {
                ProviderEvent::TextDelta(t) => text.push_str(&t),
                ProviderEvent::Completed => completed = true,
                ProviderEvent::Failed(m) => panic!("turn failed: {m}"),
                _ => {}
            }
        }
        task.await.unwrap().unwrap();
        assert!(completed);
        let expected: String = (0..600).map(|n| format!("{n},")).collect();
        assert_eq!(text, expected);
        provider.shutdown().await;
    }
    #[tokio::test]
    async fn bounded_frames_survive_cancelled_read() {
        let (mut writer, reader) = tokio::io::duplex(128);
        let mut reader = BufReader::new(reader);
        let mut bytes = Vec::new();
        writer.write_all(b"{\"method\":").await.unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            read_frame(&mut reader, &mut bytes)
        )
        .await
        .is_err());
        writer.write_all(b"\"test\"}\n").await.unwrap();
        assert_eq!(
            read_frame(&mut reader, &mut bytes).await.unwrap().unwrap()["method"],
            "test"
        );
        let oversized = vec![b'x'; MAX_FRAME + 1];
        let mut reader = BufReader::new(oversized.as_slice());
        assert!(read_frame(&mut reader, &mut bytes)
            .await
            .unwrap_err()
            .contains("1 MiB"));
    }
    #[test]
    fn native_response_shapes() {
        let pin: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/codex/protocol-0.166.0.json"
        ))
        .unwrap();
        assert_eq!(pin["version"], "codex-cli 0.166.0");
        assert!(pin["schemas"]["v2/TurnSteerParams"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("expectedTurnId")));
        assert!(
            pin["schemas"]["CommandExecutionRequestApprovalResponse"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("decision"))
        );
        assert_eq!(
            native_response(
                "item/commandExecution/requestApproval",
                &json!("decline"),
                None
            )
            .unwrap(),
            json!({"decision":"decline"})
        );
        assert_eq!(
            native_response(
                "item/tool/requestUserInput",
                &Value::Null,
                Some(json!({"q":["yes"]}))
            )
            .unwrap(),
            json!({"answers":{"q":{"answers":["yes"]}}})
        );
        assert_eq!(
            native_response(
                "mcpServer/elicitation/request",
                &json!("accept"),
                Some(json!({"name":"test"}))
            )
            .unwrap(),
            json!({"action":"accept","content":{"name":"test"}})
        );
    }
    #[tokio::test]
    #[ignore = "requires installed codex; starts isolated app-server without an inference call"]
    async fn live_installed_codex_handshake_and_empty_resume() {
        let home = std::env::temp_dir().join(format!(
            "o3-codex-live-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&home).unwrap();
        let provider = CodexProvider::new("codex".into(), Some(home.clone()));
        provider.connection().await.unwrap();
        assert_eq!(provider.config()[0]["version"], "0.166.0");
        let c = provider.connection().await.unwrap();
        let thread = c
            .call(
                "thread/start",
                json!({"cwd":home,"approvalPolicy":"on-request","sandbox":"workspace-write"}),
            )
            .await
            .unwrap();
        let tid = &thread["thread"]["id"];
        let error = c
            .call("thread/resume", json!({"threadId":tid,"excludeTurns":true}))
            .await
            .unwrap_err();
        assert!(error.contains("no rollout found for thread id"));
        drop(c);
        provider.shutdown().await;
        std::fs::remove_dir_all(home).unwrap();
    }
}
