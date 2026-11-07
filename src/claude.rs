//! Direct Claude Code stream-json transport. No transcript cache or Node bridge.
use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use crate::provider_frames::{Frame, FrameBuffer, FrameError, MAX_FRAME};
use serde_json::{json, Value};
use std::{
    collections::HashMap, future::Future, path::PathBuf, pin::Pin, process::Stdio, sync::Mutex,
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, watch},
};
const MAX_SESSIONS: usize = 16;
/// A Claude Code session that launched this server exports these. A child spawned with them
/// joins that session (id, messaging socket/token, effort). Upstream 47f2c8d29.
const HOST_SESSION_VARIABLES: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_DESKTOP_APP_VERSION",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
    "CLAUDE_AGENT_SDK_VERSION",
];
/// Strips inherited host-session variables, then applies the entrypoint and the explicit
/// instance overrides, which win. A `CLAUDE_CODE_ENTRYPOINT` in Claude's settings `env` is
/// honoured because Claude reads the entrypoint from the process env only.
fn apply_environment(cmd: &mut Command, instance: &[(String, String)]) {
    for name in HOST_SESSION_VARIABLES {
        cmd.env_remove(name);
    }
    cmd.env("CLAUDE_CODE_ENTRYPOINT", "t3-code");
    if let Some(entrypoint) = settings_entrypoint(instance) {
        cmd.env("CLAUDE_CODE_ENTRYPOINT", entrypoint);
    }
    cmd.envs(instance.iter().cloned());
}
fn settings_entrypoint(instance: &[(String, String)]) -> Option<String> {
    let lookup = |key: &str| {
        instance
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| std::ffi::OsString::from(v))
            .or_else(|| std::env::var_os(key))
    };
    let dir = lookup("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| lookup("HOME").map(|h| PathBuf::from(h).join(".claude")))?;
    let file = std::fs::File::open(dir.join("settings.json")).ok()?;
    if file.metadata().ok()?.len() > 1024 * 1024 {
        return None;
    }
    let settings: Value = serde_json::from_reader(file).ok()?;
    settings["env"]["CLAUDE_CODE_ENTRYPOINT"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
pub struct ClaudeProvider {
    agent_scopes: Option<std::sync::Arc<crate::agent_scope::Registry>>,
    binary: PathBuf,
    idle_timeout: std::time::Duration,
    environment: Vec<(String, String)>,
    launch_args: Vec<String>,
    managed_policy: bool,
    metadata: Mutex<Value>,
    sessions: std::sync::Arc<Mutex<HashMap<String, Session>>>,
}
struct Session {
    _agent_scope: Option<crate::agent_scope::Lease>,
    native_mcp: Option<std::sync::Arc<crate::native_mcp::Context>>,
    _mcp_config: Option<PrivateConfig>,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    buffer: FrameBuffer,
    native: String,
    cwd: PathBuf,
    options: Value,
    browser: bool,
    idle_since: tokio::time::Instant,
}
// Claude accepts --mcp-config as a JSON file path. Never serialize credentials in argv.
struct PrivateConfig(PathBuf, crate::cleanup::Identity);
impl PrivateConfig {
    fn create(value: &Value) -> Result<Self, String> {
        use std::{
            io::Write,
            os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        };
        let dir = std::env::temp_dir().join(format!("o3-mcp-{}", crate::store::random_id()?));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| e.to_string())?;
        use crate::cleanup::Tree;
        let identity = crate::cleanup::FileTree
            .identity(&dir)
            .map_err(|error| error.to_string())?;
        let guard = Self(dir, identity);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(guard.0.join("config.json"))
            .map_err(|e| e.to_string())?;
        file.write_all(value.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(guard)
    }
}
impl Drop for PrivateConfig {
    fn drop(&mut self) {
        let _ = crate::cleanup::remove_owned_files(&self.0, self.1, &["config.json"]);
    }
}
impl ClaudeProvider {
    pub fn discover() -> Option<Self> {
        crate::executable::resolve(std::path::Path::new("claude")).map(Self::new)
    }
    pub fn new(binary: PathBuf) -> Self {
        Self {
            agent_scopes: None,
            binary,
            idle_timeout: std::time::Duration::from_secs(60),
            environment: Vec::new(),
            launch_args: Vec::new(),
            managed_policy: false,
            metadata: Mutex::new(json!({})),
            sessions: std::sync::Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub fn with_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
    pub fn with_agent_scopes(
        mut self,
        registry: std::sync::Arc<crate::agent_scope::Registry>,
    ) -> Self {
        self.agent_scopes = Some(registry);
        self
    }
    pub(crate) fn with_optional_agent_scopes(
        mut self,
        registry: Option<std::sync::Arc<crate::agent_scope::Registry>>,
    ) -> Self {
        self.agent_scopes = registry;
        self
    }
    pub fn with_managed_policy(mut self, managed: bool) -> Self {
        self.managed_policy = managed;
        self
    }
    pub fn with_launch_args(mut self, args: Vec<String>) -> Self {
        self.launch_args = args;
        self
    }
    pub fn with_environment(mut self, environment: Vec<(String, String)>) -> Self {
        self.environment = environment;
        self
    }
    pub async fn shutdown(&self) {
        let sessions = {
            let mut sessions = self.sessions.lock().unwrap();
            for s in sessions.values() {
                if let Some(c) = &s.native_mcp {
                    c.revoke();
                }
            }
            std::mem::take(&mut *sessions)
        };
        for (_, mut session) in sessions {
            session.stop().await;
        }
    }
    fn expire_idle(
        &self,
        thread_id: String,
        stamp: tokio::time::Instant,
        timeout: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let expired = {
                let mut map = sessions.lock().unwrap();
                if map.get(&thread_id).is_some_and(|s| s.idle_since == stamp) {
                    let expired = map.remove(&thread_id);
                    if let Some(context) = expired.as_ref().and_then(|s| s.native_mcp.as_ref()) {
                        context.revoke();
                    }
                    expired
                } else {
                    None
                }
            };
            if let Some(mut session) = expired {
                session.stop().await;
            }
        })
    }
    async fn spawn(&self, input: &RunInput) -> Result<Session, String> {
        let (model, effort, mut settings, _) =
            crate::claude_models::compile(&input.model_selection)?;
        let mut cmd = Command::new(&self.binary);
        crate::managed_policy::provider_host_environment(&mut cmd, self.managed_policy);
        apply_environment(&mut cmd, &self.environment);
        cmd.env(
            "T3CODE_THREAD_ID",
            input.model_selection["o3AppThreadId"]
                .as_str()
                .unwrap_or(&input.thread_id),
        );
        cmd.args(&self.launch_args);
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--allow-dangerously-skip-permissions",
        ])
        .arg("--model")
        .arg(model)
        .arg("--permission-mode")
        .arg(mode(input))
        .current_dir(&input.cwd);
        if input.model_selection["o3ColdCompact"] == true {
            // Build the continuation summary without permitting historical tool replay.
            cmd.args([
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
            ]);
        }
        if let Some(effort) = effort {
            cmd.arg("--effort").arg(effort);
        }
        let mut mcp_config = None;
        let injected = if input.model_selection["o3NativeMcp"].is_object() {
            &input.model_selection["o3NativeMcp"]
        } else {
            &input.model_selection["o3BrowserMcp"]
        };
        if injected.is_object() {
            let mut config = injected.clone();
            config["type"] = json!("http");
            let file = PrivateConfig::create(&json!({"mcpServers":{"t3-code":config}}))?;
            cmd.arg("--mcp-config").arg(file.0.join("config.json"));
            mcp_config = Some(file);
        }
        if input.thread_id == "discovery" || input.runtime_mode == "text-only" {
            settings["disableAllHooks"] = json!(true);
            cmd.args([
                "--no-session-persistence",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
            ])
            .env("ENABLE_CLAUDEAI_MCP_SERVERS", "false")
            .env("CLAUDE_CODE_AUTO_CONNECT_IDE", "0")
            .env("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1");
        }
        if input.thread_id == "discovery" {
            cmd.args(["--setting-sources", "user,project,local"]);
        }
        if input.runtime_mode == "text-only" {
            cmd.args([
                "--tools",
                "",
                "--disable-slash-commands",
                "--setting-sources",
                "",
            ]);
        }
        cmd.arg("--settings").arg(settings.to_string());
        if let Some(id) = input.model_selection["o3NewSessionId"].as_str() {
            cmd.arg("--session-id").arg(id);
        }
        if let Some(id) = input.model_selection["nativeForkSource"].as_str() {
            cmd.arg("--resume").arg(id).arg("--fork-session");
        } else if let Some(id) = &input.native_session_id {
            cmd.arg("--resume").arg(id);
        }

        let (mut cmd, agent_scope) = if let Some(registry) = &self.agent_scopes {
            let thread = input.model_selection["o3AppThreadId"]
                .as_str()
                .unwrap_or(&input.thread_id);
            registry
                .wrap(cmd, "claude", Some(thread), false, false)
                .await?
        } else {
            (cmd, None)
        };
        cmd.process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("Claude spawn: {e}"))?;
        let stdin = child.stdin.take().ok_or("Claude stdin missing")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("Claude stdout missing")?);
        let mut s = Session {
            _agent_scope: agent_scope,
            native_mcp: input.native_mcp.clone(),
            _mcp_config: mcp_config,
            child,
            stdin,
            stdout,
            buffer: FrameBuffer::default(),
            native: input
                .native_session_id
                .clone()
                .or_else(|| {
                    input.model_selection["o3NewSessionId"]
                        .as_str()
                        .map(str::to_owned)
                })
                .unwrap_or_default(),
            cwd: input.cwd.clone(),
            options: input.model_selection["options"].clone(),
            browser: injected.is_object(),
            idle_since: tokio::time::Instant::now(),
        };
        s.send(json!({"type":"control_request","request_id":"initialize","request":{"subtype":"initialize"}})).await?;
        // Pin the handshake: do not submit a prompt before initialize succeeds.
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let v = s.frame().await?;
                if v["type"] == "control_response" && v["response"]["request_id"] == "initialize" {
                    if v["response"]["subtype"] != "success" {
                        return Err(format!("Claude initialize rejected: {}",v["response"]["error"].as_str().unwrap_or("native initialization failed").chars().take(500).collect::<String>()));
                    }
                    let response = &v["response"]["response"];
                    let mut metadata = self.metadata.lock().unwrap();
                    if let Some(commands) = response["commands"].as_array() {
                        metadata["slashCommands"] = json!(commands.iter().map(|c| {
                            let mut command = json!({"name":c["name"],"description":c["description"]});
                            if let Some(hint)=c["argumentHint"].as_str() {command["input"]=json!({"hint":hint});}
                            command
                        }).collect::<Vec<_>>());
                        metadata["slashCommandsPending"] = json!(false);
                    }
                    if let Some(account) = response.get("account").filter(|v|v.is_object()) {
                        let authenticated = account["email"].is_string() || account["subscriptionType"].is_string() || account["tokenSource"].as_str().is_some_and(|s|s!="none") || account["apiKeySource"].as_str().is_some_and(|s|s!="none") || account["apiProvider"].as_str().is_some_and(|s|s!="firstParty");
                        metadata["auth"] = json!({"status":if authenticated {"authenticated"}else{"unauthenticated"}});
                        for key in ["email","workspaceId"] { if let Some(value)=account.get(key).filter(|v|v.is_string()) {metadata["auth"][key]=value.clone();} }
                        metadata["status"] = json!(if authenticated {"ready"}else{"warning"});
                    }
                    return Ok::<_, String>(());
                }
            }
        })
        .await
        .map_err(|_| "Claude initialize timeout")??;
        if let Some(context) = &input.native_mcp {
            let sessions = std::sync::Arc::downgrade(&self.sessions);
            let key = input.thread_id.clone();
            let id = context.id().to_owned();
            let dispose_sessions = sessions.clone();
            let dispose_key = key.clone();
            context.set_disposer(std::sync::Arc::new(move |_revoke| {
                let sessions = dispose_sessions.clone();
                let key = dispose_key.clone();
                Box::pin(async move {
                    let session = sessions
                        .upgrade()
                        .and_then(|sessions| sessions.lock().unwrap().remove(&key));
                    if let Some(mut session) = session {
                        session.stop().await;
                    }
                    Ok(())
                })
            }));
            context.set_process_check(std::sync::Arc::new(move || {
                let Some(sessions) = sessions.upgrade() else {
                    return false;
                };
                let mut sessions = sessions.lock().unwrap();
                sessions
                    .get_mut(&key)
                    .filter(|s| s.native_mcp.as_ref().is_some_and(|c| c.id() == id))
                    .is_none_or(|s| matches!(s.child.try_wait(), Ok(None)))
            }));
        }
        Ok(s)
    }
    async fn recover_input(
        &self,
        input: &mut RunInput,
        output: &mpsc::Sender<ProviderEvent>,
        reason: crate::provider::NativeRecoveryReason,
        current: &[Value],
    ) -> Result<(), String> {
        let id = crate::native_recovery::request(input, output, reason, current).await?;
        let previous = self.sessions.lock().unwrap().remove(&input.thread_id);
        if let Some(mut previous) = previous {
            previous.stop().await;
        }
        if let Some(context) = &input.native_mcp {
            context.revoke();
        }
        crate::native_recovery::released(output, id).await
    }
    async fn run(
        &self,
        mut input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        mut control: mpsc::Receiver<ProviderControl>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<(), String> {
        if *cancel.borrow() {
            return Ok(());
        }
        let mut content = crate::provider_input::content(&input, true)?;
        if let Some(reason) = crate::handoff_delivery::recovery_reason(&input, &content) {
            self.recover_input(&mut input, &output, reason, &content)
                .await?;
            content = crate::provider_input::content(&input, true)?;
        }
        let maintenance = crate::handoff_delivery::maintenance_input(&input);
        let manual = if maintenance {
            vec![]
        } else {
            crate::handoff_delivery::manual(&input, &mut content)?
        };
        let mut deliveries = if maintenance {
            vec![]
        } else {
            crate::handoff_delivery::prepare(&input, &content)?
        };
        if (!deliveries.is_empty() || !manual.is_empty()) && input.native_session_id.is_none() {
            input.model_selection["o3NewSessionId"] = json!(crate::store::random_id()?);
        }
        if !deliveries.is_empty() {
            let context = deliveries
                .iter()
                .map(|d| d.inline())
                .collect::<Vec<_>>()
                .join("\n\n");
            let current = content[0]["text"].as_str().unwrap_or("");
            content[0]["text"] = json!(format!("{context}\n\nUser message:\n{current}"));
        }
        deliveries.extend(manual);
        let existing = if input.runtime_mode == "text-only"
            || input.model_selection["o3ForceNewNativeThread"] == true
        {
            None
        } else {
            self.sessions.lock().unwrap().remove(&input.thread_id)
        };
        let loaded: Result<Session, String> = async {
            let session = if let Some(mut s) = existing {
                if input.native_session_id.is_none() && !s.native.is_empty() {
                    input.native_session_id = Some(s.native.clone());
                }
                let same_mcp = match (&s.native_mcp, &input.native_mcp) {
                    (Some(old), Some(new)) => old.id() == new.id(),
                    (None, None) => {
                        !s.browser
                            && !input.model_selection["o3BrowserMcp"].is_object()
                            && !input.model_selection["o3NativeMcp"].is_object()
                    }
                    _ => false,
                };
                if same_mcp
                    && s.cwd == input.cwd
                    && crate::selection_transition::option_key(&s.options) == crate::selection_transition::option_key(&input.model_selection["options"])
                    && s.child.try_wait().map_err(|e| e.to_string())?.is_none()
                {
                    s.control(
                        "model",
                        json!({"subtype":"set_model","model":crate::claude_models::compile(&input.model_selection)?.0}),
                    )
                    .await?;
                    s.control(
                        "mode",
                        json!({"subtype":"set_permission_mode","mode":mode(&input)}),
                    )
                    .await?;
                    s
                } else {
                    s.stop().await;
                    refresh_mcp(&mut input).await?;
                    self.spawn(&input).await?
                }
            } else {
                refresh_mcp(&mut input).await?;
                self.spawn(&input).await?
            };
            Ok(session)
        }
        .await;
        let mut session = match loaded {
            Ok(session) => session,
            Err(error)
                if input.model_selection["nativeForkSource"].is_string()
                    || (input.native_session_id.is_some()
                        && !crate::native_recovery::thread_in_use(&error)) =>
            {
                let base = crate::provider_input::content(&input, true)?;
                let reason = if input.model_selection["nativeForkSource"].is_string() {
                    crate::provider::NativeRecoveryReason::ForkFailed
                } else {
                    crate::provider::NativeRecoveryReason::ResumeFailed
                };
                self.recover_input(&mut input, &output, reason, &base)
                    .await?;
                content = crate::provider_input::content(&input, true)?;
                let manual = if maintenance {
                    vec![]
                } else {
                    crate::handoff_delivery::manual(&input, &mut content)?
                };
                deliveries = if maintenance {
                    vec![]
                } else {
                    crate::handoff_delivery::prepare(&input, &content)?
                };
                if !deliveries.is_empty() {
                    let history = deliveries
                        .iter()
                        .map(|d| d.inline())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let current = content[0]["text"].as_str().unwrap_or("");
                    content[0]["text"] = json!(format!("{history}\n\nUser message:\n{current}"));
                }
                deliveries.extend(manual);
                input.model_selection["o3NewSessionId"] = json!(crate::store::random_id()?);
                refresh_mcp(&mut input).await?;
                self.spawn(&input).await?
            }
            Err(error) => return Err(error),
        };
        if crate::claude_models::compile(&input.model_selection)?.3 {
            let text = content[0]["text"].as_str().unwrap().to_owned();
            content[0]["text"] = json!(format!("{text}\n\nultrathink"));
        }
        let result = async {
            if *cancel.borrow() || output.is_closed() || matches!(control.try_recv(), Ok(ProviderControl::Interrupt)) { return Ok(false); }
            for delivery in &deliveries {delivery.acknowledge(&output,&session.native,"pending").await?;}
            let mut history_inline = false;
            session.send(json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":session.native})).await?;
            output.send(ProviderEvent::Started).await.map_err(|_|"receiver closed")?;
            let mut pending: HashMap<String, Value> = HashMap::new();
            let mut boundary=TurnBoundary::default();
            let mut interrupt_requested=false;
            let mut cold_compact_pending = input.model_selection["o3ColdCompact"] == true;
            loop {
                tokio::select! {
                    biased;
                    _=output.closed()=>return Ok(false),
                    _=cancel.changed()=>return Ok(false),
                    c=control.recv()=>match c {
                        Some(ProviderControl::Interrupt)=>{
                            session.control("interrupt",json!({"subtype":"interrupt"})).await?;
                            interrupt_requested=true;
                            if boundary.terminal.is_some() { boundary.terminal=Some(ProviderEvent::Interrupted); }
                        },
                        Some(ProviderControl::SteerContent(content))=>session.send(json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":session.native})).await?,
                        Some(ProviderControl::Steer(text))=>session.prompt(&text).await?,
                        Some(ProviderControl::Respond{request_id,decision,feedback,answers})=>{
                            let native=request_id.strip_prefix(&format!("{}:",input.run_id)).ok_or("foreign Claude request")?;
                            // A provider withdrawal can race an already admitted client answer.
                            let Some(updated)=pending.remove(native) else {continue;};
                            let mut tool_input=updated["input"].clone();
                            let mut decision=decision;
                            if updated["tool_name"]=="AskUserQuestion" {
                                if let Some(answers)=answers.as_ref() {
                                    tool_input["answers"]=sdk_answers(answers)?;
                                    decision=json!("accept");
                                }
                            }
                            let mut response=permission_response(native,&decision,feedback.as_deref(),tool_input);
                            if decision=="acceptForSession" {
                                if let Some(suggestions)=updated["permission_suggestions"].as_array() {
                                    let permissions:Vec<Value>=suggestions.iter().cloned().map(|mut p|{p["destination"]=json!("session");p}).collect();
                                    response["response"]["response"]["updatedPermissions"]=json!(permissions);
                                }
                            }
                            session.send(response).await?;
                        },
                        None=>return Ok(false),
                    },
                    frame=session.frame()=>{
                        let v=frame?;
                        if v["type"]=="system" && v["subtype"]=="init" {
                            let mut metadata=self.metadata.lock().unwrap();
                            metadata["slashCommandsPending"]=json!(false);
                            for (native,field) in [("slash_commands","slashCommands"),("skills","skills")] {
                                if let Some(entries)=v[native].as_array() {
                                    metadata[field]=json!(entries.iter().map(|entry| if let Some(name)=entry.as_str() {json!({"name":name,"description":""})} else {entry.clone()}).collect::<Vec<_>>());
                                }
                            }
                            if let Some(version)=v["claude_code_version"].as_str() {metadata["version"]=json!(version);}
                        }
                        if v["type"]=="rate_limit_event" {
                            self.metadata.lock().unwrap()["usageLimits"]=usage_limits(&v["rate_limit_info"]);
                        }
                        if let Some(id)=v["session_id"].as_str() {
                            session.native=id.into();
                            if !history_inline { for delivery in &deliveries {delivery.acknowledge(&output,id,"inline").await?;} history_inline=true; }
                        }
                        if v["type"]=="control_response" && v["response"]["subtype"]=="error" { return Err(format!("Claude control rejected: {}",v["response"]["error"])); }
                        if v["type"]=="control_cancel_request" {
                            if let Some(native)=v["request_id"].as_str() {
                                if pending.remove(native).is_some() {
                                    output.send(ProviderEvent::Native(json!({"type":"permission_cancelled","request_id":format!("{}:{native}",input.run_id)}))).await.map_err(|_|"receiver closed")?;
                                }
                            }
                            continue;
                        }
                        if v["type"]=="control_request" {
                            let native=v["request_id"].as_str().ok_or("missing request id")?;
                            if v["request"]["subtype"]!="can_use_tool" { return Err("unsupported Claude control request".into()); }
                            if v["request"]["tool_name"]=="ExitPlanMode" {
                                let raw=&v["request"]["input"];
                                let raw=if raw["type"]=="record" {&raw["value"]} else {raw};
                                if let Some(markdown)=raw["plan"].as_str().filter(|s|!s.trim().is_empty() && s.len()<=120_000) {
                                    output.send(ProviderEvent::Native(json!({"type":"proposed_plan","id":format!("plan-{}-{native}",input.run_id),"markdown":markdown.trim()}))).await.map_err(|_|"receiver closed")?;
                                }
                                session.send(permission_response(native,&json!("decline"),Some("The client captured your proposed plan. Stop and wait for user feedback or implementation in a later turn."),json!({}))).await?;
                                continue;
                            }
                            if pending.len()>=64 { return Err("Claude pending request limit".into()); }
                            pending.insert(native.into(),v["request"].clone());
                            output.send(ProviderEvent::Native(json!({"type":"permission","request_id":format!("{}:{native}",input.run_id),"native_id":native,"request":v["request"]}))).await.map_err(|_|"receiver closed")?;
                        } else {
                            boundary.observe(&v)?;
                            for event in decode_with_continuation(&v, boundary.awaiting_continuation) {
                                if matches!(event,ProviderEvent::Completed|ProviderEvent::Failed(_)) {boundary.awaiting_continuation=false; boundary.terminal=Some(if interrupt_requested {ProviderEvent::Interrupted}else{event});} else {output.send(event).await.map_err(|_|"receiver closed")?;}
                            }
                        }
                        if boundary.ready() {
                            if cold_compact_pending && matches!(boundary.terminal, Some(ProviderEvent::Completed)) {
                                cold_compact_pending = false;
                                boundary = TurnBoundary::default();
                                session.prompt("/compact").await?;
                                continue;
                            }
                            output.send(boundary.terminal.take().unwrap()).await.map_err(|_|"receiver closed")?;
                            return Ok(true);
                        }
                    }
                }
            }
        }.await;
        if matches!(result, Ok(true))
            && input.runtime_mode != "text-only"
            && input.model_selection["o3ColdCompact"] != true
        {
            let stamp = tokio::time::Instant::now();
            session.idle_since = stamp;
            let thread_id = input.thread_id.clone();
            let evicted = {
                let mut sessions = self.sessions.lock().unwrap();
                let evicted = if sessions.len() >= MAX_SESSIONS {
                    let key = sessions.keys().next().cloned().unwrap();
                    let removed = sessions.remove(&key);
                    if let Some(c) = removed.as_ref().and_then(|s| s.native_mcp.as_ref()) {
                        c.revoke();
                    }
                    removed
                } else {
                    None
                };
                sessions.insert(input.thread_id, session);
                evicted
            };
            self.expire_idle(thread_id, stamp, self.idle_timeout);
            if let Some(mut s) = evicted {
                s.stop().await;
            }
        } else {
            session.stop().await;
        }
        result.map(|_| ())
    }
}
async fn refresh_mcp(input: &mut RunInput) -> Result<(), String> {
    if let Some(context) = &input.native_mcp {
        let context = context
            .refresh(&input.cwd, &input.model_selection["options"])
            .await?;
        input.model_selection["o3NativeMcp"] = context.config();
        input.native_mcp = Some(context);
    }
    Ok(())
}

#[derive(Default)]
struct TurnBoundary {
    tasks: std::collections::HashSet<String>,
    ended: std::collections::HashSet<String>,
    terminal: Option<ProviderEvent>,
    awaiting_continuation: bool,
}
impl TurnBoundary {
    fn observe(&mut self, v: &Value) -> Result<(), String> {
        if v["type"] == "system" {
            if v["subtype"] == "background_tasks_changed" {
                for task in v["tasks"].as_array().into_iter().flatten() {
                    if let Some(id) = task["task_id"].as_str() {
                        if self.tasks.len() >= 64 && !self.tasks.contains(id) {
                            return Err("Claude task limit reached".into());
                        }
                        if !self.ended.contains(id) {
                            self.tasks.insert(id.into());
                        }
                    }
                }
            }
            if let Some(id) = v["task_id"].as_str() {
                if v["subtype"] == "task_started" {
                    if self.tasks.len() >= 64 {
                        return Err("Claude task limit reached".into());
                    }
                    if !self.ended.contains(id) {
                        self.tasks.insert(id.into());
                    }
                }
                if v["subtype"] == "task_notification" {
                    // The root's earlier result only acknowledged launching background work.
                    // Claude next emits a task-notification turn; consume that turn before
                    // settling the app run. Duplicate notifications cannot reopen a result.
                    if self.tasks.remove(id)
                        && matches!(self.terminal, Some(ProviderEvent::Completed))
                    {
                        self.terminal = None;
                        self.awaiting_continuation = true;
                    }
                    if self.ended.len() >= 1024 {
                        return Err("Claude completed task limit reached".into());
                    }
                    self.ended.insert(id.into());
                }
            }
        }
        Ok(())
    }
    fn ready(&self) -> bool {
        self.terminal.is_some() && self.tasks.is_empty()
    }
}
fn mode(input: &RunInput) -> &str {
    if input.interaction_mode == "plan" {
        "plan"
    } else {
        match input.runtime_mode.as_str() {
            "full-access" => "bypassPermissions",
            "auto-accept-edits" => "acceptEdits",
            _ => "default",
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(context) = &self.native_mcp {
            context.revoke();
        }
        if let Some(pid) = self.child.id() {
            // Each CLI owns an isolated process group. Never leave tool grandchildren behind.
            // SAFETY: kill takes integer arguments and no pointers; pid is our live child's ID.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Session {
    async fn stop(&mut self) {
        if let Some(context) = self.native_mcp.take() {
            context.revoke();
        }
        if let Some(pid) = self.child.id() {
            // SAFETY: the isolated group is owned by this Session, with no borrowed pointers.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.kill().await;
        if let Some(mut scope) = self._agent_scope.take() {
            scope.stop().await;
        }
    }
    async fn send(&mut self, v: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Claude input frame too large".into());
        }
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|e| e.to_string())
    }
    async fn control(&mut self, id: &str, request: Value) -> Result<(), String> {
        self.send(json!({"type":"control_request","request_id":id,"request":request}))
            .await
    }
    async fn prompt(&mut self, text: &str) -> Result<(), String> {
        self.send(json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":text}]},"parent_tool_use_id":null,"session_id":self.native})).await
    }
    // Cancellation safe; the partial frame survives select! cancellation. A frame above the
    // buffering limit is drained and surfaced as `o3_oversized_frame` so the turn continues.
    async fn frame(&mut self) -> Result<Value, String> {
        match self.buffer.next(&mut self.stdout, MAX_FRAME).await {
            Ok(Some(Frame::Json(v, _))) => Ok(v),
            Ok(Some(Frame::Oversized(bytes))) => {
                Ok(json!({"type":"o3_oversized_frame","bytes":bytes}))
            }
            Ok(None) | Err(FrameError::Truncated) => Err("Claude stdout closed".into()),
            Err(FrameError::Io(e)) => Err(e),
            Err(FrameError::Json(e)) => Err(format!("Claude invalid JSON: {e}")),
        }
    }
}
impl Provider for ClaudeProvider {
    fn dispose<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.shutdown().await;
            Ok(())
        })
    }

    fn capabilities(&self, _instance: &str) -> Option<Value> {
        crate::selection_transition::native_capabilities("claudeAgent")
    }
    fn plan_selection_transition(
        &self,
        _instance: &str,
        _current: &Value,
        _target: &Value,
        _capabilities: &Value,
    ) -> Result<Option<crate::selection_transition::Selection>, String> {
        Ok(crate::selection_transition::turn_scoped())
    }

    fn detach<'a>(
        &'a self,
        _instance: &'a str,
        key: &'a str,
        _native: Option<&'a str>,
        _revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let session = self.sessions.lock().unwrap().remove(key);
            if let Some(mut session) = session {
                session.stop().await;
            }
            Ok(())
        })
    }

    fn discover<'a>(
        &'a self,
        cwd: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let input = RunInput {
                native_mcp: None,
                provider_instance_id: "claudeAgent".into(),
                native_session_id: None,
                thread_id: "discovery".into(),
                run_id: "discovery".into(),
                cwd,
                text: String::new(),
                model_selection: json!({"model":"haiku","options":[]}),
                runtime_mode: "approval-required".into(),
                interaction_mode: "default".into(),
                attachment_root: PathBuf::new(),
                attachments: json!([]),
                context: None,
            };
            let mut session = self.spawn(&input).await?;
            session.stop().await;
            Ok(())
        })
    }

    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let dir = self
                .environment
                .iter()
                .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
                .map(|(_, v)| PathBuf::from(v))
                .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
                .ok_or("Claude home unavailable")?;
            let credentials: Value = serde_json::from_slice(
                &std::fs::read(dir.join(".credentials.json"))
                    .map_err(|_| "Claude OAuth credentials unavailable")?,
            )
            .map_err(|_| "Claude OAuth credentials invalid")?;
            let token = credentials["claudeAiOauth"]["accessToken"]
                .as_str()
                .ok_or("Claude OAuth token unavailable")?;
            let usage = crate::config_sources::request(
                "https://api.anthropic.com/api/oauth/usage",
                token,
                &[
                    "anthropic-beta: oauth-2025-04-20",
                    "User-Agent: claude-code/2.1.273",
                ],
                None,
            )
            .await
            .map_err(|_| "Claude quota request failed")?;
            let mut windows = Vec::new();
            for (key, kind, label) in [
                ("five_hour", "session", "Session"),
                ("seven_day", "weekly", "Weekly"),
            ] {
                if let Some(used) = usage[key]["utilization"].as_f64() {
                    windows.push(json!({"id":key,"kind":kind,"label":label,"usedPercent":used.clamp(0.,100.),"resetsAt":usage[key]["resets_at"]}));
                }
            }
            self.metadata.lock().unwrap()["usageLimits"] =
                json!({"checkedAt":crate::store::iso(crate::store::now_ms()),"windows":windows});
            Ok(())
        })
    }

    fn config(&self) -> Vec<Value> {
        let mut descriptor = config(true);
        for (key, value) in self.metadata.lock().unwrap().as_object().unwrap() {
            descriptor[key] = value.clone();
        }
        vec![descriptor]
    }

    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        control: mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(self.run(input, output, control, cancel))
    }
}
pub(crate) fn permission_response(
    id: &str,
    decision: &Value,
    feedback: Option<&str>,
    input: Value,
) -> Value {
    let allow = matches!(
        decision.as_str(),
        Some("accept" | "acceptForSession" | "acceptAlways")
    );
    json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":if allow {json!({"behavior":"allow","updatedInput":input})}else{json!({"behavior":"deny","message":feedback.unwrap_or("Permission denied by user")})}}})
}
fn sdk_answers(answers: &Value) -> Result<Value, String> {
    let answers = answers.as_object().ok_or("answers must be an object")?;
    let mut result = serde_json::Map::new();
    for (key, value) in answers {
        let text = if let Some(text) = value.as_str() {
            text.to_owned()
        } else if let Some(values) = value.as_array() {
            values
                .iter()
                .map(|v| v.as_str().ok_or("answer must be text"))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        } else {
            return Err("answer must be text".into());
        };
        result.insert(key.clone(), json!(text));
    }
    Ok(Value::Object(result))
}

