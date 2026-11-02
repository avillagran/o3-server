//! Provider setup: `provider.auth.*`, `provider.install.*`, `server.updateProvider` and the ACP
//! registry reads. Sign-in drives the vendor CLI (`claude auth login`, `codex login --device-auth`)
//! as a child process owned by a task that exists only while a flow runs. Built-in missing CLIs
//! may be installed in versioned data-directory prefixes; existing external CLIs are never replaced.
use crate::{
    p2::{invalid, str_field, Reply},
    store::{iso, now_ms, random_id, State},
    terminal::{bump, Step},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Notify},
};

const FLOW_TIMEOUT: Duration = Duration::from_secs(600);
const OUTPUT_CAP: usize = 16 * 1024;
const MAX_FLOWS: usize = 16;
const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "provider.auth.start"
            | "provider.auth.respond"
            | "provider.auth.cancel"
            | "provider.auth.logout"
            | "provider.install.start"
            | "provider.install.cancel"
            | "provider.install.remove"
            | "server.updateProvider"
            | "server.searchAcpRegistry"
            | "server.prepareAcpRegistryAgent"
    )
}

struct Flow {
    id: String,
    phase: &'static str,
    url: Option<String>,
    expires_at: Option<i64>,
    message: Option<String>,
    interaction: Option<Value>,
    output: String,
    input: Option<mpsc::Sender<Vec<u8>>>,
    cancel: Arc<Notify>,
}

#[derive(Default)]
pub struct Registry {
    flows: Mutex<HashMap<String, Flow>>,
    version: AtomicU64,
    updates: Mutex<HashMap<String, Value>>,
    versions: Mutex<HashMap<String, Value>>,
    installs: Mutex<HashMap<String, Value>>,
    install_cancels: Mutex<HashMap<String, Arc<Notify>>>,
}

impl Registry {
    fn touch(&self, s: &State) {
        self.version.fetch_add(1, Ordering::Relaxed);
        bump(&s.streams.changed);
    }
}

/// Only advertise setup operations that this server can actually execute.
pub fn decorate(s: &State, config: &mut Value) {
    for p in config["providers"].as_array_mut().into_iter().flatten() {
        let supported = matches!(p["driver"].as_str(), Some("claudeAgent" | "codex"));
        let installed = p["installed"] == true;
        let id = p["instanceId"].as_str().unwrap_or("");
        let managed = managed_binary(s, id).is_some_and(|path| {
            p["executablePath"]
                .as_str()
                .is_some_and(|v| std::path::Path::new(v) == path)
                || crate::settings::read(s).ok().is_some_and(|v| {
                    v["providerInstances"][id]["config"]["binaryPath"]
                        == path.to_string_lossy().as_ref()
                })
        });
        let installable = matches!(id, "codex" | "claudeAgent")
            && which("npm").is_some()
            && which("node").is_some();
        p["setup"] =
            json!({"canAuthenticate":supported&&installed,"canInstall":installable&&!installed});
        if !p["auth"].is_object() {
            p["auth"] = json!({"status":"unknown"});
        }
        p["auth"]["canLogout"] = json!(supported && installed);
        p["versionAdvisory"] = json!({"status":"unknown","currentVersion":p["version"],"latestVersion":null,
            "updateCommand":if managed&&installable {json!("Managed provider update")}else if p["driver"]=="claudeAgent" {json!("claude update")}else{Value::Null},
            "canUpdate":installed&&(p["driver"]=="claudeAgent"||managed&&installable),"canInstallVersion":installable&&(managed||!installed),
            "checkedAt":p["checkedAt"],"message":"Latest-version lookup is unavailable; installed version is reported by the native CLI."});
        if let Some(advisory) = s
            .streams
            .p2
            .setup
            .versions
            .lock()
            .unwrap()
            .get(p["instanceId"].as_str().unwrap_or(""))
        {
            if crate::settings::read(s).is_ok_and(|v| v["enableProviderUpdateChecks"] == true) {
                crate::settings::merge(&mut p["versionAdvisory"], advisory);
            }
        }
        p["updateState"]=s.streams.p2.setup.updates.lock().unwrap().get(p["instanceId"].as_str().unwrap_or("")).cloned()
            .unwrap_or(json!({"status":"idle","startedAt":null,"finishedAt":null,"message":null,"output":null}));
    }
}

