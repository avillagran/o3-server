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
        self.inner.cli.request(RemoteMode::Probe, None).await
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
        inner.state.lock().unwrap().remote = Some(remote.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_native_ssh::Config;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicBool, Ordering},
    };
    struct Fixture {
        cli: Arc<Cli>,
        healthy: Arc<AtomicBool>,
        server: tokio::task::JoinHandle<()>,
        trace: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl Fixture {
        async fn make(fail_forwards: u8) -> Self {
            crate::test_sandbox::assert_isolated();
            use http_body_util::Full;
            use hyper::{service::service_fn, Response};
            use hyper_util::rt::TokioIo;
            let base = std::env::temp_dir().join(format!(
                "private-ssh-{}",
                o3_server::store::random_id().unwrap()
            ));
            std::fs::create_dir(&base).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let healthy = Arc::new(AtomicBool::new(true));
            let ready = healthy.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (socket, _) = listener.accept().await.unwrap();
                    let healthy = ready.clone();
                    tokio::spawn(async move {
                        let service =
                            service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                                let healthy = healthy.clone();
                                async move {
                                    let status = if request.uri().path() == "/readyz"
                                        && !healthy.load(Ordering::Acquire)
                                    {
                                        503
                                    } else {
                                        200
                                    };
                                    let body = if request.uri().path() == "/api/devices" {
                                        r#"{"simulators":[],"emulators":[]}"#
                                    } else {
                                        "ready"
                                    };
                                    Ok::<_, std::convert::Infallible>(
                                        Response::builder()
                                            .status(status)
                                            .body(Full::new(hyper::body::Bytes::from(body)))
                                            .unwrap(),
                                    )
                                }
                            });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(socket), service)
                            .await;
                    });
                }
            });
            let executable = base.join("ssh-private-cli.py");
            std::fs::copy(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/device/ssh-private-cli.py"),
                &executable,
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let trace = base.join("trace.jsonl");
            std::fs::write(executable.with_extension("json"),serde_json::to_vec(&serde_json::json!({"remotePort":port,"receipt":"c".repeat(48),"trace":trace,"counter":base.join("counter"),"failForwards":fail_forwards})).unwrap()).unwrap();
            let config=Config::decode(&serde_json::json!({"id":"mini","label":"Private SSH","target":"user@private.invalid","port":2222,"identityFile":"/private/key with ' quote"})).unwrap();
            let owner = plan::owner(Some("private-environment"), "/private/state", &config.id);
            let cli = Arc::new(Cli::new(executable, base, config, owner).unwrap());
            Self {
                cli,
                healthy,
                server,
                trace,
            }
        }
        fn owner(&self) -> SshOwner {
            SshOwner::new(self.cli.clone()).with_forward_options(ForwardOptions {
                ready_timeout: Duration::from_millis(400),
                probe_interval: Duration::from_millis(30),
                probe_timeout: Duration::from_millis(50),
            })
        }
        fn trace(&self) -> Vec<Value> {
            std::fs::read_to_string(&self.trace)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }
    #[tokio::test]
    async fn actual_private_cli_forward_retry_hub_and_receipt_stop() {
        let fixture = Fixture::make(2).await;
        let owner = fixture.owner();
        let (first, second) = tokio::join!(owner.ensure_ready(), owner.ensure_ready());
        let first = first.unwrap();
        assert_eq!(first.generation, second.unwrap().generation);
        assert!(first.hub.client.list().await.unwrap().devices.is_empty());
        let trace = fixture.trace();
        assert_eq!(
            trace
                .iter()
                .filter(|entry| entry["mode"] == "forward")
                .count(),
            3
        );
        assert_eq!(
            trace
                .iter()
                .filter(|entry| entry["mode"] == "start")
                .count(),
            3
        );
        owner.stop().await.unwrap();
        assert!(owner.current().is_none());
        assert!(!first.hub.process.alive());
        let trace = fixture.trace();
        let stopped = trace.iter().find(|entry| entry["mode"] == "stop").unwrap();
        assert_eq!(stopped["receipt"], "c".repeat(48));
        assert_eq!(stopped["owner"], fixture.cli.owner);
    }
    #[tokio::test]
    async fn three_unhealthy_probes_reconnect_and_drop_closes_forward_with_live_ready_handle() {
        let fixture = Fixture::make(0).await;
        let owner = fixture.owner();
        let first = owner.ensure_ready().await.unwrap();
        fixture.healthy.store(false, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(3), first.hub.process.wait())
            .await
            .unwrap();
        fixture.healthy.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if owner
                    .current()
                    .is_some_and(|ready| ready.generation > first.generation)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let current = owner.current().unwrap();
        drop(owner);
        tokio::time::timeout(Duration::from_secs(3), current.hub.process.wait())
            .await
            .unwrap();
        assert!(!current.hub.process.alive());
        // Explicit teardown remains available with the retained receipt after local owner release.
        fixture.cli.stop(&"c".repeat(48)).await.unwrap();
    }
}