#[cfg(test)]
pub(crate) fn decode(v: &Value) -> Vec<ProviderEvent> {
    decode_with_continuation(v, false)
}
fn decode_with_continuation(v: &Value, awaiting_continuation: bool) -> Vec<ProviderEvent> {
    let mut out = Vec::new();
    match v["type"].as_str().unwrap_or("") {
        "stream_event" => {
            let e = &v["event"];
            if e["type"] == "content_block_delta" {
                let d = &e["delta"];
                if d["type"] == "text_delta" && v["parent_tool_use_id"].is_null() {
                    if let Some(t) = d["text"].as_str() {
                        out.push(ProviderEvent::TextDelta(t.into()));
                    }
                }
                if d["type"] == "text_delta" && !v["parent_tool_use_id"].is_null() {
                    out.push(ProviderEvent::Native(json!({"type":"subagent_text","text":d["text"],"parent_tool_use_id":v["parent_tool_use_id"]})));
                }
                if d["type"] == "thinking_delta" {
                    out.push(ProviderEvent::Native(json!({"type":"thinking","text":d["thinking"],"parent_tool_use_id":v["parent_tool_use_id"],"index":e["index"]})));
                }
            }
        }
        "rate_limit_event" => out.push(ProviderEvent::Native(
            json!({"type":"rate_limit","info":v["rate_limit_info"]}),
        )),
        "system" if v["subtype"] == "init" => out.push(ProviderEvent::Native(
            json!({"type":"session","session_id":v["session_id"]}),
        )),
        "system"
            if matches!(
                v["subtype"].as_str(),
                Some("task_started" | "task_notification" | "background_tasks_changed")
            ) =>
        {
            out.push(ProviderEvent::Native(json!({"type":"task","frame":v})))
        }
        "assistant" | "user" => {
            if v["parent_tool_use_id"].is_null() {
                if v["type"] == "assistant" && v["message"]["model"] != "<synthetic>" {
                    out.push(ProviderEvent::Native(json!({"type":"goal_checked"})));
                } else if v["message"]["model"] == "<synthetic>"
                    || (v["type"] == "user" && v["isSynthetic"] == true)
                {
                    let text = crate::native_goal::claude_text(v);
                    if crate::native_goal::trim(&text).starts_with("Goal ")
                        || crate::native_goal::trim(&text).starts_with("No goal set")
                        || text.starts_with("Stop hook feedback:\n[")
                    {
                        out.push(ProviderEvent::Native(
                            json!({"type":"goal_message","message":v}),
                        ));
                        if v["type"] == "assistant"
                            && (crate::native_goal::trim(&text).starts_with("Goal active: ")
                                || crate::native_goal::trim(&text).starts_with("No goal set"))
                        {
                            out.push(ProviderEvent::TextDelta(text));
                        }
                    }
                }
            }
            if let Some(content) = v["message"]["content"].as_array() {
                for block in content {
                    if matches!(block["type"].as_str(), Some("tool_use" | "tool_result")) {
                        let mut block = block.clone();
                        if block["type"] == "tool_result" {
                            crate::provider_frames::redact_inline_binary(&mut block["content"]);
                        }
                        out.push(ProviderEvent::Native(json!({"type":"tool","block":block,"parent_tool_use_id":v["parent_tool_use_id"]})));
                        if block["type"] == "tool_use" && block["name"] == "Bash" {
                            out.push(ProviderEvent::Command(
                                crate::command_output::CommandEvent::Started {
                                    item_id: format!(
                                        "native-tool:{}",
                                        block["id"].as_str().unwrap_or("")
                                    ),
                                    input: block["input"]["command"].as_str().unwrap_or("").into(),
                                },
                            ));
                        } else if block["type"] == "tool_result" {
                            out.push(ProviderEvent::Command(
                                crate::command_output::CommandEvent::Finished {
                                    item_id: format!(
                                        "native-tool:{}",
                                        block["tool_use_id"].as_str().unwrap_or("")
                                    ),
                                    exit_code: if block["is_error"] == true {
                                        Some(1)
                                    } else {
                                        Some(0)
                                    },
                                    output: Some(
                                        block["content"]
                                            .as_str()
                                            .map(str::to_owned)
                                            .unwrap_or_else(|| block["content"].to_string()),
                                    ),
                                },
                            ));
                        }
                    }
                }
            }
        }
        "result" => {
            // A resumed CLI can flush a zero-turn task-notification result before it
            // processes our user message. It neither settles that message nor supplies
            // its usage. An observed background completion, however, owns a continuation:
            // its zero-turn result can legitimately settle that continuation without text.
            // Upstream ClaudeAdapterV2 also distinguishes these two routes.
            if !awaiting_continuation
                && v["origin"]["kind"] == "task-notification"
                && v["num_turns"].as_u64() == Some(0)
                && v["subtype"] == "success"
                && v["is_error"] != true
            {
                return out;
            }
            out.push(ProviderEvent::Native(json!({"type":"usage","usage":v["usage"],"modelUsage":v["modelUsage"],"cost":v["total_cost_usd"],"session_id":v["session_id"],"origin":v["origin"],"num_turns":v["num_turns"],"subtype":v["subtype"],"terminal_reason":v["terminal_reason"]})));
            out.push(if v["is_error"] == true || v["subtype"] != "success" {
                ProviderEvent::Failed(v["result"].as_str().unwrap_or("Claude turn failed").into())
            } else {
                ProviderEvent::Completed
            });
        }
        _ => {}
    }
    out
}
/// Evidence the CLI can sign in, without launching it. Clients hide the models of a provider
/// that is not `ready`, so a lazy provider cannot wait for its first turn to report it.
fn credentials_present() -> bool {
    const ENV: [&str; 4] = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
    ];
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude")));
    ENV.iter().any(|v| std::env::var_os(v).is_some())
        || dir.is_some_and(|d| d.join(".credentials.json").is_file())
}
pub fn config(installed: bool) -> Value {
    static CHECKED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let checked = CHECKED.get_or_init(|| crate::store::iso(crate::store::now_ms()));
    json!({"instanceId":"claudeAgent","driver":"claudeAgent","displayName":"Claude Code","enabled":installed,"installed":installed,"version":null,"status":if !installed {"disabled"}else if credentials_present() {"ready"}else{"warning"},"auth":{"status":"unknown"},"checkedAt":checked,"message":"Direct CLI adapter; authentication checked at execution. Native-session continuation after restart; no inference reattachment.","slashCommands":[{"name":"compact","description":"Compact conversation history"}],"skills":[],"slashCommandsPending":true,"models":crate::claude_models::models(),"supportedRuntimeModes":["approval-required","auto-accept-edits","full-access"],"supportsConversationRollback":false,"nativeSessions":{"canList":false,"canLoad":false,"canResume":true}})
}