/// Read vendor package metadata; never install or execute downloaded content.
pub(crate) async fn check_versions(s: &State) {
    if !crate::settings::read(s).is_ok_and(|v| v["enableProviderUpdateChecks"] == true) {
        return;
    }
    let providers = s
        .provider
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.config())
        .unwrap_or_default();
    for p in providers {
        if p["installed"] != true {
            continue;
        }
        let package = match p["driver"].as_str() {
            Some("claudeAgent") => "@anthropic-ai/claude-code",
            Some("codex") => "@openai/codex",
            _ => continue,
        };
        let Some(id) = p["instanceId"].as_str() else {
            continue;
        };
        let base = std::env::var("O3_PROVIDER_VERSION_REGISTRY")
            .unwrap_or_else(|_| "https://registry.npmjs.org".into());
        let url = format!("{}/{}/latest", base.trim_end_matches('/'), package);
        let output = tokio::process::Command::new("curl")
            .args([
                "-fsSL",
                "--max-time",
                "3",
                "--max-filesize",
                "65536",
                "--proto",
                "=http,https",
                "--",
                &url,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;
        let latest = output
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .and_then(|v| {
                v["version"]
                    .as_str()
                    .filter(|v| version_parts(v).is_some())
                    .map(str::to_owned)
            });
        let current = p["version"].as_str().and_then(version_parts);
        let status = match (current, latest.as_deref().and_then(version_parts)) {
            (Some(a), Some(b)) if a < b => "behind_latest",
            (Some(_), Some(_)) => "current",
            _ => "unknown",
        };
        s.streams.p2.setup.versions.lock().unwrap().insert(id.into(),json!({"status":status,"latestVersion":latest,"checkedAt":iso(now_ms()),"message":if latest.is_some(){Value::Null}else{json!("Package version lookup failed")}}));
    }
    s.notify();
}
fn version_parts(value: &str) -> Option<Vec<u64>> {
    let version = value
        .split_whitespace()
        .find(|s| s.starts_with(|c: char| c.is_ascii_digit()))?;
    let numbers = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (numbers.len() == 3).then_some(numbers)
}

fn setup_error(instance: &str, operation: &str, detail: &str) -> Value {
    json!({"_tag":"ProviderSetupError","instanceId":instance,"operation":operation,"detail":detail,"message":detail})
}

#[derive(Clone)]
struct Driver {
    instance: String,
    kind: &'static str, // "claudeAgent" | "codex"
    binary: PathBuf,
    environment: Vec<(String, String)>,
}

fn instance_id(p: &Value) -> Result<&str, Value> {
    p["instanceId"]
        .as_str()
        .filter(|i| !i.is_empty() && i.len() <= 64)
        .ok_or_else(|| invalid("instanceId is required"))
}

fn which(name: &str) -> Option<PathBuf> {
    crate::executable::resolve(std::path::Path::new(name))
}

/// The CLI behind `instance`, from the instance's configured `binaryPath` or `PATH`.
fn driver(s: &State, instance: &str, operation: &str) -> Result<Driver, Value> {
    let settings = crate::settings::read(s).map_err(|e| setup_error(instance, operation, &e))?;
    let configured = &settings["providerInstances"][instance];
    let kind = match configured["driver"].as_str() {
        Some("claudeAgent") => "claudeAgent",
        Some("codex") => "codex",
        Some(other) => {
            return Err(setup_error(
                instance,
                operation,
                &format!("o3-server has no setup flow for the {other} driver."),
            ))
        }
        None => match instance {
            "claudeAgent" | "claude" | "claude-code" => "claudeAgent",
            "codex" => "codex",
            _ => {
                return Err(setup_error(
                    instance,
                    operation,
                    "Unknown provider instance.",
                ))
            }
        },
    };
    let name = if kind == "codex" { "codex" } else { "claude" };
    let binary = crate::provider::instance_binary(configured, name);
    Ok(Driver {
        instance: instance.into(),
        kind,
        binary,
        environment: crate::provider::instance_environment(configured),
    })
}

fn methods(d: &Driver) -> Value {
    let name = if d.kind == "codex" { "Codex" } else { "Claude" };
    json!([{"id":"login","name":format!("Sign in with {name}"),"description":null,"type":"agent"}])
}

fn state_json(d: &Driver, flow: Option<&Flow>) -> Value {
    let mut v = json!({"instanceId":d.instance,"phase":"idle","flowId":null,"authorizationUrl":null,"expiresAt":null,"message":null,"methods":methods(d),"credentialOwner":"provider"});
    if let Some(f) = flow {
        v["phase"] = json!(f.phase);
        v["flowId"] = json!(f.id);
        v["authorizationUrl"] = json!(f.url);
        v["expiresAt"] = json!(f.expires_at.map(iso));
        v["message"] = json!(f.message);
        v["interaction"] = f.interaction.clone().unwrap_or(Value::Null);
    }
    v
}

fn current_auth(s: &State, d: &Driver) -> Value {
    let flows = s.streams.p2.setup.flows.lock().unwrap();
    state_json(d, flows.get(&d.instance))
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn first_url(text: &str) -> Option<String> {
    let i = text.find("https://")?;
    let url: String = text[i..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != '>')
        .collect();
    (url.len() > 8 && url.len() <= 16_384).then_some(url)
}
fn device_code(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|w| {
            w.len() >= 9
                && w.len() <= 16
                && w.matches('-').count() == 1
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
                && w.chars()
                    .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
        .map(str::to_owned)
}

pub async fn call(s: &Arc<State>, tag: &str, p: &Value) -> Reply {
    match tag {
        "provider.auth.start" => auth_start(s, p).await,
        "provider.auth.respond" => auth_respond(s, p).await,
        "provider.auth.cancel" => auth_cancel(s, p),
        "provider.auth.logout" => auth_logout(s, p).await,
        "provider.install.start" => {
            let instance = instance_id(p)?;
            if driver(s, instance, "install")
                .is_ok_and(|d| crate::executable::resolve(&d.binary).is_some())
            {
                return Err(setup_error(
                    instance,
                    tag,
                    "An installed CLI is already configured; use update instead.",
                ));
            }
            start_managed(s, instance, None).await
        }
        "provider.install.remove" => Err(setup_error(
            instance_id(p)?,
            tag,
            "Managed removal is unavailable; no existing CLI is deleted.",
        )),
        "provider.install.cancel" => {
            let id = instance_id(p)?;
            let operation = str_field(p, "operationId")?;
            let state = s
                .streams
                .p2
                .setup
                .installs
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or_else(|| setup_error(id, tag, "No installation is running"))?;
            if state["operationId"] != operation {
                return Err(setup_error(id, tag, "Unknown installation operation"));
            }
            if let Some(cancel) = s.streams.p2.setup.install_cancels.lock().unwrap().get(id) {
                cancel.notify_one();
            }
            Ok(state)
        }
        "server.updateProvider" => update_provider(s, p).await,
        "server.searchAcpRegistry" => search_registry(p).await,
        _ => prepare_registry_agent(p).await,
    }
}

// ---- sign-in ------------------------------------------------------------------------------

async fn auth_start(s: &Arc<State>, p: &Value) -> Reply {
    let d = driver(s, instance_id(p)?, "auth.start")?;
    if let Some(m) = p["methodId"].as_str() {
        if m != "login" {
            return Err(setup_error(
                &d.instance,
                "auth.start",
                "Unknown authentication method.",
            ));
        }
    }
    let (tx, rx) = mpsc::channel::<Vec<u8>>(8);
    let cancel = Arc::new(Notify::new());
    let flow_id = random_id().map_err(|e| setup_error(&d.instance, "auth.start", &e))?;
    {
        let mut flows = s.streams.p2.setup.flows.lock().unwrap();
        if flows
            .get(&d.instance)
            .is_some_and(|f| matches!(f.phase, "starting" | "waiting" | "verifying"))
        {
            return Err(setup_error(
                &d.instance,
                "auth.start",
                "A sign-in flow is already running for this provider.",
            ));
        }
        if flows.len() >= MAX_FLOWS && !flows.contains_key(&d.instance) {
            flows.retain(|_, f| matches!(f.phase, "starting" | "waiting" | "verifying"));
        }
        flows.insert(
            d.instance.clone(),
            Flow {
                id: flow_id.clone(),
                phase: "starting",
                url: None,
                expires_at: Some(now_ms() + FLOW_TIMEOUT.as_millis() as i64),
                message: None,
                interaction: None,
                output: String::new(),
                input: Some(tx),
                cancel: cancel.clone(),
            },
        );
    }
    let args: &[&str] = if d.kind == "codex" {
        &["login", "--device-auth"]
    } else {
        &["auth", "login"]
    };
    let mut cmd = tokio::process::Command::new(&d.binary);
    cmd.envs(d.environment.iter().cloned())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("NO_COLOR", "1")
        .env("BROWSER", "true");
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            s.streams.p2.setup.flows.lock().unwrap().remove(&d.instance);
            return Err(setup_error(
                &d.instance,
                "auth.start",
                &format!("Could not start the CLI: {e}"),
            ));
        }
    };
    s.streams.p2.setup.touch(s);
    tokio::spawn(run_flow(s.clone(), d.clone(), flow_id, child, rx, cancel));
    Ok(current_auth(s, &d))
}

fn update_flow(s: &State, d: &Driver, flow_id: &str, f: impl FnOnce(&mut Flow)) {
    let mut flows = s.streams.p2.setup.flows.lock().unwrap();
    if let Some(flow) = flows.get_mut(&d.instance).filter(|f| f.id == flow_id) {
        f(flow);
    }
    drop(flows);
    s.streams.p2.setup.touch(s);
}

async fn run_flow(
    s: Arc<State>,
    d: Driver,
    flow_id: String,
    mut child: tokio::process::Child,
    mut input: mpsc::Receiver<Vec<u8>>,
    cancel: Arc<Notify>,
) {
    let mut stdin = child.stdin.take();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child.stderr.take().map(|p| Box::new(p) as _),
    ]
    .into_iter()
    .flatten()
    {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe.take(1024 * 1024)).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(line).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(out_tx);
    let deadline = tokio::time::sleep(FLOW_TIMEOUT);
    tokio::pin!(deadline);
    let mut outcome: Option<&'static str> = None;
    let status = loop {
        tokio::select! {
            line = out_rx.recv() => {
                let Some(line) = line else {
                    break child.wait().await.ok();
                };
                let line = strip_ansi(&line);
                let codex = d.kind == "codex";
                update_flow(&s, &d, &flow_id, |f| {
                    if f.output.len() < OUTPUT_CAP {
                        f.output.push_str(&line);
                        f.output.push('\n');
                    }
                    if f.url.is_none() {
                        if let Some(url) = first_url(&line) {
                            f.url = Some(url.clone());
                            f.phase = "waiting";
                            f.interaction = Some(json!({"type":"browser","id":"browser","url":url,"requiresConsent":false}));
                        }
                    }
                    if codex {
                        if let Some(code) = device_code(&line) {
                            if let Some(url) = f.url.clone() {
                                f.interaction = Some(json!({"type":"deviceCode","id":"device","url":url,"userCode":code}));
                            }
                        }
                    }
                });
            }
            Some(bytes) = input.recv() => {
                if let Some(w) = stdin.as_mut() {
                    let _ = w.write_all(&bytes).await;
                    let _ = w.flush().await;
                }
            }
            _ = cancel.notified() => { outcome = Some("cancelled"); let _ = child.start_kill(); break child.wait().await.ok(); }
            _ = &mut deadline => { outcome = Some("expired"); let _ = child.start_kill(); break child.wait().await.ok(); }
        }
    };
    drop(stdin);
    match outcome {
        Some("cancelled") => update_flow(&s, &d, &flow_id, |f| {
            f.phase = "cancelled";
            f.message = Some("Sign-in was cancelled.".into());
            f.input = None;
        }),
        Some(_) => update_flow(&s, &d, &flow_id, |f| {
            f.phase = "failed";
            f.message = Some("Sign-in timed out.".into());
            f.input = None;
        }),
        None => {
            let ok = status.is_some_and(|st| st.success());
            if !ok {
                update_flow(&s, &d, &flow_id, |f| {
                    f.phase = "failed";
                    f.input = None;
                    let tail = f
                        .output
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("The CLI exited without signing in.");
                    f.message = Some(tail.chars().take(500).collect());
                });
            } else {
                update_flow(&s, &d, &flow_id, |f| {
                    f.phase = "verifying";
                    f.input = None;
                });
                let verified = verify(&d).await;
                update_flow(&s, &d, &flow_id, |f| {
                    f.phase = if verified { "succeeded" } else { "failed" };
                    f.message = Some(if verified {
                        "Signed in.".into()
                    } else {
                        "The CLI finished but is still not signed in.".into()
                    });
                });
                // Providers report their auth status in the config snapshot.
                s.notify();
            }
        }
    }
}

