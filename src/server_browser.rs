//! Lazy system Chromium, controlled over an inherited CDP pipe (no debugging listener).
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, Mutex},
};

const LIFETIME: Duration = Duration::from_secs(1800);
type Request = (String, Value, oneshot::Sender<Result<Value, String>>);
#[derive(Default)]
pub struct Browsers {
    tabs: Mutex<BTreeMap<(String, String), Tab>>,
}
struct Tab {
    profile: String,
    tx: mpsc::Sender<Request>,
    done: tokio::sync::watch::Receiver<bool>,
    cancel: std::sync::Arc<tokio::sync::Notify>,
}
pub fn executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("O3_BROWSER_BINARY") {
        return crate::executable::resolve(Path::new(&path));
    }
    [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ]
    .iter()
    .find_map(|n| crate::executable::resolve(Path::new(n)))
}
fn directory(root: &Path, profile: &str) -> Result<PathBuf, String> {
    let base = root.join("browser-profiles");
    if base
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err("Browser profile storage is unsafe".into());
    }
    std::fs::create_dir_all(&base).map_err(|_| "Cannot create private browser profiles")?;
    let path = base.join(format!("{:x}", Sha256::digest(profile.as_bytes())));
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err("Browser profile storage is unsafe".into());
    }
    Ok(path)
}
fn error(message: String) -> Value {
    json!({"_tag":"ServerBrowserLaunchError","message":message})
}
async fn request(tx: &mpsc::Sender<Request>, method: &str, params: Value) -> Result<Value, String> {
    let (reply, rx) = oneshot::channel();
    tokio::time::timeout(Duration::from_secs(6), async {
        tx.send((method.into(), params, reply))
            .await
            .map_err(|_| "Browser session ended")?;
        rx.await.map_err(|_| "Browser session ended")?
    })
    .await
    .map_err(|_| "Browser command timed out".to_string())?
}
struct Group(u32);
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
async fn launch(root: &Path, profile: &str) -> Result<Tab, String> {
    use std::os::fd::AsRawFd;
    let binary=executable().ok_or("Install a system Chromium browser, or set O3_BROWSER_BINARY to its executable; o3 does not download browsers")?;
    let ephemeral = profile == "incognito";
    let dir = directory(
        root,
        &if ephemeral {
            crate::store::random_id().map_err(|_| "Cannot create isolated context")?
        } else {
            profile.into()
        },
    )?;
    std::fs::create_dir_all(&dir).map_err(|_| "Cannot create browser profile")?;
    let (parent, child_pipe) =
        std::os::unix::net::UnixStream::pair().map_err(|_| "Cannot create CDP pipe")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "Cannot configure CDP pipe")?;
    // Duplicate above stdio to avoid an fd 3/4 collision during pre_exec.
    let fd = unsafe { libc::fcntl(child_pipe.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if fd < 0 {
        return Err("Cannot allocate CDP pipe".into());
    }
    let mut command = tokio::process::Command::new(binary);
    command
        .args([
            "--headless=new",
            "--remote-debugging-pipe",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
        ])
        .arg(format!("--user-data-dir={}", dir.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 || libc::dup2(fd, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::close(fd);
            Ok(())
        });
    }
    let spawned = command.spawn();
    unsafe {
        libc::close(fd);
    }
    let mut child = spawned.map_err(|_| {
        "Cannot start system Chromium; check its executable and sandbox dependencies"
    })?;
    drop(child_pipe);
    let group = Group(child.id().unwrap());
    let socket = tokio::net::UnixStream::from_std(parent).map_err(|_| "Cannot open CDP pipe")?;
    let (r, mut w) = socket.into_split();
    let mut reader = BufReader::new(r);
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let stop_signal = cancel.clone();
    tokio::spawn(async move {
        let _group = group;
        let deadline = tokio::time::sleep(LIFETIME);
        tokio::pin!(deadline);
        let mut id = 0u64;
        let mut session: Option<String> = None;
        loop {
            let next = tokio::select! {r=rx.recv()=>r,_=stop_signal.notified()=>None,_=&mut deadline=>None,_=child.wait()=>None};
            let Some((method, params, reply)) = next else {
                break;
            };
            id += 1;
            let work = async {
                let mut message = json!({"id":id,"method":method,"params":params});
                if !method.starts_with("Target.") && !method.starts_with("Browser.") {
                    if let Some(session) = &session {
                        message["sessionId"] = json!(session);
                    }
                }
                let mut packet = serde_json::to_vec(&message).unwrap();
                packet.push(0);
                w.write_all(&packet).await.map_err(|_| "CDP write failed")?;
                loop {
                    let mut buf = Vec::new();
                    // Bound every inbound JSON message, including unsolicited CDP events.
                    use tokio::io::AsyncReadExt;
                    let n = (&mut reader)
                        .take(4 * 1024 * 1024)
                        .read_until(0, &mut buf)
                        .await
                        .map_err(|_| "CDP read failed")?;
                    if n == 0 || n >= 4 * 1024 * 1024 {
                        return Err("CDP output limit or connection closed".into());
                    }
                    buf.pop();
                    let v: Value =
                        serde_json::from_slice(&buf).map_err(|_| "Invalid CDP output")?;
                    if v["id"] == id {
                        if method == "Target.attachToTarget" {
                            session = v["result"]["sessionId"].as_str().map(str::to_owned);
                        }
                        return if v.get("error").is_some() {
                            Err("Browser rejected CDP command".into())
                        } else {
                            Ok(v["result"].clone())
                        };
                    }
                }
            };
            match tokio::time::timeout(Duration::from_secs(5), work).await {
                Ok(result) => {
                    let failed = result.is_err();
                    let _ = reply.send(result);
                    if failed {
                        break;
                    }
                }
                Err(_) => {
                    let _ = reply.send(Err("Browser command timed out".into()));
                    break;
                }
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(_group);
        if ephemeral {
            let _ = std::fs::remove_dir_all(&dir);
        }
        let _ = done_tx.send(true);
    });
    Ok(Tab {
        profile: profile.into(),
        tx,
        done,
        cancel,
    })
}

impl Browsers {
    pub async fn clear(&self, root: &Path, profile: &str) -> Result<(), Value> {
        if profile == "incognito" {
            return Ok(());
        }
        let mut tabs = self.tabs.lock().await;
        let keys: Vec<_> = tabs
            .iter()
            .filter(|(_, t)| t.profile == profile)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if let Some(tab) = tabs.remove(&key) {
                if !stop(tab).await {
                    return Err(error(
                        "Browser has not stopped; profile was not removed".into(),
                    ));
                }
            }
        }
        let path = directory(root, profile).map_err(error)?;
        // Processes must have acknowledged exit before removal: refuse active profiles.
        if path.exists() {
            std::fs::remove_dir_all(path).map_err(|_| {
                error("Cannot clear browser profile; retry after closing its sessions".into())
            })?;
        }
        Ok(())
    }
    pub async fn shutdown(&self) {
        let tabs = std::mem::take(&mut *self.tabs.lock().await);
        for (_, tab) in tabs {
            stop(tab).await;
        }
    }
    pub async fn close(&self, thread: &str, tab: Option<&str>) {
        let mut tabs = self.tabs.lock().await;
        let keys: Vec<_> = tabs
            .keys()
            .filter(|(t, k)| t == thread && tab.is_none_or(|tab| k == tab))
            .cloned()
            .collect();
        for key in keys {
            if let Some(tab) = tabs.remove(&key) {
                stop(tab).await;
            }
        }
    }
    pub async fn open(
        &self,
        root: &Path,
        thread: &str,
        tab: &str,
        profile: &str,
        url: Option<&str>,
    ) -> Result<(), Value> {
        let mut tabs = self.tabs.lock().await;
        if tabs.len() >= 8 {
            return Err(error(
                "Close a server browser session before opening another".into(),
            ));
        }
        if tabs.values().any(|t| t.profile == profile) {
            return Err(error(
                "Profile already in use; close its existing browser session first".into(),
            ));
        }
        let browser = launch(root, profile).await.map_err(error)?;
        // A browser-level request creates a page. The actor routes page commands to
        // the attached target using flattened sessionId.
        let target = request(
            &browser.tx,
            "Target.createTarget",
            json!({"url":url.unwrap_or("about:blank")}),
        )
        .await
        .map_err(error)?;
        request(
            &browser.tx,
            "Target.attachToTarget",
            json!({"targetId":target["targetId"],"flatten":true}),
        )
        .await
        .map_err(error)?;
        request(&browser.tx, "Page.enable", json!({}))
            .await
            .map_err(error)?;
        tabs.insert((thread.into(), tab.into()), browser);
        Ok(())
    }
    pub async fn command(
        &self,
        thread: &str,
        tab: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Value> {
        let tx = self
            .tabs
            .lock()
            .await
            .get(&(thread.into(), tab.into()))
            .map(|t| t.tx.clone())
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        request(&tx, method, params).await.map_err(error)
    }
    pub async fn screenshot(&self, thread: &str, tab: &str) -> Result<Vec<u8>, Value> {
        let value = self
            .command(
                thread,
                tab,
                "Page.captureScreenshot",
                json!({"format":"jpeg","quality":70}),
            )
            .await?;
        base64::engine::general_purpose::STANDARD
            .decode(value["data"].as_str().unwrap_or(""))
            .map_err(|_| error("Invalid browser image".into()))
    }
}

async fn stop(tab: Tab) -> bool {
    let Tab {
        tx,
        mut done,
        cancel,
        ..
    } = tab;
    cancel.notify_one();
    drop(tx);
    if !*done.borrow() {
        let _ = tokio::time::timeout(Duration::from_secs(7), done.changed()).await;
    }
    let stopped = *done.borrow();
    stopped
}

pub async fn stream<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    state: std::sync::Arc<crate::store::State>,
    session: Value,
    thread: String,
    tab: String,
    interactive: bool,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let operate = interactive
        && session["scopes"]
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s == "preview:operate"));
    let hello = json!({"type":"control","canOperate":operate,"controller":if operate{"you"}else{"unclaimed"},"generation":1,"dialog":null});
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let deadline = tokio::time::sleep(LIFETIME);
    tokio::pin!(deadline);
    let mut waiting: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            _=&mut deadline=>break,
            _=interval.tick()=>{
                let valid=session["sid"].as_str().is_some_and(|sid|state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?)",rusqlite::params![sid,crate::store::now_ms()],|r|r.get::<_,bool>(0)).unwrap_or(false));
                if !valid||waiting.is_some_and(|at|at.elapsed()>Duration::from_secs(10)){break;}
                if waiting.is_some(){continue;}
                let Ok(frame)=state.streams.p2.preview.browser.screenshot(&thread,&tab).await else{break;};
                if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Binary(frame.into()))).await,Ok(Ok(()))){break;}
                waiting=Some(std::time::Instant::now());
            }
            incoming=socket.next()=>{
                let Some(Ok(Message::Text(raw)))=incoming else{break;};
                if raw.len()>16384 {break;}
                let Ok(v)=serde_json::from_str::<Value>(&raw) else{continue;};
                if v["type"]=="ack" {waiting=None;continue;}
                if !operate {continue;}
                let (method,params)=match v["type"].as_str(){
                    Some("text")=>("Input.insertText",json!({"text":v["text"].as_str().unwrap_or("").chars().take(10000).collect::<String>()})),
                    Some("navigate")=>{let Ok(url)=crate::preview::normalize_url(v["url"].as_str().unwrap_or(""))else{continue;};("Page.navigate",json!({"url":url}))},
                    Some("reload")=>("Page.reload",json!({"ignoreCache":v["ignoreCache"]==true})),
                    Some("resize")=>("Emulation.setDeviceMetricsOverride",json!({"width":v["width"].as_u64().unwrap_or(1280).clamp(240,3840),"height":v["height"].as_u64().unwrap_or(800).clamp(240,2160),"deviceScaleFactor":1,"mobile":false})),
                    Some("key")=>("Input.dispatchKeyEvent",json!({"type":if v["action"]=="up"{"keyUp"}else{"keyDown"},"key":v["key"],"code":v["code"],"text":v["text"].as_str().unwrap_or("")})),
                    Some("mouse"|"wheel")=>("Input.dispatchMouseEvent",json!({"type":if v["type"]=="wheel"{"mouseWheel"}else if v["action"]=="down"{"mousePressed"}else if v["action"]=="up"{"mouseReleased"}else{"mouseMoved"},"x":v["x"].as_f64().unwrap_or(0.0),"y":v["y"].as_f64().unwrap_or(0.0),"button":v["button"].as_str().unwrap_or("none"),"clickCount":v["clickCount"].as_u64().unwrap_or(1).min(3),"deltaX":v["deltaX"].as_f64().unwrap_or(0.0),"deltaY":v["deltaY"].as_f64().unwrap_or(0.0)})),
                    _=>continue,
                };
                if state.streams.p2.preview.browser.command(&thread,&tab,method,params).await.is_err(){break;}
            }
        }
    }
    let _ = socket.close(None).await;
}
