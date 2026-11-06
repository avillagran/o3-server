//! Owned SSH child transports. Only explicit argv and owner-bound loopback forwards are used.
use crate::device_native_owner::{host_error, HostFuture, HubProcess};
use crate::device_native_ssh::{self as plan, Config, RemoteMode};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::{oneshot, watch},
};

struct Group(u32);
impl Group {
    fn terminate(&self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGTERM);
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
#[derive(Clone)]
pub struct Cli {
    executable: PathBuf,
    cwd: PathBuf,
    pub config: Config,
    pub owner: String,
}
#[derive(Clone, Debug)]
pub struct RemoteHub {
    pub node_path: String,
    pub port: u16,
    pub receipt: String,
    pub platforms: Value,
    pub helpers: Value,
}
fn remote_error(config: &Config, step: &str, cause: impl ToString) -> Value {
    host_error(&config.id, step, cause)
}
async fn read_capped<R: AsyncRead + Unpin>(mut source: R, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let size = source
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if size == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(size) > limit {
            return Err("SSH output exceeded its size limit".into());
        }
        output.extend_from_slice(&buffer[..size]);
    }
}
impl Cli {
    pub fn new(
        executable: PathBuf,
        cwd: PathBuf,
        config: Config,
        owner: String,
    ) -> Result<Self, String> {
        if !executable.is_absolute() || !cwd.is_absolute() {
            return Err("SSH executable and working directory must be absolute".into());
        }
        Config::decode(&serde_json::to_value(&config).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        plan::verified_remote_script(&owner, RemoteMode::Probe, None)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            executable,
            cwd,
            config,
            owner,
        })
    }
    fn command(&self, args: &[String]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        command
    }
    async fn output(
        &self,
        args: Vec<String>,
        stdin: Option<String>,
        timeout: Duration,
        step: &str,
    ) -> Result<(Vec<u8>, String, i32), Value> {
        let mut child = self
            .command(&args)
            .spawn()
            .map_err(|error| remote_error(&self.config, step, error))?;
        let group = Group(child.id().expect("spawned SSH child"));
        let mut input = child.stdin.take().expect("piped SSH stdin");
        let stdout = child.stdout.take().expect("piped SSH stdout");
        let stderr = child.stderr.take().expect("piped SSH stderr");
        let work = async {
            let write = async {
                if let Some(stdin) = stdin {
                    input
                        .write_all(stdin.as_bytes())
                        .await
                        .map_err(|error| error.to_string())?;
                }
                drop(input);
                Ok::<(), String>(())
            };
            let (_, stdout, stderr) = tokio::try_join!(
                write,
                read_capped(stdout, 8 * 1024 * 1024),
                read_capped(stderr, 64 * 1024)
            )?;
            let stderr = String::from_utf8_lossy(&stderr).into_owned();
            let status = child.wait().await.map_err(|error| error.to_string())?;
            Ok::<_, String>((stdout, stderr, status.code().unwrap_or(127)))
        };
        let result = tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| remote_error(&self.config, step, "SSH device operation timed out"))?
            .map_err(|error| remote_error(&self.config, step, error));
        drop(group);
        result
    }
    pub async fn request(&self, mode: RemoteMode, receipt: Option<&str>) -> Result<Value, Value> {
        let script = plan::verified_remote_script(&self.owner, mode, receipt)
            .map_err(|error| remote_error(&self.config, mode.label(), error))?;
        let (stdout, stderr, code) = self
            .output(
                plan::bootstrap_argv(&self.config),
                Some(script),
                plan::bootstrap_timeout(mode.label()),
                mode.label(),
            )
            .await?;
        if code != 0 {
            // Do not expose remote stdout/stderr, which can contain a process environment or auth material.
            let diagnostic = serde_json::from_str::<Value>(stderr.trim())
                .ok()
                .and_then(|value| value["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    format!("Verified remote device command exited with code {code}")
                });
            return Err(remote_error(&self.config, mode.label(), diagnostic));
        }
        let value: Value = serde_json::from_slice(&stdout).map_err(|_| {
            remote_error(
                &self.config,
                "reading host endpoints",
                "Invalid remote device response",
            )
        })?;
        if value["protocol"] != 1 || value["owner"] != self.owner {
            return Err(remote_error(
                &self.config,
                "reading host endpoints",
                "Remote device owner metadata did not match",
            ));
        }
        Ok(value)
    }
    pub async fn start(&self) -> Result<RemoteHub, Value> {
        let value = self.request(RemoteMode::Start, None).await?;
        let port = value["hubPort"]
            .as_u64()
            .filter(|port| (1..=65535).contains(port))
            .ok_or_else(|| {
                remote_error(
                    &self.config,
                    "reading host endpoints",
                    "Invalid remote hub port",
                )
            })? as u16;
        let receipt = value["receipt"]
            .as_str()
            .filter(|receipt| {
                receipt.len() == 48
                    && receipt
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or_else(|| {
                remote_error(
                    &self.config,
                    "reading host endpoints",
                    "Missing remote owner receipt",
                )
            })?;
        let node_path = value["nodePath"]
            .as_str()
            .filter(|path| Path::new(path).is_absolute())
            .ok_or_else(|| {
                remote_error(
                    &self.config,
                    "reading host endpoints",
                    "Remote Node path is not absolute",
                )
            })?;
        if value["verifiedInstalled"] != true || !value["platforms"].is_array() {
            return Err(remote_error(
                &self.config,
                "reading host endpoints",
                "Remote verified installation is unavailable",
            ));
        }
        Ok(RemoteHub {
            node_path: node_path.into(),
            port,
            receipt: receipt.into(),
            platforms: value["platforms"].clone(),
            helpers: value["helpers"].clone(),
        })
    }
    pub async fn stop(&self, receipt: &str) -> Result<(), Value> {
        let response = self.request(RemoteMode::Stop, Some(receipt)).await?;
        if response["stopped"] == true {
            Ok(())
        } else {
            Err(remote_error(
                &self.config,
                "stop",
                "Remote process stop was not confirmed",
            ))
        }
    }
    pub async fn run(
        &self,
        command: &str,
        args: &[String],
    ) -> crate::device_native_hub::CommandResult {
        let args = match plan::run_argv(&self.config, &self.owner, command, args) {
            Ok(args) => args,
            Err(error) => {
                return crate::device_native_hub::CommandResult {
                    code: 127,
                    stdout: String::new(),
                    stderr: error.to_string(),
                }
            }
        };
        match self
            .output(
                args,
                None,
                Duration::from_secs(60),
                "running device command",
            )
            .await
        {
            Ok((stdout, stderr, code)) => {
                let stdout = String::from_utf8_lossy(&stdout).into_owned();
                if code == 0 {
                    crate::device_native_hub::CommandResult {
                        code,
                        stdout,
                        stderr,
                    }
                } else {
                    plan::failed_command(&self.config, code, &stdout, &stderr)
                }
            }
            Err(_) => crate::device_native_hub::CommandResult {
                code: 127,
                stdout: String::new(),
                stderr: "SSH device command failed or timed out".into(),
            },
        }
    }
    pub async fn forward(
        &self,
        remote: &RemoteHub,
        options: ForwardOptions,
    ) -> Result<ForwardedHub, Value> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| remote_error(&self.config, "reserving hub port", error))?;
        let port = listener
            .local_addr()
            .map_err(|error| remote_error(&self.config, "reserving hub port", error))?
            .port();
        let origin = format!("http://127.0.0.1:{port}");
        let args = plan::forward_argv(&self.config, port, remote.port, None);
        drop(listener);
        let mut child = self
            .command(&args)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|error| remote_error(&self.config, "forwarding ports", error))?;
        let group = Group(child.id().expect("spawned SSH forward"));
        let stdin = child.stdin.take().expect("forward lifetime stdin");
        let stderr = child.stderr.take().expect("piped SSH stderr");
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| remote_error(&self.config, "forwarding ports", error))?;
        let (stop, mut stopped) = oneshot::channel();
        let (exited, rx) = watch::channel(false);
        let stop = Arc::new(Mutex::new(Some(stop)));
        let handle = Arc::new(ForwardHandle {
            stop: stop.clone(),
            exited: rx,
        });
        let probe_origin = origin.clone();
        let monitor_http = http.clone();
        tokio::spawn(async move {
            let _stdin = stdin;
            let drain = tokio::spawn(async move {
                let mut stderr = stderr;
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            });
            let unhealthy = async move {
                let mut misses = 0;
                loop {
                    tokio::time::sleep(options.probe_interval).await;
                    let alive = tokio::time::timeout(
                        options.probe_timeout,
                        monitor_http.get(format!("{probe_origin}/readyz")).send(),
                    )
                    .await
                    .is_ok_and(|result| result.is_ok_and(|response| response.status() == 200));
                    let (next, done) = plan::probe_misses(misses, alive, true);
                    misses = next;
                    if done {
                        return;
                    }
                }
            };
            tokio::select! {_=child.wait()=>{},_=&mut stopped=>{group.terminate();let _=tokio::time::timeout(Duration::from_secs(5),child.wait()).await;},_=unhealthy=>{group.terminate();let _=tokio::time::timeout(Duration::from_secs(5),child.wait()).await;}}
            drop(group);
            let _ = child.start_kill();
            let _ = child.wait().await;
            drop(_stdin);
            drain.abort();
            exited.send_replace(true);
        });
        let deadline = tokio::time::Instant::now() + options.ready_timeout;
        loop {
            if !handle.alive() {
                return Err(remote_error(
                    &self.config,
                    "waiting for SSH forward",
                    "Owned SSH connection exited before readiness",
                ));
            }
            let probe =
                tokio::time::timeout_at(deadline, http.get(format!("{origin}/readyz")).send())
                    .await;
            if matches!(probe,Ok(Ok(response)) if response.status()==200) {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(remote_error(
                    &self.config,
                    "waiting for SSH forward",
                    "Forwarded endpoint did not answer",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(ForwardedHub {
            origin,
            process: handle,
            lease: ForwardLease { stop },
        })
    }
}
#[derive(Clone, Copy)]
pub struct ForwardOptions {
    pub ready_timeout: Duration,
    pub probe_interval: Duration,
    pub probe_timeout: Duration,
}
impl Default for ForwardOptions {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(15),
            probe_interval: Duration::from_secs(10),
            probe_timeout: Duration::from_secs(5),
        }
    }
}
pub struct ForwardedHub {
    pub origin: String,
    pub process: Arc<dyn HubProcess>,
    pub(crate) lease: ForwardLease,
}
pub(crate) struct ForwardLease {
    stop: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}
impl Drop for ForwardLease {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
    }
}
struct ForwardHandle {
    stop: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    exited: watch::Receiver<bool>,
}
impl Drop for ForwardHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
    }
}
impl HubProcess for ForwardHandle {
    fn alive(&self) -> bool {
        !*self.exited.borrow()
    }
    fn wait(&self) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let mut exited = self.exited.clone();
            while !*exited.borrow_and_update() {
                if exited.changed().await.is_err() {
                    return;
                }
            }
        })
    }
    fn stop(&self) -> HostFuture<'_, ()> {
        Box::pin(async move {
            if let Some(stop) = self.stop.lock().unwrap().take() {
                let _ = stop.send(());
            }
            self.wait().await;
        })
    }
}