async fn verify(d: &Driver) -> bool {
    let args: &[&str] = if d.kind == "codex" {
        &["login", "status"]
    } else {
        &["auth", "status"]
    };
    run_quiet(d, args, Duration::from_secs(20))
        .await
        .is_some_and(|(code, _)| code == 0)
}

async fn run_quiet(d: &Driver, args: &[&str], timeout: Duration) -> Option<(i32, String)> {
    let mut cmd = tokio::process::Command::new(&d.binary);
    cmd.envs(d.environment.iter().cloned());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("NO_COLOR", "1");
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.code().unwrap_or(-1), text))
}

async fn auth_respond(s: &Arc<State>, p: &Value) -> Reply {
    let d = driver(s, instance_id(p)?, "auth.respond")?;
    let flow_id = str_field(p, "flowId")?;
    str_field(p, "interactionId")?;
    let response = &p["response"];
    let (input, cancel) = {
        let flows = s.streams.p2.setup.flows.lock().unwrap();
        let f = flows
            .get(&d.instance)
            .filter(|f| f.id == flow_id && matches!(f.phase, "starting" | "waiting"))
            .ok_or_else(|| {
                setup_error(
                    &d.instance,
                    "auth.respond",
                    "No sign-in flow is waiting for a response.",
                )
            })?;
        (f.input.clone(), f.cancel.clone())
    };
    match response["type"].as_str() {
        Some("browser") if response["action"] == "decline" => cancel.notify_one(),
        Some("browser") if response["action"] == "accept" => {}
        Some("terminal") => {
            let data = response["data"]
                .as_str()
                .ok_or_else(|| invalid("data is required"))?;
            let Some(tx) = input else {
                return Err(setup_error(
                    &d.instance,
                    "auth.respond",
                    "The CLI is no longer reading input.",
                ));
            };
            tx.send(data.as_bytes().to_vec()).await.map_err(|_| {
                setup_error(
                    &d.instance,
                    "auth.respond",
                    "The CLI is no longer reading input.",
                )
            })?;
        }
        Some("credentials") => {
            return Err(setup_error(
                &d.instance,
                "auth.respond",
                "This provider does not take credentials through o3-server.",
            ))
        }
        _ => return Err(invalid("response is invalid")),
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(current_auth(s, &d))
}

fn auth_cancel(s: &Arc<State>, p: &Value) -> Reply {
    let d = driver(s, instance_id(p)?, "auth.cancel")?;
    let flow_id = str_field(p, "flowId")?;
    let cancel = {
        let flows = s.streams.p2.setup.flows.lock().unwrap();
        flows
            .get(&d.instance)
            .filter(|f| f.id == flow_id && matches!(f.phase, "starting" | "waiting" | "verifying"))
            .map(|f| f.cancel.clone())
    };
    if let Some(c) = cancel {
        c.notify_one();
    }
    Ok(current_auth(s, &d))
}

async fn auth_logout(s: &Arc<State>, p: &Value) -> Reply {
    let d = driver(s, instance_id(p)?, "auth.logout")?;
    let args: &[&str] = if d.kind == "codex" {
        &["logout"]
    } else {
        &["auth", "logout"]
    };
    let (code, text) = run_quiet(&d, args, Duration::from_secs(30))
        .await
        .ok_or_else(|| setup_error(&d.instance, "auth.logout", "The CLI did not answer."))?;
    if code != 0 {
        return Err(setup_error(
            &d.instance,
            "auth.logout",
            text.trim()
                .lines()
                .last()
                .unwrap_or("The CLI could not sign out."),
        ));
    }
    s.streams.p2.setup.flows.lock().unwrap().remove(&d.instance);
    s.streams.p2.setup.touch(s);
    s.notify();
    Ok(current_auth(s, &d))
}

// ---- install state ------------------------------------------------------------------------

fn install_json(
    driver: &str,
    path: Option<&PathBuf>,
    version: Option<String>,
    message: &str,
) -> Value {
    json!({"driver":driver,"operationId":null,"phase":"idle","downloadedBytes":0,"totalBytes":null,"version":null,
        "installedVersion":version,"executablePath":path.map(|p|p.to_string_lossy().into_owned()),
        "source":path.map(|_|"local"),"canRemove":false,"message":message})
}

// ---- update -------------------------------------------------------------------------------

async fn update_provider(s: &Arc<State>, p: &Value) -> Reply {
    let provider = str_field(p, "provider")?;
    let fail = |reason: &str| json!({"_tag":"ServerProviderUpdateError","provider":provider,"reason":reason,"message":format!("Provider update failed for {provider}: {reason}")});
    let instance = p["instanceId"].as_str().unwrap_or(provider);
    let settings = crate::settings::read(s).map_err(|e| fail(&e))?;
    let missing_builtin = matches!(instance, "codex" | "claudeAgent")
        && driver(s, instance, "update")
            .is_ok_and(|d| crate::executable::resolve(&d.binary).is_none());
    if missing_builtin
        || managed_binary(s, instance).is_some_and(|path| {
            settings["providerInstances"][instance]["config"]["binaryPath"]
                == path.to_string_lossy().as_ref()
        })
    {
        if provider != instance {
            return Err(fail("Provider driver does not match built-in instance"));
        }
        let mut changes = s.streams.changed.subscribe();
        start_managed(s, instance, p["targetVersion"].as_str())
            .await
            .map_err(|e| fail(e["detail"].as_str().unwrap_or("managed update failed")))?;
        loop {
            let state = s
                .streams
                .p2
                .setup
                .installs
                .lock()
                .unwrap()
                .get(instance)
                .cloned()
                .unwrap_or(Value::Null);
            match state["phase"].as_str() {
                Some("succeeded") => {
                    return Ok(
                        json!({"providers":s.provider.lock().unwrap().as_ref().map(|p|p.config()).unwrap_or_default()}),
                    )
                }
                Some("failed" | "cancelled") => {
                    return Err(fail("Managed provider update did not complete"))
                }
                _ => {}
            }
            changes
                .changed()
                .await
                .map_err(|_| fail("Server stopped during update"))?;
        }
    }
    if provider != "claudeAgent" {
        return Err(fail("o3-server can only update the Claude CLI (`claude update`); update this provider with its package manager"));
    }
    if p["targetVersion"].as_str().is_some() {
        return Err(fail(
            "the Claude CLI updates to its latest release; pinning a version is not supported",
        ));
    }
    let instance = p["instanceId"].as_str().unwrap_or("claudeAgent");
    let d = driver(s, instance, "update")
        .map_err(|e| fail(e["detail"].as_str().unwrap_or("provider unavailable")))?;
    if d.kind != "claudeAgent" {
        return Err(fail("the instance does not use the Claude driver"));
    }
    let started = iso(now_ms());
    s.streams.p2.setup.updates.lock().unwrap().insert(instance.into(),json!({"status":"running","startedAt":started,"finishedAt":null,"message":null,"output":null}));
    s.notify();
    let result = run_quiet(&d, &["update"], Duration::from_secs(300)).await;
    s.streams.p2.setup.updates.lock().unwrap().insert(instance.into(),json!({"status":if result.as_ref().is_some_and(|(code,_)|*code==0){"succeeded"}else{"failed"},"startedAt":started,"finishedAt":iso(now_ms()),"message":null,"output":result.as_ref().map(|(_,text)|text.chars().take(10000).collect::<String>())}));
    s.notify();
    let (code, text) = result.ok_or_else(|| fail("`claude update` did not finish"))?;
    if code != 0 {
        return Err(fail(
            text.trim()
                .lines()
                .last()
                .unwrap_or("`claude update` failed"),
        ));
    }
    let registry = s.provider.lock().unwrap().clone();
    if let Some(provider) = registry {
        let _ = provider.refresh().await;
    }
    check_versions(s).await;
    s.notify();
    let providers = s
        .provider
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.config())
        .unwrap_or_default();
    Ok(json!({"providers":providers}))
}

