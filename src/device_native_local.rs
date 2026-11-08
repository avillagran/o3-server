//! A real local hub process driver. Tests inject a driver and never start host tools.
use crate::{
    device_native_client::{HostTools, HubClient},
    device_native_owner::{host_error, HostFuture, HubDriver, HubProcess, SpawnedHub},
    device_native_tools::{InstalledTool, Tool},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{oneshot, watch};

pub struct LocalDriver {
    node: PathBuf,
    tool_root: PathBuf,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
    tools: Arc<dyn HostTools>,
    platform: String,
}
impl LocalDriver {
    /// The tool root is reverified against its complete pinned manifest on every launch.
    pub fn new(
        node: PathBuf,
        tool_root: PathBuf,
        cwd: PathBuf,
        environment: BTreeMap<String, String>,
        tools: Arc<dyn HostTools>,
        platform: String,
    ) -> Result<Self, String> {
        if !node.is_absolute() || !cwd.is_absolute() || !tool_root.is_absolute() {
            return Err("Native device process paths must be absolute".into());
        }
        InstalledTool::inspect(&tool_root, Tool::Hub).map_err(|error| error.to_string())?;
        Ok(Self {
            node,
            tool_root,
            cwd,
            environment,
            tools,
            platform,
        })
    }
}
struct Group(u32);
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
struct ChildHandle {
    stop: Mutex<Option<oneshot::Sender<()>>>,
    exited: watch::Receiver<bool>,
}
impl Drop for ChildHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.get_mut().unwrap().take() {
            let _ = stop.send(());
        }
    }
}
impl HubProcess for ChildHandle {
    fn alive(&self) -> bool {
        !*self.exited.borrow()
    }
    fn wait(&self) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let mut exited = self.exited.clone();
            while !*exited.borrow_and_update() {
                if exited.changed().await.is_err() {
                    break;
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
/// No shell is involved. The child and its descendants belong to a new process group.
pub fn hub_argv(entry: &Path, port: u16, platform: &str) -> Vec<String> {
    let mut args = vec![
        entry.to_string_lossy().into_owned(),
        "--port".into(),
        port.to_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--hide-sidebar".into(),
        "--hide-boot-device".into(),
    ];
    if platform != "darwin" {
        args.extend(["--platform".into(), "android".into()]);
    }
    args
}
impl HubDriver for LocalDriver {
    fn spawn(&self) -> HostFuture<'_, Result<SpawnedHub, Value>> {
        Box::pin(async move {
            // File verification is blocking and belongs off the async runtime's request workers.
            let root = self.tool_root.clone();
            let installed =
                tokio::task::spawn_blocking(move || InstalledTool::inspect(&root, Tool::Hub))
                    .await
                    .map_err(|error| host_error("local", "checking device support", error))?
                    .map_err(|error| host_error("local", "checking device support", error))?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| {
                    host_error("local", "reserving a port for the device hub", error)
                })?;
            let port = listener
                .local_addr()
                .map_err(|error| host_error("local", "reserving a port for the device hub", error))?
                .port();
            let origin = format!("http://127.0.0.1:{port}");
            let client = Arc::new(
                HubClient::new("local".into(), &origin, self.tools.clone())
                    .map_err(|error| host_error("local", "starting the device hub", error))?,
            );
            let cameras = if self
                .environment
                .get("ANDROID_HOME")
                .is_some_and(|root| !root.is_empty())
            {
                let front = tokio::time::timeout(
                    Duration::from_secs(5),
                    self.tools.run("emulator", &["-help-camera-front".into()]),
                )
                .await
                .ok();
                let back = tokio::time::timeout(
                    Duration::from_secs(5),
                    self.tools.run("emulator", &["-help-camera-back".into()]),
                )
                .await
                .ok();
                [front, back]
            } else {
                [None, None]
            };
            let uid = unsafe { libc::getuid() };
            let runtime_directory =
                std::fs::metadata(format!("/run/user/{uid}")).is_ok_and(|metadata| {
                    use std::os::unix::fs::MetadataExt;
                    metadata.is_dir() && metadata.uid() == uid
                });
            let environment = crate::device_native_platform::hub_environment(
                &self.environment,
                &self.platform,
                Some(uid),
                runtime_directory,
                [cameras[0].as_ref(), cameras[1].as_ref()],
            );
            let mut command = tokio::process::Command::new(&self.node);
            command
                .args(hub_argv(&installed.entry, port, &self.platform))
                .current_dir(&self.cwd)
                .envs(&environment)
                .env("FORCE_COLOR", "0")
                .env("NO_COLOR", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .process_group(0);
            // The native hub accepts a port rather than an inherited listener.
            drop(listener);
            let mut child = command
                .spawn()
                .map_err(|error| host_error("local", "starting the device hub", error))?;
            let group = Group(child.id().expect("spawned child has a PID"));
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            let (stop, mut stopped) = oneshot::channel();
            let (exited, rx) = watch::channel(false);
            let handle = Arc::new(ChildHandle {
                stop: Mutex::new(Some(stop)),
                exited: rx,
            });
            tokio::spawn(async move {
                // Drain without retaining unbounded output or leaking environment values into logs.
                let drain = async move {
                    let mut stdout = stdout;
                    let mut stderr = stderr;
                    let mut stdout_sink = tokio::io::sink();
                    let mut stderr_sink = tokio::io::sink();
                    let _ = tokio::join!(
                        tokio::io::copy(&mut stdout, &mut stdout_sink),
                        tokio::io::copy(&mut stderr, &mut stderr_sink)
                    );
                };
                let drains = tokio::spawn(drain);
                tokio::select! {_=child.wait()=>{},_=&mut stopped=>{drop(group);let _=child.start_kill();let _=child.wait().await;exited.send_replace(true);drains.abort();return;}}
                drop(group);
                exited.send_replace(true);
                drains.abort();
            });
            let http = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| {
                    host_error("local", "waiting for the device hub to answer", error)
                })?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                if !handle.alive() {
                    return Err(host_error(
                        "local",
                        "waiting for the device hub to answer",
                        "Device hub exited before readiness",
                    ));
                }
                let probe =
                    tokio::time::timeout_at(deadline, http.get(format!("{origin}/readyz")).send())
                        .await;
                match probe {
                    Ok(Ok(response)) if response.status().is_success() => break,
                    Err(_) => {
                        return Err(host_error(
                            "local",
                            "waiting for the device hub to answer",
                            "Device hub readiness timed out",
                        ))
                    }
                    _ => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(host_error(
                        "local",
                        "waiting for the device hub to answer",
                        "Device hub readiness timed out",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok(SpawnedHub {
                process: handle,
                client,
                origin,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_native_hub::{CommandResult, Platform};
    use crate::device_native_owner::LocalOwner;
    struct Tools;
    impl HostTools for Tools {
        fn platform_available(&self, _: Platform) -> HostFuture<'_, bool> {
            Box::pin(async { false })
        }
        fn run<'a>(&'a self, _: &'a str, _: &'a [String]) -> HostFuture<'a, CommandResult> {
            Box::pin(async { panic!("Synthetic hub must never invoke a host tool") })
        }
    }
    fn fixture(mode: &str) -> (LocalDriver, PathBuf) {
        crate::test_sandbox::assert_isolated();
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "synthetic-hub-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let root = base.join("installation");
        let package = root.join("node_modules/expo-device-hub");
        std::fs::create_dir_all(package.join("dist/server")).unwrap();
        std::fs::write(
            package.join("package.json"),
            br#"{"name":"expo-device-hub","version":"0.19.1"}"#,
        )
        .unwrap();
        // This is a synthetic Python HTTP service, not an SDK, emulator, package, or Node tool.
        std::fs::write(package.join("dist/server/cli.mjs"),r#"import http.server, os, sys
with open(os.environ['FIXTURE_PID_PATH'], 'w') as output: output.write(str(os.getpid()))
if os.environ['FIXTURE_MODE'] == 'exit': sys.exit(7)
port=int(sys.argv[sys.argv.index('--port')+1])
class Handler(http.server.BaseHTTPRequestHandler):
 protocol_version="HTTP/1.1"
 booted=False
 def record(self):
  import json
  with open(os.environ['FIXTURE_PID_PATH']+'.requests','a') as output: output.write(json.dumps({'method':self.command,'path':self.path,'headers':dict(self.headers)})+'\n')
 def do_GET(self):
  self.record()
  if self.path.split('?')[0]=='/vendor/serve-sim/helper/ws':
   import base64, hashlib
   accept=base64.b64encode(hashlib.sha1((self.headers['Sec-WebSocket-Key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
   self.send_response(101); self.send_header('Upgrade','websocket'); self.send_header('Connection','Upgrade'); self.send_header('Sec-WebSocket-Accept',accept); self.end_headers()
   while True:
    head=self.rfile.read(2)
    if len(head)!=2: break
    length=head[1]&127
    if length==126: length=int.from_bytes(self.rfile.read(2),'big')
    if length==127: length=int.from_bytes(self.rfile.read(8),'big')
    mask=self.rfile.read(4) if head[1]&128 else b''
    data=self.rfile.read(length)
    if mask: data=bytes(value^mask[index%4] for index,value in enumerate(data))
    self.wfile.write(bytes([head[0],len(data)])+data); self.wfile.flush()
    if head[0]&15==8: break
   return
  if os.environ['FIXTURE_MODE'] == 'unready':
   self.send_response(503); self.end_headers(); return
  body=b'{"simulators":[],"emulators":[]}' if self.path == '/api/devices' else b'ready'
  if self.path == '/api/devices' and os.environ['FIXTURE_MODE']=='native':
   import json
   body=json.dumps({'simulators':[{'id':'sim-1','name':'Private Simulator','platform':'ios','version':'26','booted':Handler.booted,'physical':False}],'emulators':[]}).encode()
  self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
 def do_POST(self):
  self.record()
  body=self.rfile.read(int(self.headers.get('Content-Length',0)))
  if self.path=='/api/devices/boot': Handler.booted=True
  if self.path=='/vendor/serve-sim/grid/api/shutdown': Handler.booted=False
  body=b'{"ok":true,"id":"sim-1"}'
  if self.path.startswith('/vendor/serve-sim/api/screenshot'): body=b'\x89PNG\r\n\x1a\nprivate-fixture'
  self.send_response(200); self.send_header('Content-Type','image/png' if self.path.startswith('/vendor/serve-sim/api/screenshot') else 'application/json'); self.send_header('Content-Length',str(len(body))); self.send_header('Set-Cookie','upstream=secret'); self.end_headers(); self.wfile.write(body)
 def log_message(self,*args): pass
http.server.ThreadingHTTPServer(('127.0.0.1',port),Handler).serve_forever()
"#).unwrap();
        InstalledTool::seal_staging(&root, Tool::Hub).unwrap();
        let pid = base.join("fixture.pid");
        let env = BTreeMap::from([
            (
                "FIXTURE_PID_PATH".into(),
                pid.to_string_lossy().into_owned(),
            ),
            ("FIXTURE_MODE".into(), mode.into()),
        ]);
        (
            LocalDriver::new(
                "/usr/bin/python3".into(),
                root,
                base,
                env,
                Arc::new(Tools),
                "linux".into(),
            )
            .unwrap(),
            pid,
        )
    }
    fn process_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    async fn read_pid(path: &Path) -> u32 {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(path) {
                    if let Ok(pid) = text.parse() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn real_owned_child_readiness_http_and_stop_in_private_container() {
        let (driver, pid) = fixture("ready");
        let owner = LocalOwner::new(Arc::new(driver));
        let ready = owner.ensure_ready().await.unwrap();
        let pid = read_pid(&pid).await;
        assert!(process_alive(pid));
        assert!(ready.hub.client.list().await.unwrap().devices.is_empty());
        owner.stop().await;
        assert!(!process_alive(pid));
        assert!(owner.current().is_none());
    }
    #[tokio::test]
    async fn failed_or_cancelled_readiness_closes_the_owned_child_scope() {
        let (driver, pid) = fixture("exit");
        let owner = LocalOwner::new(Arc::new(driver));
        assert_eq!(
            owner.ensure_ready().await.err().unwrap()["_tag"],
            "DeviceHostError"
        );
        let pid = read_pid(&pid).await;
        assert!(!process_alive(pid));
        assert!(owner.current().is_none());
        let (driver, path) = fixture("unready");
        let task = tokio::spawn(async move { driver.spawn().await });
        let pid = read_pid(&path).await;
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while process_alive(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn actual_rpc_inventory_boot_session_and_authenticated_screenshot_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (driver, pid_path) = fixture("native");
        let owner = LocalOwner::new(Arc::new(driver));
        let directory = pid_path.parent().unwrap().join("profile");
        let state = Arc::new(crate::store::State::open(&directory).unwrap());
        crate::settings::update(&state,&serde_json::json!({"patch":{"enableDeviceSupport":true,"enableAgentDeviceAccess":true}})).unwrap();
        let summary = serde_json::json!({"id":"local","kind":"local","label":"This machine","platforms":[{"platform":"ios","available":true},{"platform":"android","available":false}],"hubInstalled":true,"agentDeviceInstalled":false});
        let runtime = Arc::new(crate::device_native_runtime::NativeRuntime::new(
            owner,
            summary,
            state.streams.changed.clone(),
        ));
        state
            .streams
            .p2
            .device
            .register_native_test(runtime.clone())
            .await;
        let listed = crate::device::call(&state, "device.list", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(listed["devices"][0]["booted"], false);
        let opened=crate::device::call(&state,"device.open",&serde_json::json!({"hostId":"local","deviceId":"sim-1","platform":"ios","threadId":"private-thread"})).await.unwrap();
        assert_eq!(opened["deviceId"], "sim-1");
        assert!(runtime.snapshot()["bootingDevices"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(runtime.snapshot()["sessions"].as_array().unwrap().len(), 1);
        // Screenshot reads and the input socket require independent native scopes.
        let token = state
            .issue_session(
                "private-fixture",
                &["orchestration:read".into(), "orchestration:operate".into()],
                60_000,
                "Private fixture",
            )
            .unwrap();
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let serving = tokio::spawn(crate::http::serve_io(server, state.clone()));
        let request=format!("POST /api/device-hub/vendor/serve-sim/api/screenshot?device=sim-1&hostId=local&wsTicket=private-ticket HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nCookie: private-cookie\r\nDPoP: private-proof\r\nOrigin: https://client.test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        client.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        serving.await.unwrap().unwrap();
        let head = String::from_utf8_lossy(
            &bytes[..bytes
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .unwrap()],
        )
        .to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(head.contains("content-type: image/png"));
        assert!(!head.contains("set-cookie"));
        assert!(bytes.windows(15).any(|part| part == b"private-fixture"));
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let (socket_client, socket_server) = tokio::io::duplex(64 * 1024);
        let socket_serving = tokio::spawn(crate::http::serve_io(socket_server, state.clone()));
        let mut socket_request="ws://localhost/api/device-hub/vendor/serve-sim/helper/ws?hostId=local&wsTicket=private-ticket".into_client_request().unwrap();
        socket_request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::client_async(socket_request, socket_client)
            .await
            .unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "private-input".into(),
            ))
            .await
            .unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echoed.to_text().unwrap(), "private-input");
        socket.close(None).await.unwrap();
        drop(socket);
        let _ = socket_serving.await;
        let (mut denied, server) = tokio::io::duplex(4096);
        let denied_serving = tokio::spawn(crate::http::serve_io(server, state.clone()));
        denied.write_all(format!("POST /api/device-hub/vendor/serve-sim/exec HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        denied.read_to_end(&mut reply).await.unwrap();
        denied_serving.await.unwrap().unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 404"));
        let records = std::fs::read_to_string(format!("{}.requests", pid_path.display())).unwrap();
        let requests = records
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let screenshot = requests
            .iter()
            .find(|request| request["path"].as_str().unwrap().contains("screenshot"))
            .unwrap();
        assert_eq!(
            screenshot["path"],
            "/vendor/serve-sim/api/screenshot?device=sim-1"
        );
        for (key, _) in screenshot["headers"].as_object().unwrap() {
            assert!(
                !["authorization", "cookie", "dpop"].contains(&key.to_ascii_lowercase().as_str())
            );
        }
        crate::device::call(&state,"device.close",&serde_json::json!({"hostId":"local","deviceId":"sim-1","threadId":"private-thread","shutdown":true})).await.unwrap();
        assert!(runtime.snapshot()["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
        crate::device::call(&state,"device.open",&serde_json::json!({"hostId":"local","deviceId":"sim-1","platform":"ios","threadId":"disable-with-open-session"})).await.unwrap();
        crate::device::call(
            &state,
            "device.configure",
            &serde_json::json!({"enabled":false}),
        )
        .await
        .unwrap();
        assert!(runtime.owner.current().is_none());
        assert!(runtime.snapshot()["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(runtime.snapshot()["devices"].as_array().unwrap().is_empty());
        assert!(!process_alive(read_pid(&pid_path).await));
    }
}
