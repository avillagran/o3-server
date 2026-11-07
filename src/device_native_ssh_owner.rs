//! Owned SSH hub lifecycle over verified-installed remote tools. No installer or credentials store.
use crate::{
    device_native_client::{HostTools, HubClient},
    device_native_hub::{CommandResult, Platform},
    device_native_owner::{host_error, HostFuture, ReadyHost, SpawnedHub},
    device_native_ssh::{self as plan, RemoteMode},
    device_native_ssh_transport::{Cli, ForwardLease, ForwardOptions, RemoteHub},
};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

type StatusCallback = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;
#[derive(Default)]
struct State {
    current: Option<ReadyHost>,
    remote: Option<RemoteHub>,
    lease: Option<ForwardLease>,
    generation: u64,
    stopped: bool,
    failure: Option<Value>,
    probe: Option<Value>,
}
struct Inner {
    cli: Arc<Cli>,
    operation: AsyncMutex<()>,
    state: Mutex<State>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    status: StatusCallback,
    options: ForwardOptions,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = self.supervisor.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
#[derive(Clone)]
pub struct SshOwner {
    inner: Arc<Inner>,
}
impl SshOwner {
    pub fn new(cli: Arc<Cli>) -> Self {
        Self {
            inner: Arc::new(Inner {
                cli,
                operation: AsyncMutex::new(()),
                state: Mutex::new(State::default()),
                supervisor: Mutex::new(None),
                status: Arc::new(|_, _| {}),
                options: ForwardOptions::default(),
            }),
        }
    }
    pub fn with_status(mut self, callback: StatusCallback) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("configure owner before cloning")
            .status = callback;
        self
    }
    pub fn with_forward_options(mut self, options: ForwardOptions) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("configure owner before cloning")
            .options = options;
        self
    }
    pub fn current(&self) -> Option<ReadyHost> {
        self.inner
            .state
            .lock()
            .unwrap()
            .current
            .as_ref()
            .filter(|ready| ready.hub.process.alive())
            .cloned()
    }
    pub fn failure(&self) -> Option<Value> {
        self.inner.state.lock().unwrap().failure.clone()
    }
    pub async fn probe(&self) -> Result<Value, Value> {
        let probe = self.inner.cli.request(RemoteMode::Probe, None).await?;
        self.inner.state.lock().unwrap().probe = Some(probe.clone());
        Ok(probe)
    }
    pub fn summary(&self) -> Option<Value> {
        self.inner.state.lock().unwrap().probe.clone()
    }
    pub async fn ensure_ready(&self) -> Result<ReadyHost, Value> {
        let _lock = self.inner.operation.lock().await;
        self.inner.state.lock().unwrap().stopped = false;
        if let Some(ready) = self.current() {
            return Ok(ready);
        }
        let probe = self.probe().await?;
        if probe["verifiedInstalled"] != true {
            return Err(host_error(
                &self.inner.cli.config.id,
                "checking device support",
                "Pinned remote tools are not verified-installed; acquisition is not enabled",
            ));
        }
        (self.inner.status)("starting", None);
        let ready = connect(&self.inner).await?;
        Self::supervise(&self.inner, ready.clone());
        Ok(ready)
    }
    fn supervise(inner: &Arc<Inner>, mut ready: ReadyHost) {
        let weak: Weak<Inner> = Arc::downgrade(inner);
        let task = tokio::spawn(async move {
            loop {
                ready.hub.process.wait().await;
                {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let mut state = inner.state.lock().unwrap();
                    if state.stopped || state.generation != ready.generation {
                        return;
                    }
                    state.current = None;
                    drop(state);
                    (inner.status)("starting", Some("Reconnecting to device host…"));
                }
                let mut delay = Duration::from_secs(1);
                loop {
                    tokio::time::sleep(delay).await;
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let _lock = inner.operation.lock().await;
                    {
                        let state = inner.state.lock().unwrap();
                        if state.stopped
                            || state.current.is_some()
                            || state.generation != ready.generation
                        {
                            return;
                        }
                    }
                    match connect(&inner).await {
                        Ok(replacement) => {
                            ready = replacement;
                            break;
                        }
                        Err(error) => {
                            inner.state.lock().unwrap().failure = Some(error.clone());
                            (inner.status)("failed", error["message"].as_str());
                            delay = plan::next_reconnect_delay(delay);
                        }
                    }
                }
            }
        });
        if let Some(old) = inner.supervisor.lock().unwrap().replace(task) {
            old.abort();
        }
    }
    /// Close the local forward before stopping precisely the remote receipt this owner acquired.
    pub async fn stop(&self) -> Result<(), Value> {
        let _lock = self.inner.operation.lock().await;
        if let Some(task) = self.inner.supervisor.lock().unwrap().take() {
            task.abort();
        }
        let (current, remote) = {
            let mut state = self.inner.state.lock().unwrap();
            state.stopped = true;
            state.generation += 1;
            state.lease.take();
            (state.current.take(), state.remote.clone())
        };
        if let Some(current) = current {
            current.hub.process.stop().await;
        }
        if let Some(remote) = remote {
            if let Err(error) = self.inner.cli.stop(&remote.receipt).await {
                self.inner.state.lock().unwrap().failure = Some(error.clone());
                return Err(error);
            }
            self.inner.state.lock().unwrap().remote = None;
        }
        self.inner.state.lock().unwrap().failure = None;
        Ok(())
    }
}
async fn connect(inner: &Arc<Inner>) -> Result<ReadyHost, Value> {
    for attempt in 0..3 {
        let remote = inner.cli.start().await?;
        {
            let mut state = inner.state.lock().unwrap();
            state.remote = Some(remote.clone());
            if let Some(probe) = &mut state.probe {
                probe["platforms"] = remote.platforms.clone();
                probe["verifiedInstalled"] = serde_json::json!(true);
            }
        }
        let forward = match inner.cli.forward(&remote, inner.options).await {
            Ok(forward) => forward,
            Err(error) => {
                if plan::retry_bind(attempt, error["step"].as_str().unwrap_or("")) {
                    continue;
                }
                return Err(error);
            }
        };
        let tools = Arc::new(RemoteTools {
            cli: inner.cli.clone(),
            node_path: remote.node_path.clone(),
            platforms: Mutex::new(remote.platforms.clone()),
            owner: Arc::downgrade(inner),
        });
        let client = HubClient::new(inner.cli.config.id.clone(), &forward.origin, tools)
            .map_err(|error| host_error(&inner.cli.config.id, "forwarding ports", error))?;
        let ready = {
            let mut state = inner.state.lock().unwrap();
            state.generation += 1;
            let ready = ReadyHost {
                generation: state.generation,
                hub: SpawnedHub {
                    process: forward.process,
                    client: Arc::new(client),
                    origin: forward.origin,
                },
            };
            state.lease = Some(forward.lease);
            state.current = Some(ready.clone());
            state.failure = None;
            ready
        };
        (inner.status)("ready", None);
        return Ok(ready);
    }
    unreachable!("last failed forward returns its native host error")
}
struct RemoteTools {
    cli: Arc<Cli>,
    node_path: String,
    platforms: Mutex<Value>,
    owner: Weak<Inner>,
}
impl HostTools for RemoteTools {
    fn platform_available(&self, platform: Platform) -> HostFuture<'_, bool> {
        Box::pin(async move {
            let available = |platforms: &Value| {
                platforms.as_array().is_some_and(|platforms| {
                    platforms.iter().any(|value| {
                        value["platform"] == platform.label() && value["available"] == true
                    })
                })
            };
            if available(&self.platforms.lock().unwrap()) {
                return true;
            }
            let Ok(probe) = self.cli.request(RemoteMode::Probe, None).await else {
                return false;
            };
            if let Some(owner) = self.owner.upgrade() {
                owner.state.lock().unwrap().probe = Some(probe.clone());
            }
            let result = available(&probe["platforms"]);
            *self.platforms.lock().unwrap() = probe["platforms"].clone();
            result
        })
    }
    fn run<'a>(&'a self, command: &'a str, args: &'a [String]) -> HostFuture<'a, CommandResult> {
        Box::pin(async move { self.cli.run(command, args).await })
    }
    fn video_encoder(&self) -> HostFuture<'_, Result<(), Value>> {
        Box::pin(async move {
            // Keep resolution on the remote host, in the same cwd/environment as its hub.
            let script="const { spawnSync } = require(\"node:child_process\");\nconst result = spawnSync(process.env.SERVE_EMU_FFMPEG?.trim() || \"ffmpeg\", [\"-hide_banner\", \"-encoders\"], { stdio: \"inherit\", timeout: 10000, killSignal: \"SIGKILL\" });\nprocess.exitCode = result.error ? 1 : (result.status ?? 1);";
            let output = self
                .cli
                .run(&self.node_path, &["-e".into(), script.into()])
                .await;
            let mut error = if output.code != 0 {
                Some(crate::device_native_client::video_encoder_error(false))
            } else {
                let pattern = regex::Regex::new(r"(?m)^\s*V\S*[ \t]+libx264(?:[ \t]|$)")
                    .expect("native encoder pattern");
                (!pattern.is_match(&format!("{}\n{}", output.stdout, output.stderr)))
                    .then(|| crate::device_native_client::video_encoder_error(true))
            };
            if let Some(error) = &mut error {
                error["hostId"] = serde_json::json!(self.cli.config.id);
                if let Some(reason) = error["reason"].as_str() {
                    error["message"] = serde_json::json!(format!(
                        "android devices are unavailable on host {}: {reason}",
                        self.cli.config.id
                    ));
                }
            }
            error.map_or(Ok(()), Err)
        })
    }
}