// ---- ACP registry -------------------------------------------------------------------------

fn registry_error(reason: &str, message: &str) -> Value {
    json!({"_tag":"AcpRegistryOperationError","reason":reason,"message":message})
}

async fn fetch_registry() -> Result<Vec<Value>, Value> {
    let url = std::env::var("O3_ACP_REGISTRY_URL").unwrap_or_else(|_| REGISTRY_URL.into());
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-fsSL",
        "--max-time",
        "20",
        "--max-filesize",
        "4194304",
        "--proto",
        "=http,https",
        "--",
        &url,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(25), cmd.output())
        .await
        .map_err(|_| registry_error("registry_unavailable", "The ACP registry timed out."))?
        .map_err(|e| {
            registry_error(
                "registry_unavailable",
                &format!("curl is required to read the ACP registry: {e}"),
            )
        })?;
    if !out.status.success() {
        return Err(registry_error(
            "registry_unavailable",
            "The ACP registry could not be downloaded.",
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        registry_error(
            "registry_unavailable",
            "The ACP registry is not valid JSON.",
        )
    })?;
    Ok(v["agents"].as_array().cloned().unwrap_or_default())
}

fn distribution(agent: &Value) -> Option<&'static str> {
    let d = &agent["distribution"];
    ["npx", "uvx", "binary"]
        .into_iter()
        .find(|k| d.get(*k).is_some_and(|v| !v.is_null()))
}

