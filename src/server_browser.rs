//! Owned Chromium pipe sessions and frontend desktop CDP connections.
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};

mod desktop;
mod events;
mod pending;
mod pipe;
mod write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Local,
    Desktop,
}

const LIFETIME: Duration = Duration::from_secs(1800);
struct Request {
    method: String,
    params: Value,
    session: Option<String>,
    reply: oneshot::Sender<Result<Value, String>>,
    expires: tokio::time::Instant,
}

#[derive(Default)]
pub struct Browsers {
    #[cfg(test)]
    pub(crate) fixture_binary: std::sync::Mutex<Option<PathBuf>>,
    removal: ProfileRemoval,
    tabs: Mutex<BTreeMap<(String, String), Tab>>,
    viewers: std::sync::Mutex<std::collections::BTreeSet<(String, String)>>,
}
struct ProfileRemoval(std::sync::Mutex<std::sync::Arc<dyn crate::cleanup::Removal>>);
impl Default for ProfileRemoval {
    fn default() -> Self {
        Self(std::sync::Mutex::new(std::sync::Arc::new(
            crate::cleanup::RealRemoval,
        )))
    }
}
#[derive(Clone)]
pub(crate) struct AbortWait {
    pub token: u64,
    pub expires: tokio::time::Instant,
}
#[derive(Clone, Default)]
pub(crate) struct Navigation {
    pub revision: u64,
    pub generation: u64,
    pub loading: bool,
    pub report: Option<Value>,
    pub aborted: Option<AbortWait>,
}
#[derive(Clone)]
struct Tab {
    origin: Origin,
    desktop_channel: Option<crate::desktop_browser_channel::Channel>,
    navigation: Option<tokio::sync::watch::Receiver<Navigation>>,
    events: Option<events::Shared>,
    process_done: tokio::sync::watch::Receiver<bool>,
    session: Option<String>,
    target: Option<String>,
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
    request_in(tx, method, params, None).await
}
async fn request_in(
    tx: &mpsc::Sender<Request>,
    method: &str,
    params: Value,
    session: Option<String>,
) -> Result<Value, String> {
    let (reply, rx) = oneshot::channel();
    let expires = tokio::time::Instant::now() + Duration::from_secs(6);
    tokio::time::timeout_at(expires, async {
        tx.send(Request {
            method: method.into(),
            params,
            session,
            reply,
            expires,
        })
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
async fn launch(
    root: &Path,
    profile: &str,
    registry: &std::sync::Arc<crate::agent_scope::Registry>,
    binary: PathBuf,
    removal: std::sync::Arc<dyn crate::cleanup::Removal>,
) -> Result<Tab, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let ephemeral = profile == "incognito" || profile.starts_with("agent:");
    let dir = directory(
        root,
        &if ephemeral {
            crate::store::random_id().map_err(|_| "Cannot create isolated context")?
        } else {
            profile.into()
        },
    )?;
    if ephemeral {
        std::fs::create_dir(&dir)
    } else {
        std::fs::create_dir_all(&dir)
    }
    .map_err(|_| "Cannot create browser profile")?;
    use crate::cleanup::Tree;
    let ephemeral_identity = if ephemeral {
        Some(
            crate::cleanup::FileTree
                .identity(&dir)
                .map_err(|_| "Cannot identify isolated browser profile")?,
        )
    } else {
        None
    };
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot restrict browser profile permissions")?;
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
        .arg("about:blank");
    let (mut command, mut agent_scope) = registry
        .wrap(command, "preview-browser", None, false, false)
        .await?;
    let (parent, child_pipe) =
        std::os::unix::net::UnixStream::pair().map_err(|_| "Cannot create CDP pipe")?;
    parent
        .set_nonblocking(true)
        .map_err(|_| "Cannot configure CDP pipe")?;
    // Keep allocation after scope preparation and own the duplicate across all error paths.
    let raw_fd = unsafe { libc::fcntl(child_pipe.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if raw_fd < 0 {
        return Err("Cannot allocate CDP pipe".into());
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(raw_fd, 3) < 0 || libc::dup2(raw_fd, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::close(raw_fd);
            Ok(())
        });
    }
    let spawned = command.spawn();
    drop(fd);
    let mut child = spawned.map_err(|_| {
        "Cannot start system Chromium; check its executable and sandbox dependencies"
    })?;
    drop(child_pipe);
    let group = Group(child.id().unwrap());
    let socket = tokio::net::UnixStream::from_std(parent).map_err(|_| "Cannot open CDP pipe")?;
    let (input, output) = socket.into_split();
    let mut writer = write::pipe(output);
    let (mut incoming, reader_task) = pipe::start(input);
    let reader_abort = pipe::Abort(reader_task.abort_handle());
    let pages: events::Shared = Default::default();
    let actor_pages = pages.clone();
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let stop_signal = cancel.clone();
    tokio::spawn(async move {
        let _group = group;
        let _reader_abort = reader_abort;
        let deadline = tokio::time::sleep(LIFETIME);
        tokio::pin!(deadline);
        let mut calls = pending::Calls::default();
        let mut writing_done = writer.completion();
        loop {
            let expires = calls.deadline();
            tokio::select! {
                next=rx.recv(),if !calls.full()=>{
                    let Some(next)=next else{break;};
                    let Some(message)=calls.insert(next)else{continue;};
                    let mut packet=match serde_json::to_vec(&message){Ok(packet)=>packet,Err(_)=>break};packet.push(0);
                    if !writer.send(packet){break;}
                },
                _=pending::deadline(expires)=>{if calls.expire(tokio::time::Instant::now()){break;}},
                _=writing_done.changed()=>break,
                _=stop_signal.notified()=>break,
                _=&mut deadline=>break,
                _=child.wait()=>break,
                packet=incoming.recv()=>{
                    let Some(Ok(value))=packet else{break;};
                    if events::notify(&actor_pages,&value){break;}
                    calls.complete(&value,&actor_pages);
                }
            }
        }
        drop(calls);
        writer.stop().await;
        reader_task.abort();
        let _ = reader_task.await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(_group);
        if let Some(scope) = agent_scope.as_mut() {
            scope.stop().await;
        }
        if let Some(identity) = ephemeral_identity {
            if let Ok(plan) = crate::cleanup::RemovalPlan::prepare(&dir, &crate::cleanup::FileTree)
            {
                if plan.operations().last().is_some_and(|operation| {
                    operation.path == dir && operation.identity == identity
                }) {
                    let _ = removal.execute(&plan);
                }
            }
        }
        events::stopped(&actor_pages);
        let _ = done_tx.send(true);
    });
    Ok(Tab {
        origin: Origin::Local,
        desktop_channel: None,
        navigation: None,
        events: Some(pages),
        process_done: done.clone(),
        session: None,
        target: None,
        profile: profile.into(),
        tx,
        done,
        cancel,
    })
}

async fn rollback_setup(tabs: &BTreeMap<(String, String), Tab>, browser: Tab, fence: bool) {
    if let (Some(pages), Some(session)) = (&browser.events, &browser.session) {
        events::remove(pages, session);
    }
    let closed = if let Some(target) = &browser.target {
        request(
            &browser.tx,
            "Target.closeTarget",
            json!({"targetId":target}),
        )
        .await
        .is_ok()
    } else {
        true
    };
    let shared = tabs
        .values()
        .any(|other| !*other.done.borrow() && other.tx.same_channel(&browser.tx));
    if fence || !closed || !shared {
        stop(browser).await;
    }
}

struct Viewer<'a> {
    browser: &'a Browsers,
    key: (String, String),
}
impl Drop for Viewer<'_> {
    fn drop(&mut self) {
        self.browser.viewers.lock().unwrap().remove(&self.key);
    }
}
impl Browsers {
    pub(crate) fn set_profile_removal(&self, removal: std::sync::Arc<dyn crate::cleanup::Removal>) {
        *self.removal.0.lock().unwrap() = removal;
    }

    pub fn human_controls(&self, thread: &str, tab: &str) -> bool {
        self.viewers
            .lock()
            .unwrap()
            .contains(&(thread.into(), tab.into()))
    }
    fn claim_viewer(&self, thread: &str, tab: &str) -> Option<Viewer<'_>> {
        let key = (thread.into(), tab.into());
        if self.viewers.lock().unwrap().insert(key.clone()) {
            Some(Viewer { browser: self, key })
        } else {
            None
        }
    }

    pub async fn clear(&self, root: &Path, profile: &str) -> Result<(), Value> {
        if profile == "incognito" {
            return Ok(());
        }
        let mut tabs = self.tabs.lock().await;
        let keys: Vec<_> = tabs
            .iter()
            .filter(|(_, t)| t.origin == Origin::Local && t.profile == profile)
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
            let plan = crate::cleanup::RemovalPlan::prepare(&path, &crate::cleanup::FileTree)
                .map_err(|_| error("Cannot safely inspect browser profile for removal".into()))?;
            self.removal.0.lock().unwrap().execute(&plan).map_err(|_| {
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
                let was_live = !*tab.done.borrow();
                if let (Some(pages), Some(session)) = (&tab.events, &tab.session) {
                    events::remove(pages, session);
                }
                if let Some(target) = tab
                    .target
                    .as_ref()
                    .filter(|_| tab.origin == Origin::Local && was_live)
                {
                    let _ =
                        request(&tab.tx, "Target.closeTarget", json!({"targetId":target})).await;
                }
                if !tabs
                    .values()
                    .any(|other| !*other.done.borrow() && other.tx.same_channel(&tab.tx))
                {
                    stop(tab).await;
                }
            }
        }
    }
    pub(crate) async fn open_owned(
        &self,
        root: &Path,
        registry: &std::sync::Arc<crate::agent_scope::Registry>,
        thread: &str,
        tab: &str,
        profile: &str,
        url: Option<&str>,
    ) -> Result<(), Value> {
        let mut tabs = self.tabs.lock().await;
        tabs.retain(|_, t| t.origin == Origin::Desktop || !*t.done.borrow());
        if tabs.values().filter(|tab| !*tab.done.borrow()).count() >= 8 {
            return Err(error(
                "Close a server browser session before opening another".into(),
            ));
        }
        let mut browser = if let Some(existing) = tabs
            .values()
            .find(|t| t.origin == Origin::Local && t.profile == profile && !*t.done.borrow())
        {
            existing.clone()
        } else {
            #[cfg(test)]
            let binary = self
                .fixture_binary
                .lock()
                .unwrap()
                .clone()
                .or_else(executable);
            #[cfg(not(test))]
            let binary = executable();
            let binary=binary.ok_or_else(||error("Install a system Chromium browser, or set O3_BROWSER_BINARY to its executable; o3 does not download browsers".into()))?;
            let removal = self.removal.0.lock().unwrap().clone();
            launch(root, profile, registry, binary, removal)
                .await
                .map_err(error)?
        };
        // Setup owns its target before admission, including when a profile reuses a process.
        browser.target = None;
        browser.session = None;
        browser.navigation = None;
        let target = match request(
            &browser.tx,
            "Target.createTarget",
            json!({"url":"about:blank"}),
        )
        .await
        {
            Ok(target) => target,
            Err(cause) => {
                let fence = cause == "Browser command timed out";
                rollback_setup(&tabs, browser, fence).await;
                return Err(error(cause));
            }
        };
        let Some(target_id) = target["targetId"]
            .as_str()
            .filter(|value| !value.is_empty())
        else {
            rollback_setup(&tabs, browser, true).await;
            return Err(error("Browser target creation failed".into()));
        };
        browser.target = Some(target_id.to_owned());
        let initialized: Result<(), String> = async {
            let attached = request(
                &browser.tx,
                "Target.attachToTarget",
                json!({"targetId":target["targetId"],"flatten":true}),
            )
            .await?;
            browser.session = Some(
                attached["sessionId"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("Browser page attachment failed")?
                    .to_owned(),
            );
            if let (Some(pages), Some(session), Some(target)) =
                (&browser.events, &browser.session, &browser.target)
            {
                let (done, navigation) = events::register(pages, session, target);
                browser.done = done;
                browser.navigation = Some(navigation);
            }
            request_in(
                &browser.tx,
                "Page.enable",
                json!({}),
                browser.session.clone(),
            )
            .await?;
            request_in(
                &browser.tx,
                "Page.getFrameTree",
                json!({}),
                browser.session.clone(),
            )
            .await?;
            request_in(
                &browser.tx,
                "Network.enable",
                json!({}),
                browser.session.clone(),
            )
            .await?;
            request_in(
                &browser.tx,
                "Network.setBlockedURLs",
                json!({"urls":["file://*","ftp://*"]}),
                browser.session.clone(),
            )
            .await?;
            request(
                &browser.tx,
                "Browser.setDownloadBehavior",
                json!({"behavior":"deny"}),
            )
            .await?;
            request(
                &browser.tx,
                "Target.setDiscoverTargets",
                json!({"discover":true}),
            )
            .await?;
            if let Some(url) = url {
                request_in(
                    &browser.tx,
                    "Page.navigate",
                    json!({"url":url}),
                    browser.session.clone(),
                )
                .await?;
            }
            Ok(())
        }
        .await;
        if let Err(cause) = initialized {
            rollback_setup(&tabs, browser, false).await;
            return Err(error(cause));
        }
        tabs.insert((thread.into(), tab.into()), browser);
        Ok(())
    }
    /// Adopt a frontend-owned page without navigating or creating another target.
    pub async fn open_desktop(
        &self,
        channel: &crate::desktop_browser_channel::Channel,
        thread: &str,
        tab: &str,
        profile: &str,
    ) -> Result<(), Value> {
        let mut tabs = self.tabs.lock().await;
        let key = (thread.into(), tab.into());
        if let Some(existing) = tabs.get(&key) {
            if existing.origin != Origin::Desktop {
                return Err(error("Browser tab is already locally owned".into()));
            }
            if !*existing.done.borrow() {
                return Ok(());
            }
            if !stop(existing.clone()).await {
                return Err(error("Desktop browser connection is still closing".into()));
            }
        }
        if tabs.values().filter(|tab| !*tab.done.borrow()).count() >= 8 {
            return Err(error(
                "Close a server browser session before opening another".into(),
            ));
        }
        let browser = desktop::launch(channel, thread, tab, profile)
            .await
            .map_err(error)?;
        tabs.insert(key, browser);
        Ok(())
    }
    /// A disconnected desktop page remains frontend-owned and may be reattached.
    pub async fn connection(&self, thread: &str, tab: &str) -> Option<(Origin, bool)> {
        self.tabs
            .lock()
            .await
            .get(&(thread.into(), tab.into()))
            .map(|tab| (tab.origin, !*tab.done.borrow()))
    }
    pub(crate) async fn navigation(
        &self,
        thread: &str,
        tab: &str,
    ) -> Option<tokio::sync::watch::Receiver<Navigation>> {
        self.tabs
            .lock()
            .await
            .get(&(thread.into(), tab.into()))
            .and_then(|tab| tab.navigation.clone())
    }
    pub(crate) async fn network_entries(
        &self,
        thread: &str,
        tab: &str,
    ) -> Result<Vec<Value>, Value> {
        let tabs = self.tabs.lock().await;
        let tab = tabs
            .get(&(thread.into(), tab.into()))
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        let entries = tab
            .events
            .as_ref()
            .zip(tab.session.as_deref())
            .and_then(|(pages, session)| events::entries(pages, session))
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        Ok(entries)
    }
    pub(crate) async fn note_navigation_url(
        &self,
        thread: &str,
        tab: &str,
        watch: &tokio::sync::watch::Receiver<Navigation>,
        url: &str,
    ) {
        let tabs = self.tabs.lock().await;
        if let Some(tab) = tabs.get(&(thread.into(), tab.into())).filter(|tab| {
            tab.navigation
                .as_ref()
                .is_some_and(|current| current.same_channel(watch))
        }) {
            if let (Some(pages), Some(session)) = (&tab.events, &tab.session) {
                events::note_url(pages, session, url);
            }
        }
    }
    pub(crate) async fn expire_navigation(
        &self,
        thread: &str,
        tab: &str,
        watch: &tokio::sync::watch::Receiver<Navigation>,
        token: u64,
    ) {
        let tabs = self.tabs.lock().await;
        if let Some(tab) = tabs.get(&(thread.into(), tab.into())).filter(|tab| {
            tab.navigation
                .as_ref()
                .is_some_and(|current| current.same_channel(watch))
        }) {
            if let (Some(pages), Some(session)) = (&tab.events, &tab.session) {
                events::expire(pages, session, token);
            }
        }
    }
    pub async fn completion(
        &self,
        thread: &str,
        tab: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.tabs
            .lock()
            .await
            .get(&(thread.into(), tab.into()))
            .map(|t| t.done.clone())
    }
    pub async fn command(
        &self,
        thread: &str,
        tab: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Value> {
        let browser = self
            .tabs
            .lock()
            .await
            .get(&(thread.into(), tab.into()))
            .cloned()
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        if method == "Input.dispatchMouseEvent" {
            if let (Some(channel), Some(x), Some(y)) = (
                &browser.desktop_channel,
                params["x"].as_f64(),
                params["y"].as_f64(),
            ) {
                let phase = match params["type"].as_str() {
                    Some("mouseMoved") => Some("move"),
                    Some("mousePressed") => Some("click"),
                    _ => None,
                };
                if let Some(phase) = phase {
                    channel
                        .pointer(
                            &crate::desktop_browser::TabKey {
                                thread_id: thread.into(),
                                tab_id: tab.into(),
                            },
                            phase,
                            x,
                            y,
                        )
                        .await;
                }
            }
        }
        request_in(&browser.tx, method, params, browser.session)
            .await
            .map_err(error)
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
        process_done: mut done,
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
    let viewer = if operate {
        state.streams.p2.preview.browser.claim_viewer(&thread, &tab)
    } else {
        None
    };
    let operate = viewer.is_some();
    let _viewer = viewer;
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
                crate::preview::sync_navigation(&state,&thread,&tab).await;
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
                    Some("back"|"forward")=>{
                        let Ok(h)=state.streams.p2.preview.browser.command(&thread,&tab,"Page.getNavigationHistory",json!({})).await else {continue;};
                        let at=h["currentIndex"].as_i64().unwrap_or(0)+if v["type"]=="back" {-1}else{1};
                        let Some(entry)=usize::try_from(at).ok().and_then(|i|h["entries"].get(i)) else {continue;};
                        ("Page.navigateToHistoryEntry",json!({"entryId":entry["id"]}))
                    },
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
