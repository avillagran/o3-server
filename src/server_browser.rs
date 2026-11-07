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
pub(crate) mod dialog;
mod events;
pub(crate) mod exact;
#[cfg(test)]
mod lifetime_tests;
mod native_grant;
pub(crate) mod native_owner;
#[cfg(test)]
mod native_owner_tests;
#[cfg(test)]
mod ownership_tests;
mod pending;
mod pipe;
mod viewer;
mod write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Local,
    Desktop,
}

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
    setup: Mutex<()>,
    tabs: std::sync::Mutex<BTreeMap<(String, String), Tab>>,
    viewers: ViewerRegistry,
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
    exact_tx: mpsc::Sender<exact::Request>,
    exact_wake: std::sync::Arc<tokio::sync::Notify>,
    closing: bool,
    keyboard: std::sync::Arc<std::sync::Mutex<crate::browser_keyboard::Keyboard>>,
    activity: std::sync::Arc<TabActivity>,
    control: Option<crate::preview_control::runtime::Control>,
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
struct TabActivity {
    order: u64,
    created: std::sync::atomic::AtomicI64,
    used: std::sync::atomic::AtomicI64,
    admitted: std::sync::atomic::AtomicBool,
}
impl Default for TabActivity {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static ORDER: AtomicU64 = AtomicU64::new(0);
        let now = crate::store::now_ms();
        Self {
            order: ORDER.fetch_add(1, Ordering::Relaxed),
            created: now.into(),
            used: now.into(),
            admitted: false.into(),
        }
    }
}
impl TabActivity {
    fn admit(&self) {
        use std::sync::atomic::Ordering;
        let now = crate::store::now_ms();
        self.created.store(now, Ordering::Relaxed);
        self.used.store(now, Ordering::Relaxed);
        self.admitted.store(true, Ordering::Release);
    }
}
pub(crate) struct OpenPage<'a> {
    pub url: Option<&'a str>,
    pub owner: Option<&'a str>,
}
/// Immutable transport identity captured before submitting controlled work.
#[derive(Clone)]
pub(crate) struct Page {
    key: (String, String),
    tab: Tab,
}
impl Page {
    pub(crate) async fn press(&self, chord: &str) -> Result<(), Value> {
        let known = self.tab.keyboard.lock().unwrap().browser_mac.is_some();
        if !known {
            let version = self.command("Browser.getVersion", json!({})).await?;
            let agent = version["userAgent"]
                .as_str()
                .ok_or_else(|| error("Browser platform is unavailable".into()))?;
            self.tab.keyboard.lock().unwrap().browser_mac =
                Some(crate::browser_keyboard::browser_mac(agent));
        }
        for (down, key) in crate::browser_keyboard::steps(chord) {
            let params = self
                .tab
                .keyboard
                .lock()
                .unwrap()
                .event(down, &key, cfg!(target_os = "macos"))
                .map_err(error)?;
            self.command("Input.dispatchKeyEvent", params).await?;
        }
        Ok(())
    }
    pub(crate) fn key(&self) -> (&str, &str) {
        (&self.key.0, &self.key.1)
    }
    pub(crate) fn control(&self) -> crate::preview_control::runtime::Control {
        self.tab
            .control
            .as_ref()
            .expect("admitted page control")
            .clone()
    }
    fn live(&self) -> Result<(), Value> {
        if *self.tab.done.borrow() || self.control().snapshot().closed {
            return Err(error("Browser tab is closed".into()));
        }
        Ok(())
    }
    pub(crate) async fn command(&self, method: &str, params: Value) -> Result<Value, Value> {
        self.live()?;
        if method == "Input.dispatchMouseEvent" {
            if let (Some(channel), Some(x), Some(y)) = (
                &self.tab.desktop_channel,
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
                                thread_id: self.key.0.clone(),
                                tab_id: self.key.1.clone(),
                            },
                            phase,
                            x,
                            y,
                        )
                        .await;
                }
            }
        }
        self.live()?;
        request_in(&self.tab.tx, method, params, self.tab.session.clone())
            .await
            .map_err(error)
    }
    fn dialog_watch(&self) -> Option<tokio::sync::watch::Receiver<dialog::State>> {
        self.live().ok()?;
        self.tab
            .events
            .as_ref()
            .zip(self.tab.session.as_deref())
            .and_then(|(pages, session)| events::dialog_watch(pages, session))
    }
    pub(crate) fn dialog_status(&self) -> Result<Value, Value> {
        self.live()?;
        self.tab
            .events
            .as_ref()
            .zip(self.tab.session.as_deref())
            .and_then(|(pages, session)| events::dialog_state(pages, session))
            .map(|state| state.status())
            .ok_or_else(|| error("Browser tab is closed".into()))
    }
    pub(crate) fn network_entries(&self) -> Result<Vec<Value>, Value> {
        self.live()?;
        self.tab
            .events
            .as_ref()
            .zip(self.tab.session.as_deref())
            .and_then(|(pages, session)| events::entries(pages, session))
            .ok_or_else(|| error("Browser tab is closed".into()))
    }
    pub(crate) async fn screenshot(&self) -> Result<Vec<u8>, Value> {
        let value = self
            .command(
                "Page.captureScreenshot",
                json!({"format":"jpeg","quality":70}),
            )
            .await?;
        base64::engine::general_purpose::STANDARD
            .decode(value["data"].as_str().unwrap_or(""))
            .map_err(|_| error("Invalid browser image".into()))
    }
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
    let (mut incoming, reader_task) = pipe::start_raw(input);
    let reader_abort = pipe::Abort(reader_task.abort_handle());
    let pages: events::Shared = Default::default();
    let actor_pages = pages.clone();
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let (exact_tx, mut exact_rx) = mpsc::channel::<exact::Request>(8);
    let exact_wake = std::sync::Arc::new(tokio::sync::Notify::new());
    let write_wake = exact_wake.clone();
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let stop_signal = cancel.clone();
    tokio::spawn(async move {
        let _group = group;
        let _reader_abort = reader_abort;
        let mut calls = pending::Calls::default();
        let mut writing_done = writer.completion();
        loop {
            let expires = calls.deadline();
            tokio::select! {
                next=rx.recv(),if !calls.legacy_full()=>{
                    let Some(next)=next else{break;};
                    let Some(message)=calls.insert(next)else{continue;};
                    let mut packet=match serde_json::to_vec(&message){Ok(packet)=>packet,Err(_)=>break};packet.push(0);
                    if !writer.send(packet){break;}
                },
                next=exact_rx.recv()=>{
                    let Some(next)=next else{break;};
                    let Some((id,mut packet,permit))=calls.insert_exact(next)else{continue;};packet.push(0);
                    if !writer.send_exact(packet,permit){calls.exact_unsent(id);}
                },
                _=write_wake.notified()=>calls.cancel_exact(),
                _=pending::deadline(expires)=>{if calls.expire(tokio::time::Instant::now()){break;}},
                _=writing_done.changed()=>break,
                _=stop_signal.notified()=>break,
                _=child.wait()=>break,
                packet=incoming.recv()=>{
                    let Some(Ok(bytes))=packet else{break;};
                    if calls.complete_exact(&bytes){continue;}
                    if native_owner::route(&actor_pages,&bytes){continue;}
                    let Ok(packet)=crate::cdp_dialog_json::decode(&bytes)else{break;};
                    if events::notify_packet(&actor_pages,&packet){break;}
                    calls.complete(&packet.value,&actor_pages);
                }
            }
        }
        drop(rx);
        drop(exact_rx);
        drop(calls);
        native_owner::stopped(&actor_pages);
        let controls = events::control_receipts(&actor_pages);
        for control in &controls {
            drop(control.close());
        }
        writer.stop().await;
        reader_task.abort();
        let _ = reader_task.await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(_group);
        if let Some(scope) = agent_scope.as_mut() {
            scope.stop().await;
        }
        events::stopped(&actor_pages);
        let mut cleanup_ready = true;
        for control in &controls {
            if control.shutdown().await.is_err() {
                cleanup_ready = false;
            }
        }
        if let Some(identity) = ephemeral_identity.filter(|_| cleanup_ready) {
            if let Ok(plan) = crate::cleanup::RemovalPlan::prepare(&dir, &crate::cleanup::FileTree)
            {
                if plan.operations().last().is_some_and(|operation| {
                    operation.path == dir && operation.identity == identity
                }) {
                    let _ = removal.execute(&plan);
                }
            }
        }
        if cleanup_ready {
            let _ = done_tx.send(true);
        }
    });
    Ok(Tab {
        exact_tx,
        exact_wake,
        keyboard: Default::default(),
        closing: false,
        activity: Default::default(),
        control: None,
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

type ViewerRegistry = std::sync::Arc<
    std::sync::Mutex<BTreeMap<(String, String), std::collections::BTreeSet<String>>>,
>;
struct RemoveViewer {
    registry: ViewerRegistry,
    key: (String, String),
    id: String,
}
impl Drop for RemoveViewer {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        if let Some(viewers) = registry.get_mut(&self.key) {
            viewers.remove(&self.id);
            if viewers.is_empty() {
                registry.remove(&self.key);
            }
        }
    }
}
struct Viewer {
    remove: Option<RemoveViewer>,
    page: Page,
    inputs: viewer::Inputs,
}
impl Drop for Viewer {
    fn drop(&mut self) {
        let remove = self.remove.take().unwrap();
        let page = self.page.clone();
        let inputs = self.inputs.clone();
        let control = page.control();
        let call = control.disconnect(&remove.id, move || viewer::release(page, inputs));
        control.own_finalizer(async move {
            let _ = call.wait().await;
            drop(remove);
        });
    }
}
impl Browsers {
    fn lifetime_catalog(&self) -> Vec<(Page, crate::browser_lifetime::Tab)> {
        use std::sync::atomic::Ordering;
        let viewers = self.viewers.lock().unwrap();
        let tabs = self.tabs.lock().unwrap();
        let mut entries = tabs
            .iter()
            .filter_map(|(key, tab)| {
                if tab.closing
                    || *tab.done.borrow()
                    || !tab.activity.admitted.load(Ordering::Acquire)
                {
                    return None;
                }
                let control = tab.control.as_ref()?.snapshot();
                if control.closed {
                    return None;
                }
                let shown = tab.desktop_channel.as_ref().is_some_and(|channel| {
                    channel.is_shown(&crate::desktop_browser::TabKey {
                        thread_id: key.0.clone(),
                        tab_id: key.1.clone(),
                    })
                });
                Some((
                    Page {
                        key: key.clone(),
                        tab: tab.clone(),
                    },
                    crate::browser_lifetime::Tab {
                        thread_id: key.0.clone(),
                        tab_id: key.1.clone(),
                        agent_id: control.agent_id,
                        desktop: tab.origin == Origin::Desktop,
                        viewers: viewers.get(key).map_or(0, |viewers| viewers.len()),
                        shown,
                        used_at: tab.activity.used.load(Ordering::Relaxed),
                        created_at: tab.activity.created.load(Ordering::Relaxed),
                    },
                ))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(page, _)| page.tab.activity.order);
        entries
    }
    pub(crate) fn mark_used(&self, thread: &str, agent: &str, explicit: Option<&str>) {
        use std::sync::atomic::Ordering;
        let tabs = self.tabs.lock().unwrap();
        let entries: Vec<_> = tabs
            .iter()
            .filter_map(|(key, tab)| {
                if tab.closing
                    || *tab.done.borrow()
                    || !tab.activity.admitted.load(Ordering::Acquire)
                {
                    return None;
                }
                let control = tab.control.as_ref()?.snapshot();
                if control.closed {
                    return None;
                }
                Some((
                    tab,
                    crate::browser_lifetime::Tab {
                        thread_id: key.0.clone(),
                        tab_id: key.1.clone(),
                        agent_id: control.agent_id,
                        desktop: tab.origin == Origin::Desktop,
                        viewers: 0,
                        shown: false,
                        used_at: 0,
                        created_at: 0,
                    },
                ))
            })
            .collect();
        let rows: Vec<_> = entries.iter().map(|(_, row)| row.clone()).collect();
        let now = crate::store::now_ms();
        for index in crate::browser_lifetime::used_indices(&rows, thread, agent, explicit) {
            entries[index].0.activity.used.store(now, Ordering::Relaxed);
        }
    }
    pub(crate) fn latest_tab(&self, thread: &str, agent: Option<&str>) -> Option<String> {
        let entries = self.lifetime_catalog();
        let rows: Vec<_> = entries.iter().map(|(_, row)| row.clone()).collect();
        crate::browser_lifetime::latest(&rows, thread, agent)
            .map(|index| rows[index].tab_id.clone())
    }
    pub(crate) fn thread_tabs(&self, thread: &str) -> Vec<String> {
        self.lifetime_catalog()
            .into_iter()
            .filter_map(|(_, row)| (row.thread_id == thread).then_some(row.tab_id))
            .collect()
    }
    pub(crate) fn owned_tabs(&self, thread: &str, agent: &str) -> Vec<String> {
        self.lifetime_catalog()
            .into_iter()
            .filter_map(|(_, row)| {
                (row.thread_id == thread && row.agent_id.as_deref() == Some(agent))
                    .then_some(row.tab_id)
            })
            .collect()
    }
    pub(crate) fn tab_capacity(
        &self,
        pending: &[crate::browser_lifetime::Pending],
        agent: &str,
    ) -> (bool, bool) {
        let rows: Vec<_> = self
            .lifetime_catalog()
            .into_iter()
            .map(|(_, row)| row)
            .collect();
        crate::browser_lifetime::capacity(&rows, pending, agent)
    }
    pub(crate) fn reclaim_candidate(
        &self,
        pending: &[crate::browser_lifetime::Pending],
        agent: &str,
        now: i64,
    ) -> Option<Page> {
        let entries = self.lifetime_catalog();
        let rows: Vec<_> = entries.iter().map(|(_, row)| row.clone()).collect();
        crate::browser_lifetime::reclaim(&rows, pending, agent, now)
            .map(|index| entries[index].0.clone())
    }
    pub(crate) fn idle_candidates(&self, now: i64, headless_only: bool) -> Vec<Page> {
        let entries = self.lifetime_catalog();
        let rows: Vec<_> = entries.iter().map(|(_, row)| row.clone()).collect();
        let indices = if headless_only {
            crate::browser_lifetime::headless_idle(&rows, now)
        } else {
            crate::browser_lifetime::idle(&rows, now)
        };
        indices
            .into_iter()
            .map(|index| entries[index].0.clone())
            .collect()
    }
    /// Recheck visibility, time and the exact page while reserving close. A
    /// newly attached viewer cannot be swept from an earlier visibility read.
    pub(crate) fn reserve_idle(&self, page: &Page, now: i64, headless_only: bool) -> bool {
        use std::sync::atomic::Ordering;
        let viewers = self.viewers.lock().unwrap();
        let mut tabs = self.tabs.lock().unwrap();
        let Some(current) = tabs
            .get_mut(&page.key)
            .filter(|current| std::sync::Arc::ptr_eq(&current.activity, &page.tab.activity))
        else {
            return false;
        };
        let control = current.control.as_ref().map(|control| control.snapshot());
        let visible = viewers
            .get(&page.key)
            .is_some_and(|viewers| !viewers.is_empty());
        if current.closing
            || *current.done.borrow()
            || visible
            || !current.activity.admitted.load(Ordering::Acquire)
            || control.is_none_or(|control| control.closed || control.agent_id.is_none())
            || (headless_only && current.origin == Origin::Desktop)
            || current.activity.used.load(Ordering::Relaxed)
                >= now.saturating_sub(crate::browser_lifetime::IDLE_MS)
        {
            return false;
        }
        let channel = current.desktop_channel.clone();
        let hidden = if let Some(channel) = &channel {
            let key = crate::desktop_browser::TabKey {
                thread_id: page.key.0.clone(),
                tab_id: page.key.1.clone(),
            };
            let Some(hidden) = channel.lock_hidden(&key) else {
                return false;
            };
            Some(hidden)
        } else {
            None
        };
        current.closing = true;
        let control = current.control.clone();
        drop(hidden);
        drop(tabs);
        drop(viewers);
        if let Some(control) = control {
            drop(control.close());
        }
        true
    }
    pub(crate) fn set_profile_removal(&self, removal: std::sync::Arc<dyn crate::cleanup::Removal>) {
        *self.removal.0.lock().unwrap() = removal;
    }

    pub fn human_controls(&self, thread: &str, tab: &str) -> bool {
        self.page(thread, tab)
            .is_ok_and(|page| page.control().snapshot().controller.is_some())
    }
    pub(crate) fn visible(&self, thread: &str, tab: &str) -> bool {
        if self
            .viewers
            .lock()
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .is_some_and(|viewers| !viewers.is_empty())
        {
            return true;
        }
        self.tabs
            .lock()
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .and_then(|tab| tab.desktop_channel.as_ref())
            .is_some_and(|channel| {
                channel.is_shown(&crate::desktop_browser::TabKey {
                    thread_id: thread.into(),
                    tab_id: tab.into(),
                })
            })
    }
    fn claim_viewer(&self, page: Page, id: String) -> Result<Viewer, Value> {
        let key = page.key.clone();
        let mut viewers = self.viewers.lock().unwrap();
        let tabs = self.tabs.lock().unwrap();
        if !tabs.get(&key).is_some_and(|current| {
            !current.closing
                && !*current.done.borrow()
                && std::sync::Arc::ptr_eq(&current.activity, &page.tab.activity)
        }) {
            return Err(error("Browser tab is closed".into()));
        }
        page.live()?;
        drop(tabs);
        viewers.entry(key.clone()).or_default().insert(id.clone());
        Ok(Viewer {
            remove: Some(RemoveViewer {
                registry: self.viewers.clone(),
                key,
                id,
            }),
            page,
            inputs: Default::default(),
        })
    }

    // Keep cleanup receipts in the authoritative map across cancellation and failed stops.
    fn closing_tabs(&self, mut matches: impl FnMut(&(String, String), &Tab) -> bool) -> Vec<Tab> {
        let mut tabs = self.tabs.lock().unwrap();
        tabs.iter_mut()
            .filter_map(|(key, tab)| {
                if !matches(key, tab) {
                    return None;
                }
                tab.closing = true;
                Some(tab.clone())
            })
            .collect()
    }
    fn forget_stopped(&self) {
        self.tabs.lock().unwrap().retain(|_, tab| {
            !(tab.closing
                && *tab.process_done.borrow()
                && tab
                    .control
                    .as_ref()
                    .is_none_or(|control| control.is_finished()))
        });
    }
    async fn rollback_setup(&self, browser: Tab, fence: bool) {
        if let Some(control) = &browser.control {
            drop(control.close());
        }
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
        let shared = self.tabs.lock().unwrap().values().any(|other| {
            !other.closing && !*other.done.borrow() && other.tx.same_channel(&browser.tx)
        });
        if !fence && closed && shared {
            if let Some(control) = &browser.control {
                if control.shutdown().await.is_err() {
                    return;
                }
            }
            self.tabs.lock().unwrap().retain(|_, other| {
                !(other.closing
                    && other.tx.same_channel(&browser.tx)
                    && other.target == browser.target)
            });
        } else {
            if fence || !closed {
                self.closing_tabs(|_, other| other.tx.same_channel(&browser.tx));
            }
            stop(browser).await;
            self.forget_stopped();
        }
    }
    pub(crate) fn reserve_close(&self, thread: &str, tab: Option<&str>) {
        self.closing_tabs(|(tid, id), _| tid == thread && tab.is_none_or(|wanted| wanted == id));
    }
    pub async fn clear(&self, root: &Path, profile: &str) -> Result<(), Value> {
        if profile == "incognito" {
            return Ok(());
        }
        self.closing_tabs(|_, tab| tab.origin == Origin::Local && tab.profile == profile);
        let _setup = self.setup.lock().await;
        let tabs =
            self.closing_tabs(|_, tab| tab.origin == Origin::Local && tab.profile == profile);
        for tab in tabs {
            if !stop(tab).await {
                return Err(error(
                    "Browser has not stopped; profile was not removed".into(),
                ));
            }
        }
        self.forget_stopped();
        let path = directory(root, profile).map_err(error)?;
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
        self.closing_tabs(|_, _| true);
        let _setup = self.setup.lock().await;
        let tabs = self.closing_tabs(|_, _| true);
        for tab in tabs {
            stop(tab).await;
        }
        self.forget_stopped();
    }
    pub async fn close(&self, thread: &str, tab: Option<&str>) {
        self.reserve_close(thread, tab);
        let _setup = self.setup.lock().await;
        let tabs =
            self.closing_tabs(|(tid, id), _| tid == thread && tab.is_none_or(|tab| tab == id));
        let mut outcomes = Vec::new();
        for tab in tabs {
            let was_live = !*tab.done.borrow();
            let closed = if let Some(target) = tab
                .target
                .as_ref()
                .filter(|_| tab.origin == Origin::Local && was_live)
            {
                request(&tab.tx, "Target.closeTarget", json!({"targetId":target}))
                    .await
                    .is_ok()
            } else {
                true
            };
            // A canceled close must not turn an unacknowledged target into a stale-page no-op.
            if closed {
                if let (Some(pages), Some(session)) = (&tab.events, &tab.session) {
                    events::remove(pages, session);
                }
            }
            outcomes.push((tab, closed));
        }
        for (tab, closed) in outcomes {
            let shared = self.tabs.lock().unwrap().values().any(|other| {
                !other.closing && !*other.done.borrow() && other.tx.same_channel(&tab.tx)
            });
            if shared && closed {
                if let Some(control) = &tab.control {
                    if control.shutdown().await.is_err() {
                        continue;
                    }
                }
                self.tabs.lock().unwrap().retain(|_, other| {
                    !(other.closing && other.tx.same_channel(&tab.tx) && other.target == tab.target)
                });
            } else {
                if !closed {
                    self.closing_tabs(|_, other| other.tx.same_channel(&tab.tx));
                }
                stop(tab).await;
                self.forget_stopped();
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
        page: OpenPage<'_>,
    ) -> Result<(), Value> {
        let OpenPage { url, owner } = page;
        let _setup = self.setup.lock().await;
        let tabs = {
            let mut tabs = self.tabs.lock().unwrap();
            tabs.retain(|_, t| {
                t.origin == Origin::Desktop
                    || !*t.process_done.borrow()
                    || t.control
                        .as_ref()
                        .is_some_and(|control| !control.is_finished())
            });
            tabs.clone()
        };
        if tabs.values().any(|t| {
            t.origin == Origin::Local
                && t.profile == profile
                && !*t.process_done.borrow()
                && (t.closing
                    || (*t.done.borrow()
                        && !tabs.values().any(|other| {
                            !other.closing && !*other.done.borrow() && other.tx.same_channel(&t.tx)
                        })))
        }) {
            return Err(error("Browser profile is still closing".into()));
        }
        let key = (thread.into(), tab.into());
        if tabs.contains_key(&key) {
            return Err(error("Browser tab is already owned".into()));
        }
        let mut browser = if let Some(existing) = tabs.values().find(|t| {
            t.origin == Origin::Local && t.profile == profile && !t.closing && !*t.done.borrow()
        }) {
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
        browser.control = Some(crate::preview_control::runtime::Control::new(
            owner.map(str::to_owned),
        ));
        browser.activity = Default::default();
        browser.keyboard = Default::default();
        browser.closing = true;
        // A canceled setup retains a non-usable cleanup receipt, including before target discovery.
        self.tabs
            .lock()
            .unwrap()
            .insert(key.clone(), browser.clone());
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
                self.rollback_setup(browser, fence).await;
                return Err(error(cause));
            }
        };
        let Some(target_id) = target["targetId"]
            .as_str()
            .filter(|value| !value.is_empty())
        else {
            self.rollback_setup(browser, true).await;
            return Err(error("Browser target creation failed".into()));
        };
        browser.target = Some(target_id.to_owned());
        self.tabs
            .lock()
            .unwrap()
            .insert(key.clone(), browser.clone());
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
                if let Some(control) = &browser.control {
                    events::register_control(pages, control.clone());
                }
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
            self.rollback_setup(browser, false).await;
            return Err(error(cause));
        }
        browser.closing = false;
        browser.activity.admit();
        self.tabs.lock().unwrap().insert(key, browser);
        Ok(())
    }
    /// Adopt a frontend-owned page without navigating or creating another target.
    /// Unowned adoption helper for the actual private actor fixtures.
    #[cfg(test)]
    pub async fn open_desktop(
        &self,
        channel: &crate::desktop_browser_channel::Channel,
        thread: &str,
        tab: &str,
        profile: &str,
    ) -> Result<(), Value> {
        self.open_desktop_owned(channel, thread, tab, profile, None)
            .await
    }
    pub(crate) async fn open_desktop_owned(
        &self,
        channel: &crate::desktop_browser_channel::Channel,
        thread: &str,
        tab: &str,
        profile: &str,
        owner: Option<&str>,
    ) -> Result<(), Value> {
        let _setup = self.setup.lock().await;
        let tabs = self.tabs.lock().unwrap().clone();
        let key = (thread.into(), tab.into());
        if let Some(existing) = tabs.get(&key) {
            if existing.origin != Origin::Desktop {
                return Err(error("Browser tab is already locally owned".into()));
            }
            if existing.closing && !*existing.process_done.borrow() {
                return Err(error("Desktop browser connection is still closing".into()));
            }
            if !*existing.done.borrow() {
                return Ok(());
            }
            if !stop(existing.clone()).await {
                return Err(error("Desktop browser connection is still closing".into()));
            }
        }
        let browser = desktop::launch(channel, thread, tab, profile, owner)
            .await
            .map_err(error)?;
        browser.activity.admit();
        self.tabs.lock().unwrap().insert(key, browser);
        Ok(())
    }
    /// A disconnected desktop page remains frontend-owned and may be reattached.
    pub async fn connection(&self, thread: &str, tab: &str) -> Option<(Origin, bool)> {
        self.tabs
            .lock()
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .map(|tab| {
                (
                    tab.origin,
                    !tab.closing
                        && !*tab.done.borrow()
                        && tab
                            .control
                            .as_ref()
                            .is_some_and(|control| !control.snapshot().closed),
                )
            })
    }
    pub(crate) async fn navigation(
        &self,
        thread: &str,
        tab: &str,
    ) -> Option<tokio::sync::watch::Receiver<Navigation>> {
        self.tabs
            .lock()
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .and_then(|tab| tab.navigation.clone())
    }
    #[cfg(test)]
    pub(crate) async fn network_entries(
        &self,
        thread: &str,
        tab: &str,
    ) -> Result<Vec<Value>, Value> {
        self.page(thread, tab)?.network_entries()
    }
    pub(crate) async fn note_navigation_url(
        &self,
        thread: &str,
        tab: &str,
        watch: &tokio::sync::watch::Receiver<Navigation>,
        url: &str,
    ) {
        let tabs = self.tabs.lock().unwrap();
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
        let tabs = self.tabs.lock().unwrap();
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
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .map(|t| t.done.clone())
    }
    fn dialog_tab(&self, thread: &str, tab: &str) -> Result<Tab, Value> {
        self.tabs
            .lock()
            .unwrap()
            .get(&(thread.into(), tab.into()))
            .filter(|tab| !tab.closing && !*tab.done.borrow())
            .cloned()
            .ok_or_else(|| error("Browser tab is closed".into()))
    }
    pub(crate) fn dialog_state(&self, thread: &str, tab: &str) -> Result<dialog::State, Value> {
        let tab = self.dialog_tab(thread, tab)?;
        tab.events
            .as_ref()
            .zip(tab.session.as_deref())
            .and_then(|(pages, session)| events::dialog_state(pages, session))
            .ok_or_else(|| error("Browser tab is closed".into()))
    }
    pub(crate) fn dialog_status(&self, thread: &str, tab: &str) -> Result<Value, Value> {
        let tab = self.dialog_tab(thread, tab)?;
        Ok(tab
            .events
            .as_ref()
            .zip(tab.session.as_deref())
            .and_then(|(pages, session)| events::dialog_state(pages, session))
            .map_or(Value::Null, |state| state.status()))
    }
    /// Native status skips page JavaScript while modal, but reads a fresh normal title.
    #[cfg(test)]
    pub(crate) async fn title_status(
        &self,
        thread: &str,
        tab: &str,
    ) -> Result<(Value, Value), Value> {
        self.title_status_exact(thread, tab)
            .await
            .map(|(dialog, title)| (dialog.status(), title))
    }
    pub(crate) async fn title_status_exact(
        &self,
        thread: &str,
        tab: &str,
    ) -> Result<(dialog::State, Value), Value> {
        let tab = self.dialog_tab(thread, tab)?;
        let (pages, session) = tab
            .events
            .as_ref()
            .zip(tab.session.as_deref())
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        let mut dialogs = events::dialog_watch(pages, session)
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        let pending = dialogs.borrow().clone();
        if !pending.status().is_null() {
            return Ok((pending, Value::Null));
        }
        let query = request_in(
            &tab.tx,
            "Runtime.evaluate",
            json!({"expression":"document.title","returnByValue":true}),
            tab.session.clone(),
        );
        tokio::pin!(query);
        loop {
            tokio::select! {
                result=&mut query=>{
                    let title=result.ok().and_then(|value|value["result"]["value"].as_str().filter(|title|!title.is_empty()).map(str::to_owned)).map_or(Value::Null,|title|json!(title));
                    let pending=dialogs.borrow().clone();
                    return Ok((pending.clone(),if pending.status().is_null(){title}else{Value::Null}));
                },
            changed=dialogs.changed()=>{
                    if changed.is_err(){return Err(error("Browser tab is closed".into()));}
                    let pending=dialogs.borrow_and_update().clone();
                    if !pending.status().is_null(){return Ok((pending,Value::Null));}
                }
            }
        }
    }
    pub(crate) async fn answer_dialog(
        &self,
        thread: &str,
        tab: &str,
        accept: bool,
        prompt: Option<&str>,
    ) -> Result<(), Value> {
        let tab = self.dialog_tab(thread, tab)?;
        let (pages, session) = tab
            .events
            .as_ref()
            .zip(tab.session.as_deref())
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        let (token, params) = events::begin_dialog(pages, session, accept, prompt)
            .map_err(|cause| error(cause.into()))?;
        request_in(
            &tab.tx,
            "Page.handleJavaScriptDialog",
            params,
            tab.session.clone(),
        )
        .await
        .map_err(error)?;
        events::finish_dialog(pages, session, token);
        Ok(())
    }
    pub(crate) fn page(&self, thread: &str, tab: &str) -> Result<Page, Value> {
        let key = (thread.into(), tab.into());
        let tab = self
            .tabs
            .lock()
            .unwrap()
            .get(&key)
            .filter(|tab| !tab.closing && !*tab.done.borrow() && tab.control.is_some())
            .cloned()
            .ok_or_else(|| error("Browser tab is closed".into()))?;
        let page = Page { key, tab };
        page.live()?;
        Ok(page)
    }
    pub(crate) fn current(&self, page: &Page) -> bool {
        self.tabs.lock().unwrap().get(&page.key).is_some_and(|tab| {
            !tab.closing
                && !*tab.done.borrow()
                && tab.tx.same_channel(&page.tab.tx)
                && tab.session == page.tab.session
                && std::sync::Arc::ptr_eq(&tab.activity, &page.tab.activity)
        })
    }
    pub async fn command(
        &self,
        thread: &str,
        tab: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Value> {
        self.page(thread, tab)?.command(method, params).await
    }
    #[cfg(test)]
    pub async fn screenshot(&self, thread: &str, tab: &str) -> Result<Vec<u8>, Value> {
        self.page(thread, tab)?.screenshot().await
    }
}

async fn stop(tab: Tab) -> bool {
    let Tab {
        tx,
        process_done: mut done,
        cancel,
        control,
        ..
    } = tab;
    if let Some(control) = &control {
        drop(control.close());
    }
    cancel.notify_one();
    drop(tx);
    if !*done.borrow() {
        let _ = tokio::time::timeout(Duration::from_secs(7), done.changed()).await;
    }
    let stopped = *done.borrow();
    if !stopped {
        return false;
    }
    if let Some(control) = control {
        return control.shutdown().await.is_ok();
    }
    true
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
    let Ok(page) = state.streams.p2.preview.browser.page(&thread, &tab) else {
        return;
    };
    let control = page.control();
    let Ok(viewer_id) = crate::store::random_id() else {
        return;
    };
    let Ok(viewer) = state
        .streams
        .p2
        .preview
        .browser
        .claim_viewer(page.clone(), viewer_id.clone())
    else {
        return;
    };
    let inputs = viewer.inputs.clone();
    let _viewer = viewer;
    let snapshot = control.snapshot();
    if operate
        && snapshot.agent_id.is_none()
        && snapshot.controller.is_none()
        && control.take(&viewer_id).wait().await.is_err()
    {
        return;
    }
    let mut controls = control.subscribe();
    let Some(mut dialogs) = page.dialog_watch() else {
        return;
    };
    let mut dialog_revision = dialogs.borrow().broadcast_revision;
    let hello = viewer::control(
        &control.snapshot(),
        &viewer_id,
        operate,
        dialogs.borrow().status(),
    );
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut waiting: Option<std::time::Instant> = None;
    type Frame =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, Value>> + Send>>;
    let mut frame_task: Option<Frame> = None;
    let mut operations: tokio::task::JoinSet<(u64, Result<Value, Value>)> =
        tokio::task::JoinSet::new();
    let mut observing = std::collections::BTreeSet::new();
    loop {
        tokio::select! {
                changed=controls.changed()=>{
                if changed.is_err()||controls.borrow_and_update().closed{break;}
                let control=viewer::control(&control.snapshot(),&viewer_id,operate,dialogs.borrow().status());
                if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Text(control.to_string().into()))).await,Ok(Ok(()))){break;}
            }
            finished=operations.join_next(),if !operations.is_empty()=>{
                if let Some(Ok((id,result)))=finished {
                    observing.remove(&id);
                    if result.is_err(){let update=viewer::control(&control.snapshot(),&viewer_id,operate,dialogs.borrow().status());if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Text(update.to_string().into()))).await,Ok(Ok(()))){break;}}
                }
            }
            changed=dialogs.changed()=>{
                if changed.is_err(){break;}
                let snapshot=dialogs.borrow_and_update().clone();
                if snapshot.broadcast_revision!=dialog_revision {
                    dialog_revision=snapshot.broadcast_revision;
                    let control=viewer::control(&control.snapshot(),&viewer_id,operate,snapshot.status());
                    if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Text(control.to_string().into()))).await,Ok(Ok(()))){break;}
                }
            }
            frame=async {match frame_task.as_mut(){Some(task)=>task.await,None=>std::future::pending().await}},if frame_task.is_some()=>{
                frame_task=None;
                let Ok(frame)=frame else{break;};
                if !matches!(tokio::time::timeout(Duration::from_secs(3),socket.send(Message::Binary(frame.into()))).await,Ok(Ok(()))){break;}
                waiting=Some(std::time::Instant::now());
            }
            _=interval.tick()=>{
                let valid=viewer::live(&state,&session);
                if !valid||waiting.is_some_and(|at|at.elapsed()>Duration::from_secs(10)){break;}
                if waiting.is_some()||frame_task.is_some(){continue;}
                let page=page.clone();
                frame_task=Some(Box::pin(async move{page.screenshot().await}));
            }
            incoming=socket.next()=>{
                let Some(Ok(Message::Text(raw)))=incoming else{break;};
                if raw.len()>16384 {break;}
                let Ok(v)=serde_json::from_str::<Value>(&raw) else{continue;};
                if v["type"]=="ack" {waiting=None;continue;}
                if !viewer::live(&state,&session){break;}
                if !operate {continue;}
                if v["type"]=="takeControl"{let call=control.take(&viewer_id);let id=call.action_id();if observing.insert(id){operations.spawn(async move{(id,call.wait().await)});}continue;}
                if v["type"]=="releaseControl"{let page=page.clone();let inputs=inputs.clone();let call=control.release(&viewer_id,move||viewer::release(page,inputs));let id=call.action_id();if observing.insert(id){operations.spawn(async move{(id,call.wait().await)});}continue;}
                if v["type"]=="dialog" {
                    if control.snapshot().controller.as_deref()==Some(&viewer_id){if let Some(accept)=v["accept"].as_bool(){let _=state.streams.p2.preview.browser.answer_dialog(&thread,&tab,accept,v["promptText"].as_str()).await;}}
                    continue;
                }
                let (method,params)=match v["type"].as_str(){
                    Some("text")=>("Input.insertText",json!({"text":v["text"].as_str().unwrap_or("").chars().take(10000).collect::<String>()})),
                    Some("navigate")=>{let Ok(url)=crate::preview::normalize_url(v["url"].as_str().unwrap_or(""))else{continue;};("Page.navigate",json!({"url":url}))},
                    Some("back"|"forward")=>("Viewer.history",json!({"offset":if v["type"]=="back"{-1}else{1}})),
                    Some("reload")=>("Page.reload",json!({"ignoreCache":v["ignoreCache"]==true})),
                    Some("resize")=>("Emulation.setDeviceMetricsOverride",json!({"width":v["width"].as_u64().unwrap_or(1280).clamp(240,3840),"height":v["height"].as_u64().unwrap_or(800).clamp(240,2160),"deviceScaleFactor":1,"mobile":false})),
                    Some("key")=>("Input.dispatchKeyEvent",json!({"type":if v["action"]=="up"{"keyUp"}else{"keyDown"},"key":v["key"],"code":v["code"],"text":v["text"].as_str().unwrap_or("")})),
                    Some("mouse"|"wheel")=>("Input.dispatchMouseEvent",json!({"type":if v["type"]=="wheel"{"mouseWheel"}else if v["action"]=="down"{"mousePressed"}else if v["action"]=="up"{"mouseReleased"}else{"mouseMoved"},"x":v["x"].as_f64().unwrap_or(0.0),"y":v["y"].as_f64().unwrap_or(0.0),"button":v["button"].as_str().unwrap_or("none"),"clickCount":v["clickCount"].as_u64().unwrap_or(1).min(3),"deltaX":v["deltaX"].as_f64().unwrap_or(0.0),"deltaY":v["deltaY"].as_f64().unwrap_or(0.0)})),
                    _=>continue,
                };
                let page=page.clone();let inputs=inputs.clone();let state=state.clone();let session=session.clone();
                let work=move|params|async move{
                    if !viewer::live(&state,&session){return Err(error("Browser viewer session ended".into()));}
                    viewer::dispatch(&page,&inputs,method,params).await
                };
                let call=if let Some(key)=viewer::latest_key(&v){
                    let wheel=v["type"]=="wheel";
                    control.human_latest(&viewer_id,&key,params,work,move|previous,next|if wheel{viewer::merge_wheel(previous,next)}else{next})
                }else{control.human(&viewer_id,move||work(params))};
                let id=call.action_id();if observing.insert(id){operations.spawn(async move{(id,call.wait().await)});}
            }
        }
    }
    let _ = socket.close(None).await;
}