async fn search_registry(p: &Value) -> Reply {
    let query = p["query"]
        .as_str()
        .ok_or_else(|| invalid("query is required"))?
        .trim()
        .to_lowercase();
    let limit = p["limit"].as_u64().unwrap_or(512).clamp(1, 512) as usize;
    let agents = fetch_registry().await?;
    let mut out = Vec::new();
    for a in agents {
        let (Some(id), Some(name), Some(version)) =
            (a["id"].as_str(), a["name"].as_str(), a["version"].as_str())
        else {
            continue;
        };
        let Some(dist) = distribution(&a) else {
            continue;
        };
        if !query.is_empty()
            && ![id, name, a["description"].as_str().unwrap_or("")]
                .iter()
                .any(|f| f.to_lowercase().contains(&query))
        {
            continue;
        }
        let icon = a["icon"].as_str().and_then(|i| {
            let u = url::Url::parse(i).ok()?;
            (u.scheme() == "https"
                && u.host_str() == Some("cdn.agentclientprotocol.com")
                && u.port().is_none()
                && u.username().is_empty())
            .then(|| u.to_string())
        });
        out.push(json!({"id":id,"name":name,"version":version,"description":a["description"].as_str().unwrap_or(""),
            "authors":a["authors"].as_array().map(|x|x.iter().filter_map(Value::as_str).take(16).collect::<Vec<_>>()).unwrap_or_default(),
            "license":a["license"].as_str(),"website":a["website"].as_str(),"repository":a["repository"].as_str(),"icon":icon,
            "distribution":dist,"integrity":if dist=="binary"{"sha256"}else{"registry"}}));
        if out.len() >= limit {
            break;
        }
    }
    Ok(json!({"agents":out}))
}