fn usage_limits(info: &Value) -> Value {
    let mut windows = Vec::new();
    for (native, id, label) in [
        ("five_hour", "session", "Session"),
        ("seven_day", "weekly", "Weekly"),
    ] {
        let w = &info["unifiedWindows"][native];
        let Some(used) = w["utilization"].as_f64().filter(|v| v.is_finite()) else {
            continue;
        };
        let mut window = json!({"id":native,"kind":id,"label":label,"usedPercent":(used*100.0).clamp(0.0,100.0)});
        if let Some(reset) = w["resetsAt"]
            .as_i64()
            .and_then(|s| s.checked_mul(1000))
            .filter(|v| time::OffsetDateTime::from_unix_timestamp(*v / 1000).is_ok())
        {
            window["resetsAt"] = json!(crate::store::iso(reset));
        }
        windows.push(window);
    }
    json!({"windows":windows,"checkedAt":crate::store::iso(crate::store::now_ms())})
}

#[cfg(test)]
mod tests {
    fn env_of(cmd: &Command) -> std::collections::HashMap<String, Option<String>> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }
    fn scratch() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("o3-env-{}", crate::store::random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn host_session_variables_are_removed_but_instance_overrides_win() {
        let home = scratch();
        let instance = vec![
            ("CLAUDE_CONFIG_DIR".to_owned(), home.display().to_string()),
            ("CLAUDE_EFFORT".to_owned(), "high".to_owned()),
        ];
        let mut cmd = Command::new("true");
        super::apply_environment(&mut cmd, &instance);
        let env = env_of(&cmd);
        for name in super::HOST_SESSION_VARIABLES {
            if name != "CLAUDE_EFFORT" {
                assert_eq!(env.get(name), Some(&None), "{name} must be removed");
            }
        }
        assert_eq!(env["CLAUDE_EFFORT"].as_deref(), Some("high"));
        assert_eq!(env["CLAUDE_CODE_ENTRYPOINT"].as_deref(), Some("t3-code"));
        std::fs::write(
            home.join("settings.json"),
            r#"{"env":{"CLAUDE_CODE_ENTRYPOINT":"custom"}}"#,
        )
        .unwrap();
        let mut cmd = Command::new("true");
        super::apply_environment(&mut cmd, &instance);
        assert_eq!(
            env_of(&cmd)["CLAUDE_CODE_ENTRYPOINT"].as_deref(),
            Some("custom")
        );
        // Fixture trees are retained for private container teardown.
    }
    #[tokio::test]
    async fn spawned_child_does_not_join_the_launching_session() {
        let dir = scratch();
        let out = dir.join("env");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!("env > {}", out.display()));
        cmd.env("CLAUDECODE", "1")
            .env("CLAUDE_CODE_SESSION_ID", "parent");
        super::apply_environment(&mut cmd, &[]);
        assert!(cmd.status().await.unwrap().success());
        let text = std::fs::read_to_string(out).unwrap();
        assert!(!text.contains("CLAUDECODE=") && !text.contains("CLAUDE_CODE_SESSION_ID="));
        assert!(text.contains("CLAUDE_CODE_ENTRYPOINT=t3-code"));
        // Fixture trees are retained for private container teardown.
    }
    #[test]
    fn text_only_mcp_config_literal_has_the_required_schema() {
        // The CLI rejects `{}`: its schema requires the mcpServers record.
        let source = include_str!("claude.rs");
        let config: Value = serde_json::from_str(r#"{"mcpServers":{}}"#).unwrap();
        assert!(config["mcpServers"].is_object());
        assert!(source.contains(concat!("r#\"", "{\"mcpServers\":{}}", "\"#")));
    }

    use super::*;
    fn fixtures(name: &str) -> Vec<Value> {
        let text = match name {
            "permission" => include_str!("../tests/fixtures/claude/permission.jsonl"),
            _ => include_str!("../tests/fixtures/claude/observed.jsonl"),
        };
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    #[tokio::test]
    async fn idle_session_is_reaped_and_canonical_instance_is_advertised() {
        assert_eq!(config(true)["instanceId"], "claudeAgent");
        let provider = ClaudeProvider::new("unused".into());
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let stamp = tokio::time::Instant::now();
        let session = Session {
            _agent_scope: None,
            native_mcp: None,
            _mcp_config: None,
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            buffer: FrameBuffer::default(),
            native: "fixture".into(),
            cwd: PathBuf::new(),
            options: Value::Null,
            browser: false,
            idle_since: stamp,
        };
        provider
            .sessions
            .lock()
            .unwrap()
            .insert("idle".into(), session);
        provider
            .expire_idle("idle".into(), stamp, std::time::Duration::from_millis(10))
            .await
            .unwrap();
        assert!(provider.sessions.lock().unwrap().is_empty());
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
    #[tokio::test]
    async fn text_generation_cannot_remove_a_colliding_conversation_session() {
        let dir = std::env::temp_dir()
            .join(crate::store::random_id().unwrap())
            .join("text-generation");
        std::fs::create_dir_all(&dir).unwrap();
        let binary =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/text-only.py");
        let provider = ClaudeProvider::new(binary);
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let session = Session {
            _agent_scope: None,
            native_mcp: None,
            _mcp_config: None,
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            buffer: FrameBuffer::default(),
            native: "conversation".into(),
            cwd: PathBuf::new(),
            options: Value::Null,
            browser: false,
            idle_since: tokio::time::Instant::now(),
        };
        provider
            .sessions
            .lock()
            .unwrap()
            .insert("collision".into(), session);
        let input = RunInput {
            native_mcp: None,
            provider_instance_id: "claudeAgent".into(),
            native_session_id: None,
            thread_id: "collision".into(),
            run_id: "collision".into(),
            cwd: dir.clone(),
            text: "title".into(),
            model_selection: json!({"model":"test"}),
            runtime_mode: "text-only".into(),
            interaction_mode: "default".into(),
            attachment_root: dir.clone(),
            attachments: json!([]),
            context: None,
        };
        let (out, _events) = mpsc::channel(16);
        let (_control, controls) = mpsc::channel(1);
        let (_cancel, cancel) = watch::channel(false);
        provider.run(input, out, controls, cancel).await.unwrap();
        assert!(provider.sessions.lock().unwrap().contains_key("collision"));
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        provider.shutdown().await;
        // Fixture trees are retained for private container teardown.
    }
    #[test]
    fn replay_real_partial_text_tools_results_and_usage() {
        let frames = fixtures("observed");
        let events: Vec<_> = frames.iter().flat_map(decode).collect();
        let text: String = events
            .iter()
            .filter_map(|e| {
                if let ProviderEvent::TextDelta(t) = e {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(text.contains("OK"));
        assert!(text.contains("DONE"));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, ProviderEvent::Completed))
                .count(),
            2
        );
        assert!(events
            .iter()
            .any(|e| matches!(e,ProviderEvent::Native(v) if v["block"]["name"]=="Bash")));
        assert!(events
            .iter()
            .any(|e| matches!(e,ProviderEvent::Native(v) if v["block"]["type"]=="tool_result")));
        assert!(events.iter().any(|e|matches!(e,ProviderEvent::Native(v) if v["type"]=="usage" && v["usage"]["output_tokens"].as_u64().unwrap()>0)));
    }
    #[test]
    fn real_permission_roundtrip_and_denial() {
        let f = fixtures("permission")
            .into_iter()
            .find(|v| v["type"] == "control_request")
            .unwrap();
        assert_eq!(f["request"]["subtype"], "can_use_tool");
        let r = permission_response(
            f["request_id"].as_str().unwrap(),
            &json!("accept"),
            None,
            f["request"]["input"].clone(),
        );
        assert_eq!(
            r["response"]["response"]["updatedInput"],
            f["request"]["input"]
        );
        assert_eq!(r["response"]["request_id"], f["request_id"]);
        assert_eq!(
            permission_response("id", &json!("decline"), Some("No"), json!({}))["response"]
                ["response"],
            json!({"behavior":"deny","message":"No"})
        );
    }
    #[tokio::test]
    async fn oversized_frames_are_skipped_and_malformed_frames_fail() {
        let mut child = Command::new("sh")
            .args(["-c", "printf 'not-json\\n'"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut s = Session {
            _agent_scope: None,
            native_mcp: None,
            _mcp_config: None,
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            buffer: FrameBuffer::default(),
            native: String::new(),
            cwd: PathBuf::new(),
            options: Value::Null,
            browser: false,
            idle_since: tokio::time::Instant::now(),
        };
        assert!(s.frame().await.unwrap_err().contains("invalid JSON"));
        let _ = s.child.wait().await;
        let mut child = Command::new("sh")
            .args([
                "-c",
                "head -c 67108865 /dev/zero; printf '\\n{\"type\":\"result\"}\\n'",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut s = Session {
            _agent_scope: None,
            native_mcp: None,
            _mcp_config: None,
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            buffer: FrameBuffer::default(),
            native: String::new(),
            cwd: PathBuf::new(),
            options: Value::Null,
            browser: false,
            idle_since: tokio::time::Instant::now(),
        };
        let skipped = s.frame().await.unwrap();
        assert_eq!(skipped["type"], "o3_oversized_frame");
        assert_eq!(skipped["bytes"], MAX_FRAME + 2);
        assert!(s.buffer.len() <= MAX_FRAME);
        assert_eq!(s.frame().await.unwrap()["type"], "result");
        s.stop().await;
    }
    #[test]
    fn tool_result_images_are_not_forwarded_as_base64() {
        let data = "A".repeat(4 * 1024 * 1024);
        let frame = json!({"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"read","content":[
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":data}}
            ]}
        ]}});
        let events = decode(&frame);
        let tool = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Native(v) if v["type"] == "tool" => Some(v),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            tool["block"]["content"][0]["source"]["data"],
            "[omitted 4194304 bytes of base64 data]"
        );
        assert!(serde_json::to_vec(tool).unwrap().len() < 4096);
    }
    #[test]
    fn observed_steering_and_interrupt_boundaries() {
        let steer: Vec<Value> = include_str!("../tests/fixtures/claude/steer.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let results: Vec<_> = steer.iter().filter(|v| v["type"] == "result").collect();
        assert_eq!(results.len(), 1);
        assert!(results[0]["result"].as_str().unwrap().contains("STEER_OK"));
        let interrupted: Vec<Value> = include_str!("../tests/fixtures/claude/interrupt.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(interrupted.iter().any(|v| v["type"] == "control_response"
            && v["response"]["request_id"] == "interrupt-probe"
            && v["response"]["subtype"] == "success"));
        assert!(interrupted
            .iter()
            .flat_map(decode)
            .any(|e| matches!(e, ProviderEvent::Failed(_))));
    }
    #[test]
    fn empty_task_notification_result_does_not_finish_submitted_prompt() {
        let mut boundary = TurnBoundary::default();
        let unrelated = json!({"type":"result","subtype":"success","is_error":false,
            "origin":{"kind":"task-notification"},"num_turns":0,
            "result":"","usage":{"input_tokens":0,"output_tokens":0}});
        for event in decode(&unrelated) {
            if matches!(event, ProviderEvent::Completed | ProviderEvent::Failed(_)) {
                boundary.terminal = Some(event);
            }
        }
        assert!(
            !boundary.ready(),
            "a notification is not the submitted prompt's result"
        );
        let reply = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"actual reply"}}});
        assert!(!decode(&reply).is_empty());
        let own = json!({"type":"result","subtype":"success","num_turns":1,
            "origin":{"kind":"human"},"usage":{"input_tokens":10,"output_tokens":2}});
        for event in decode(&own) {
            if matches!(event, ProviderEvent::Completed | ProviderEvent::Failed(_)) {
                boundary.terminal = Some(event);
            }
        }
        assert!(boundary.ready());
        // Do not hide zero-turn human commands, errors or real notification turns.
        for result in [
            json!({"type":"result","subtype":"success","num_turns":0,"origin":{"kind":"human"}}),
            json!({"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"origin":{"kind":"task-notification"}}),
            json!({"type":"result","subtype":"success","num_turns":1,"origin":{"kind":"task-notification"}}),
        ] {
            assert!(decode(&result)
                .iter()
                .any(|e| matches!(e, ProviderEvent::Completed | ProviderEvent::Failed(_))));
        }
    }

    #[test]
    fn background_acknowledgement_does_not_finish_parent() {
        let frames: Vec<Value> = include_str!("../tests/fixtures/claude/subagent.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let start = frames
            .iter()
            .find(|v| v["subtype"] == "task_started")
            .unwrap();
        let done = frames
            .iter()
            .find(|v| v["subtype"] == "task_notification")
            .unwrap();
        let mut boundary = TurnBoundary::default();
        boundary.observe(start).unwrap();
        boundary.terminal = Some(ProviderEvent::Completed);
        // Reorder the observed terminal boundary to exercise a background completion race.
        assert!(!boundary.ready());
        boundary.observe(done).unwrap();
        assert!(!boundary.ready(), "the parent's response is still unread");
        assert!(boundary.awaiting_continuation);
        let reply = json!({"type":"result","subtype":"success","origin":{"kind":"task-notification"},"num_turns":1});
        for event in decode_with_continuation(&reply, boundary.awaiting_continuation) {
            if matches!(event, ProviderEvent::Completed | ProviderEvent::Failed(_)) {
                boundary.terminal = Some(event);
            }
        }
        assert!(boundary.ready());
        boundary.observe(done).unwrap();
        assert!(
            boundary.ready(),
            "a duplicate must not reopen the continuation"
        );
    }

    #[test]
    fn owned_zero_turn_notification_settles_only_its_continuation() {
        let result = json!({"type":"result","subtype":"success","is_error":false,
            "origin":{"kind":"task-notification"},"num_turns":0});
        assert!(decode(&result).is_empty());
        assert!(decode_with_continuation(&result, true)
            .iter()
            .any(|event| matches!(event, ProviderEvent::Completed)));
    }
    #[tokio::test]
    #[ignore = "uses installed Claude credentials and Haiku quota; only temporary workspace"]
    async fn live_persistent_turns_permission_and_native_resume() {
        let dir = std::env::temp_dir().join(format!(
            "o3-claude-live-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&dir).unwrap();
        let provider = ClaudeProvider::discover().expect("claude installed");
        async fn turn(
            p: &ClaudeProvider,
            dir: &std::path::Path,
            text: &str,
            resume: Option<String>,
        ) -> Vec<ProviderEvent> {
            let input = RunInput {
                native_mcp: None,
                provider_instance_id: "claude".into(),
                native_session_id: resume,
                thread_id: "live".into(),
                run_id: crate::store::random_id().unwrap(),
                cwd: dir.into(),
                text: text.into(),
                model_selection: json!({"instanceId":"claude","model":"haiku"}),
                runtime_mode: "approval-required".into(),
                interaction_mode: "default".into(),
                attachment_root: dir.join("attachments"),
                attachments: json!([]),
                context: None,
            };
            let (tx, mut rx) = mpsc::channel(16);
            let (control, crx) = mpsc::channel(8);
            let (_cancel, watch) = watch::channel(false);
            let read = async {
                let mut events = vec![];
                while let Some(e) = rx.recv().await {
                    if let ProviderEvent::Native(v) = &e {
                        if v["type"] == "permission" {
                            control
                                .send(ProviderControl::Respond {
                                    request_id: v["request_id"].as_str().unwrap().into(),
                                    decision: json!("accept"),
                                    feedback: None,
                                    answers: None,
                                })
                                .await
                                .unwrap();
                        }
                    }
                    events.push(e);
                }
                events
            };
            let (r, events) = tokio::time::timeout(std::time::Duration::from_secs(90), async {
                tokio::join!(p.execute(input, tx, crx, watch), read)
            })
            .await
            .unwrap();
            r.unwrap();
            assert!(events.iter().any(|e| matches!(e, ProviderEvent::Completed)));
            events
        }
        let events = turn(
            &provider,
            &dir,
            "Use Write to create proof.txt containing TURQUOISE-741. Then reply OK.",
            None,
        )
        .await;
        assert!(events
            .iter()
            .any(|e| matches!(e,ProviderEvent::Native(v) if v["type"]=="permission")));
        let (pid, native) = {
            let pool = provider.sessions.lock().unwrap();
            let s = &pool["live"];
            (s.child.id(), s.native.clone())
        };
        let events = turn(
            &provider,
            &dir,
            "What exact code did I ask you to write? No tools.",
            None,
        )
        .await;
        assert_eq!(provider.sessions.lock().unwrap()["live"].child.id(), pid);
        assert!(events
            .iter()
            .any(|e| matches!(e,ProviderEvent::TextDelta(t) if t.contains("741"))));
        let mut s = provider.sessions.lock().unwrap().remove("live").unwrap();
        s.child.kill().await.unwrap();
        let events = turn(
            &provider,
            &dir,
            "Repeat the exact code from our earlier conversation. No tools.",
            Some(native),
        )
        .await;
        assert!(events
            .iter()
            .any(|e| matches!(e,ProviderEvent::TextDelta(t) if t.contains("741"))));
        let mut s = provider.sessions.lock().unwrap().remove("live").unwrap();
        s.child.kill().await.unwrap();
        // Fixture trees are retained for private container teardown.
    }
}