async fn prepare_registry_agent(p: &Value) -> Reply {
    let id = str_field(p, "agentId")?;
    let agents = fetch_registry().await?;
    let agent = agents.iter().find(|a| a["id"] == id).ok_or_else(|| {
        registry_error(
            "agent_not_found",
            &format!("The ACP registry has no agent {id}."),
        )
    })?;
    let dist = distribution(agent).ok_or_else(|| {
        registry_error(
            "unsupported_distribution",
            "The agent has no supported distribution.",
        )
    })?;
    match dist {
        "npx" | "uvx" => {
            if which(dist).is_none() {
                return Err(registry_error(
                    "runner_unavailable",
                    &format!("{dist} is not installed or not on PATH."),
                ));
            }
            Err(registry_error(
                "execution_unavailable",
                "No ACP execution driver is available; this registry agent was not prepared.",
            ))
        }
        _ => Err(registry_error(
            "unsupported_distribution",
            "o3-server does not download or unpack binary ACP agents.",
        )),
    }
}

// ---- streams ------------------------------------------------------------------------------

pub struct Cursor {
    install: bool,
    instance: String,
    seen: Option<u64>,
    waiting: bool,
    pending: Option<tokio::sync::oneshot::Receiver<Value>>,
}

pub fn open_stream(s: &Arc<State>, tag: &str, p: &Value) -> Result<Cursor, Value> {
    let instance = instance_id(p)?.to_owned();
    let install = tag == "provider.install.subscribe";
    // Fail the subscription now for unknown instances (a missing CLI is still a valid state).
    if let Err(e) = driver(s, &instance, "subscribe") {
        if !(install
            || e["detail"]
                .as_str()
                .is_some_and(|d| d.contains("not installed")))
        {
            return Err(e);
        }
    }
    Ok(Cursor {
        install,
        instance,
        seen: None,
        waiting: false,
        pending: None,
    })
}

impl Cursor {
    pub fn next(&mut self, s: &State) -> Step {
        if self.waiting {
            return Step::Idle;
        }
        let version = s.streams.p2.setup.version.load(Ordering::Relaxed);
        if self.seen == Some(version) {
            return Step::Idle;
        }
        self.seen = Some(version);
        self.waiting = true;
        if self.install {
            if let Some(value) = s
                .streams
                .p2
                .setup
                .installs
                .lock()
                .unwrap()
                .get(&self.instance)
                .cloned()
            {
                return Step::Values(vec![value]);
            }
            // The CLI version probe is a subprocess; do it off the socket loop.
            let _ = &self.pending;
            let d = driver(s, &self.instance, "subscribe");
            let mut state = match d {
                Ok(d) => {
                    let version = std::process::Command::new(&d.binary)
                        .envs(d.environment.iter().cloned())
                        .arg("--version")
                        .stdin(Stdio::null())
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .and_then(|o| {
                            String::from_utf8_lossy(&o.stdout)
                                .split_whitespace()
                                .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
                                .map(str::to_owned)
                        });
                    install_json(
                        d.kind,
                        Some(&d.binary),
                        version,
                        "Installed outside o3-server.",
                    )
                }
                Err(e) => install_json(
                    if self.instance == "codex" {
                        "codex"
                    } else {
                        "claudeAgent"
                    },
                    None,
                    None,
                    e["detail"].as_str().unwrap_or("Not installed."),
                ),
            };
            if state["executablePath"]
                .as_str()
                .is_some_and(|p| std::path::Path::new(p).starts_with(s.dir.join("provider-clis")))
            {
                state["source"] = json!("managed");
                state["message"] = json!("Managed provider CLI");
            }
            return Step::Values(vec![state]);
        }
        match driver(s, &self.instance, "subscribe") {
            Ok(d) => Step::Values(vec![{
                let flows = s.streams.p2.setup.flows.lock().unwrap();
                state_json(&d, flows.get(&self.instance))
            }]),
            Err(_) => Step::Done,
        }
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
}

fn managed_binary(s: &State, id: &str) -> Option<PathBuf> {
    let name = match id {
        "codex" => "codex",
        "claudeAgent" => "claude",
        _ => return None,
    };
    let root = s.dir.join("provider-clis").join(id);
    if let Some(path) = crate::settings::read(s).ok().and_then(|v| {
        v["providerInstances"][id]["config"]["binaryPath"]
            .as_str()
            .map(PathBuf::from)
    }) {
        if path.starts_with(&root) && path.ends_with(format!("node_modules/.bin/{name}")) {
            return Some(path);
        }
    }
    Some(root.join("node_modules/.bin").join(name))
}
async fn start_managed(s: &Arc<State>, id: &str, version: Option<&str>) -> Reply {
    let _identity = managed_binary(s, id).ok_or_else(|| {
        setup_error(
            id,
            "install",
            "Only built-in provider identities support managed installation",
        )
    })?;
    let npm = which("npm").ok_or_else(|| setup_error(id, "install", "npm is unavailable"))?;
    if which("node").is_none() {
        return Err(setup_error(id, "install", "node is unavailable"));
    }
    let version = version.unwrap_or("latest");
    if version.is_empty()
        || version.len() > 80
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err(setup_error(id, "install", "Invalid provider version"));
    }
    let package = format!(
        "{}@{version}",
        if id == "codex" {
            "@openai/codex"
        } else {
            "@anthropic-ai/claude-code"
        }
    );
    let operation = random_id().map_err(|e| setup_error(id, "install", &e))?;
    let prefix = s
        .dir
        .join("provider-clis")
        .join(id)
        .join("versions")
        .join(&operation);
    let binary =
        prefix
            .join("node_modules/.bin")
            .join(if id == "codex" { "codex" } else { "claude" });
    std::fs::create_dir_all(&prefix).map_err(|e| setup_error(id, "install", &e.to_string()))?;
    if !prefix
        .canonicalize()
        .map_err(|e| setup_error(id, "install", &e.to_string()))?
        .starts_with(
            s.dir
                .canonicalize()
                .map_err(|e| setup_error(id, "install", &e.to_string()))?,
        )
    {
        return Err(setup_error(
            id,
            "install",
            "Managed directory escapes server data directory",
        ));
    }

    let cancel = Arc::new(Notify::new());
    {
        let mut running = s.streams.p2.setup.install_cancels.lock().unwrap();
        if !running.is_empty() {
            return Err(setup_error(
                id,
                "install",
                "Another managed installation is running",
            ));
        }
        running.insert(id.into(), cancel.clone());
    }
    let state = json!({"driver":id,"operationId":operation,"phase":"downloading","downloadedBytes":0,"totalBytes":null,"version":version,"installedVersion":null,"executablePath":null,"source":"managed","canRemove":false,"message":null});
    s.streams
        .p2
        .setup
        .installs
        .lock()
        .unwrap()
        .insert(id.into(), state.clone());
    s.streams.p2.setup.updates.lock().unwrap().insert(id.into(),json!({"status":"running","startedAt":iso(now_ms()),"finishedAt":null,"message":null,"output":null}));
    s.streams.p2.setup.touch(s);
    s.notify();
    let s = s.clone();
    let id = id.to_owned();
    let mut final_state = state.clone();
    tokio::spawn(async move {
        let cache = s.dir.join("npm-cache");
        let user_config = prefix.join("npm-user.conf");
        let global_config = prefix.join("npm-global.conf");
        let configs_ready = std::fs::write(&user_config, "")
            .and_then(|_| std::fs::write(&global_config, ""))
            .is_ok();
        let claude_config = prefix.join("claude-config");
        let xdg_cache = prefix.join("cache");
        let environment = [
            ("HOME", prefix.as_path()),
            ("CLAUDE_CONFIG_DIR", claude_config.as_path()),
            ("XDG_CACHE_HOME", xdg_cache.as_path()),
        ];
        let args = vec![
            "--userconfig".into(),
            user_config.to_string_lossy().into_owned(),
            "--globalconfig".into(),
            global_config.to_string_lossy().into_owned(),
            "--cache".into(),
            cache.to_string_lossy().into_owned(),
            "install".into(),
            "--prefix".into(),
            prefix.to_string_lossy().into_owned(),
            if id == "claudeAgent" {
                "--ignore-scripts=false"
            } else {
                "--ignore-scripts"
            }
            .into(),
            "--no-audit".into(),
            "--no-fund".into(),
            package,
        ];
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = if !configs_ready {
            None
        } else {
            tokio::select! {
                _=cancel.notified()=>None,
                r=crate::proc::run(npm.to_str().unwrap_or("npm"),&prefix,&refs,crate::proc::Opts{timeout:Duration::from_secs(300),cap:OUTPUT_CAP,allow_truncate:true},&environment)=>Some(r),
            }
        };
        final_state["phase"] = json!(match &result {
            None => "cancelled",
            Some(Ok(o)) if o.ok() && binary.is_file() => "succeeded",
            _ => "failed",
        });
        if final_state["phase"] == "succeeded" {
            let manifest = prefix
                .join("node_modules")
                .join(if id == "codex" {
                    "@openai/codex"
                } else {
                    "@anthropic-ai/claude-code"
                })
                .join("package.json");
            let version = std::fs::read(manifest)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v["version"].as_str().map(str::to_owned));
            if let Some(version) = version {
                let runnable = crate::proc::run(
                    binary.to_str().unwrap_or(""),
                    &prefix,
                    &["--version"],
                    crate::proc::Opts {
                        timeout: Duration::from_secs(20),
                        cap: 4096,
                        allow_truncate: false,
                    },
                    &environment,
                )
                .await
                .is_ok_and(|o| {
                    o.ok() && !o.truncated && String::from_utf8_lossy(&o.stdout).contains(&version)
                });
                if runnable {
                    final_state["version"] = json!(version);
                } else {
                    final_state["phase"] = json!("failed");
                }
            } else {
                final_state["phase"] = json!("failed");
            }
        }
        if final_state["phase"] == "succeeded" {
            let mut instances = crate::settings::read(&s)
                .ok()
                .and_then(|v| v["providerInstances"].as_object().cloned())
                .unwrap_or_default();
            let instance = instances.entry(id.clone()).or_insert_with(|| json!({}));
            instance["driver"] = json!(id);
            instance["enabled"] = json!(true);
            instance["config"]["binaryPath"] = json!(binary);
            let patch = json!({"patch":{"providerInstances":instances}});
            if crate::settings::update(&s, &patch).is_err() {
                final_state["phase"] = json!("failed");
            } else {
                final_state["executablePath"] = json!(binary);
                final_state["installedVersion"] = final_state["version"].clone();
            }
        }
        final_state["message"] = json!(if final_state["phase"] == "succeeded" {
            "Managed CLI installed"
        } else {
            "Managed installation did not complete; no existing CLI was replaced"
        });
        if let Some(update) = s.streams.p2.setup.updates.lock().unwrap().get_mut(&id) {
            update["status"] = json!(if final_state["phase"] == "succeeded" {
                "succeeded"
            } else {
                "failed"
            });
            update["finishedAt"] = json!(iso(now_ms()));
        }
        s.streams
            .p2
            .setup
            .installs
            .lock()
            .unwrap()
            .insert(id.clone(), final_state);
        s.streams
            .p2
            .setup
            .install_cancels
            .lock()
            .unwrap()
            .remove(&id);
        s.streams.p2.setup.touch(&s);
        s.notify();
    });
    Ok(state)
}
