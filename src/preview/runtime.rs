//! Owned preview setup and desktop channel lifetime; logical cancellation never drops a CDP launch.
use super::*;
use tokio::sync::watch;
type Key = (String, String);
pub(super) struct Job {
    key: Key,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
pub(super) struct Pending {
    generation: u64,
    cancel: watch::Sender<bool>,
    result: watch::Receiver<Option<Result<(), Value>>>,
}
pub(super) struct DesktopSlot {
    id: u64,
    channel: crate::desktop_browser_channel::Channel,
    stop: watch::Sender<bool>,
    task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}
#[derive(Clone)]
struct Owned {
    inner: Arc<Mutex<Inner>>,
    browser: Arc<crate::server_browser::Browsers>,
    desktop: Arc<Mutex<Option<DesktopSlot>>>,
    jobs: Arc<Mutex<Vec<Job>>>,
    monitors: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    changed: crate::terminal::Notifier,
    root: std::path::PathBuf,
    registry: Arc<crate::agent_scope::Registry>,
}
impl Owned {
    fn new(s: &State) -> Self {
        let m = &s.streams.p2.preview;
        Self {
            inner: m.inner.clone(),
            browser: m.browser.clone(),
            desktop: m.desktop.clone(),
            jobs: m.jobs.clone(),
            monitors: m.monitors.clone(),
            changed: s.streams.changed.clone(),
            root: s.dir.clone(),
            registry: s.agent_scopes.clone(),
        }
    }
}
fn alive(inner: &Inner, key: &Key, generation: u64) -> bool {
    !inner.stopping
        && inner.kernel.get(&key.0, &key.1).is_some()
        && inner
            .pending
            .get(key)
            .is_some_and(|pending| pending.generation == generation)
}
fn prepare(inner: &mut Inner) -> (Pending, watch::Sender<Option<Result<(), Value>>>) {
    inner.next_job += 1;
    let (cancel, _) = watch::channel(false);
    let (sender, result) = watch::channel(None);
    let pending = Pending {
        generation: inner.next_job,
        cancel,
        result,
    };
    (pending, sender)
}
fn reserve(inner: &mut Inner, key: Key) -> (Pending, watch::Sender<Option<Result<(), Value>>>) {
    let (pending, sender) = prepare(inner);
    inner.pending.insert(key, pending.clone());
    (pending, sender)
}
async fn result(mut receiver: watch::Receiver<Option<Result<(), Value>>>) -> Result<(), Value> {
    loop {
        let value = receiver.borrow().clone();
        if let Some(value) = value {
            return value;
        }
        receiver
            .changed()
            .await
            .map_err(|_| invalid("Browser setup ended"))?;
    }
}
struct OpenWait {
    inner: Arc<Mutex<Inner>>,
    changed: crate::terminal::Notifier,
    key: Key,
    generation: u64,
    armed: bool,
    browser: Arc<crate::server_browser::Browsers>,
    jobs: Arc<Mutex<Vec<Job>>>,
}
impl Drop for OpenWait {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let ready = {
            let mut inner = self.inner.lock().unwrap();
            if inner.kernel.get(&self.key.0, &self.key.1).is_none() {
                return;
            }
            let pending = inner.pending.get(&self.key);
            if pending.is_some_and(|pending| pending.generation != self.generation) {
                return;
            }
            let ready = pending.is_none();
            if let Some(pending) = pending {
                pending.cancel.send_replace(true);
            }
            inner
                .kernel
                .close(&json!({"threadId":self.key.0,"tabId":self.key.1}), || {
                    iso(now_ms())
                });
            inner.flush();
            ready
        };
        bump(&self.changed);
        if ready {
            let browser = self.browser.clone();
            let key = self.key.clone();
            let mut jobs = self.jobs.lock().unwrap();
            let task_key = key.clone();
            let task = tokio::spawn(async move {
                browser.close(&key.0, Some(&key.1)).await;
            });
            jobs.push(Job {
                key: task_key,
                task,
            });
        }
    }
}

pub(super) async fn open(s: &State, input: &Value) -> Reply {
    let thread = str_field(input, "threadId")?;
    if input["viewport"].is_object() && !viewport_ok(&input["viewport"]) {
        return Err(invalid("viewport is invalid"));
    }
    let m = &s.streams.p2.preview;
    let gate = m.lifecycle.lock().await;
    if crate::events::get(&s.db.lock().unwrap(), "thread", thread)
        .is_ok_and(|thread| !thread["deletedAt"].is_null())
    {
        return Err(invalid("The thread was deleted."));
    }
    let (mut snapshot, pending, send) = {
        let mut inner = m.inner.lock().unwrap();
        if inner.stopping {
            return Err(invalid("Server is shutting down"));
        }
        if inner.kernel.all().len() >= MAX_SESSIONS {
            return Err(invalid("too many open preview sessions"));
        }
        inner.next_tab += 1;
        let base = format!("tab_{}", radix36(inner.next_tab));
        let (pending, send) = if input["runtime"] == "server" {
            let (pending, send) = prepare(&mut inner);
            (Some(pending), Some(send))
        } else {
            (None, None)
        };
        let Inner {
            kernel,
            pending: owners,
            ..
        } = &mut *inner;
        let snapshot = kernel
            .open(
                input,
                &base,
                || iso(now_ms()),
                |snapshot| {
                    if let Some(pending) = &pending {
                        owners.insert(
                            (thread.to_string(), tab_of(snapshot).to_string()),
                            pending.clone(),
                        );
                    }
                },
            )
            .map_err(|error| error.value())?;
        inner.flush();
        (snapshot, pending, send)
    };
    bump(&s.streams.changed);
    drop(gate);
    if let (Some(pending), Some(send)) = (pending, send) {
        let key = (thread.to_string(), tab_of(&snapshot).to_string());
        let mut guard = OpenWait {
            inner: m.inner.clone(),
            changed: s.streams.changed.clone(),
            key: key.clone(),
            generation: pending.generation,
            armed: true,
            browser: m.browser.clone(),
            jobs: m.jobs.clone(),
        };
        spawn(Owned::new(s), snapshot.clone(), pending.clone(), send);
        let outcome = result(pending.result).await;
        guard.armed = false;
        outcome?;
        snapshot = m
            .inner
            .lock()
            .unwrap()
            .kernel
            .get(&key.0, &key.1)
            .cloned()
            .ok_or_else(|| lookup_error(&key.0, &key.1))?;
    }
    Ok(snapshot)
}
pub(super) async fn ensure(s: &State, snapshot: &Value) -> Result<(), Value> {
    let m = &s.streams.p2.preview;
    let key = (
        thread_of(snapshot).to_string(),
        tab_of(snapshot).to_string(),
    );
    if m.browser
        .connection(&key.0, &key.1)
        .await
        .is_some_and(|(_, connected)| connected)
    {
        return Ok(());
    }
    let (pending, send) = {
        let mut inner = m.inner.lock().unwrap();
        if inner.kernel.get(&key.0, &key.1).is_none() {
            return Err(lookup_error(&key.0, &key.1));
        }
        if let Some(pending) = inner.pending.get(&key) {
            (pending.clone(), None)
        } else {
            let (pending, send) = reserve(&mut inner, key);
            (pending, Some(send))
        }
    };
    if let Some(send) = send {
        spawn(Owned::new(s), snapshot.clone(), pending.clone(), send);
    }
    result(pending.result).await
}
fn spawn(
    owned: Owned,
    snapshot: Value,
    pending: Pending,
    sender: watch::Sender<Option<Result<(), Value>>>,
) {
    let jobs = owned.jobs.clone();
    let task_key = (
        thread_of(&snapshot).to_string(),
        tab_of(&snapshot).to_string(),
    );
    let mut jobs_guard = jobs.lock().unwrap();
    jobs_guard.retain(|job| !job.task.is_finished());
    let handle = tokio::spawn(async move {
        let key = (
            thread_of(&snapshot).to_string(),
            tab_of(&snapshot).to_string(),
        );
        let generation = pending.generation;
        let mut cancelled = pending.cancel.subscribe();
        let setup=async{
            if !alive(&owned.inner.lock().unwrap(),&key,generation)||*cancelled.borrow(){return Err(lookup_error(&key.0,&key.1));}
            let channel=owned.desktop.lock().unwrap().as_ref().map(|slot|slot.channel.clone());
            let attached=if snapshot["headless"]!=true{if let Some(channel)=&channel{let tab=crate::desktop_browser::TabKey{thread_id:key.0.clone(),tab_id:key.1.clone()};tokio::select!{_=cancelled.changed()=>false,attached=channel.await_attached(&tab,Duration::from_secs(10))=>attached}}else{false}}else{false};
            if !alive(&owned.inner.lock().unwrap(),&key,generation)||*cancelled.borrow(){return Err(lookup_error(&key.0,&key.1));}
            if attached{owned.browser.open_desktop(channel.as_ref().unwrap(),&key.0,&key.1,snapshot["profileId"].as_str().unwrap_or("default")).await?;}else{
                if channel.is_some(){let mut inner=owned.inner.lock().unwrap();if alive(&inner,&key,generation){let _=inner.kernel.mark_headless(&json!({"threadId":key.0,"tabId":key.1}),||iso(now_ms()));inner.flush();drop(inner);bump(&owned.changed);}}
                let isolated=if snapshot["automationOwner"].is_string()&&snapshot.get("profileId").is_none(){format!("agent:{}:{}",key.0,key.1)}else{snapshot["profileId"].as_str().unwrap_or("default").to_string()};
                owned.browser.open_owned(&owned.root,&owned.registry,&key.0,&key.1,&isolated,snapshot["navStatus"]["url"].as_str()).await?;
            }
            if snapshot["viewport"].is_object()&&snapshot["viewport"]["_tag"]!="fill"{owned.browser.command(&key.0,&key.1,"Emulation.setDeviceMetricsOverride",json!({"width":snapshot["viewport"]["width"],"height":snapshot["viewport"]["height"],"deviceScaleFactor":1,"mobile":false})).await?;}
            Ok(())
        }.await;
        let outcome = {
            let mut inner = owned.inner.lock().unwrap();
            let valid = alive(&inner, &key, generation) && !*cancelled.borrow();
            let outcome = if valid {
                setup
            } else {
                Err(lookup_error(&key.0, &key.1))
            };
            if inner
                .pending
                .get(&key)
                .is_some_and(|pending| pending.generation == generation)
            {
                inner.pending.remove(&key);
                if outcome.is_err() {
                    inner
                        .kernel
                        .close(&json!({"threadId":key.0,"tabId":key.1}), || iso(now_ms()));
                    inner.flush();
                }
            }
            outcome
        };
        if outcome.is_err() {
            owned.browser.close(&key.0, Some(&key.1)).await;
        }
        bump(&owned.changed);
        let _ = sender.send(Some(outcome.clone()));
        if outcome.is_ok() {
            if let Some(mut done) = owned.browser.completion(&key.0, &key.1).await {
                let inner = owned.inner.clone();
                let browser = Arc::downgrade(&owned.browser);
                let changed = owned.changed.clone();
                let monitor =
                    tokio::spawn(async move {
                        while !*done.borrow() {
                            if done.changed().await.is_err() {
                                break;
                            }
                        }
                        if let Some(browser) = browser.upgrade() {
                            if browser.connection(&key.0, &key.1).await.is_some_and(
                                |(origin, _)| origin == crate::server_browser::Origin::Desktop,
                            ) {
                                return;
                            }
                        }
                        let mut inner = inner.lock().unwrap();
                        inner
                            .kernel
                            .close(&json!({"threadId":key.0,"tabId":key.1}), || iso(now_ms()));
                        inner.flush();
                        drop(inner);
                        bump(&changed);
                    });
                owned.monitors.lock().unwrap().push(monitor);
            }
        }
    });
    jobs_guard.push(Job {
        key: task_key,
        task: handle,
    });
}
pub(super) async fn close(s: &State, thread: &str, tab: Option<&str>) {
    let m = &s.streams.p2.preview;
    let ready = {
        let mut inner = m.inner.lock().unwrap();
        let targets = if let Some(tab) = tab.filter(|tab| !tab.is_empty()) {
            inner
                .kernel
                .get(thread, tab)
                .cloned()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            inner
                .kernel
                .all()
                .into_iter()
                .filter(|snapshot| thread_of(snapshot) == thread)
                .collect::<Vec<_>>()
        };
        let mut ready = Vec::new();
        for snapshot in targets {
            let key = (
                thread_of(&snapshot).to_string(),
                tab_of(&snapshot).to_string(),
            );
            if let Some(pending) = inner.pending.get(&key) {
                pending.cancel.send_replace(true);
            } else {
                ready.push(key);
            }
        }
        inner
            .kernel
            .close(&json!({"threadId":thread,"tabId":tab}), || iso(now_ms()));
        inner.flush();
        ready
    };
    bump(&s.streams.changed);
    for (thread, tab) in ready {
        m.browser.close(&thread, Some(&tab)).await;
    }
}

async fn drain(m: &Manager, thread: &str, tab: Option<&str>) {
    let jobs = {
        let mut jobs = m.jobs.lock().unwrap();
        let mut selected = Vec::new();
        let mut i = 0;
        while i < jobs.len() {
            if jobs[i].key.0 == thread && tab.is_none_or(|tab| jobs[i].key.1 == tab) {
                selected.push(jobs.swap_remove(i));
            } else {
                i += 1;
            }
        }
        selected
    };
    for job in jobs {
        let _ = job.task.await;
    }
}
pub(super) async fn cleanup_thread(s: &State, thread: &str) {
    let pending = s
        .streams
        .p2
        .preview
        .inner
        .lock()
        .unwrap()
        .pending
        .iter()
        .filter(|((tid, _), _)| tid == thread)
        .map(|(_, pending)| pending.result.clone())
        .collect::<Vec<_>>();
    close(s, thread, None).await;
    drain(&s.streams.p2.preview, thread, None).await;
    for pending in pending {
        let _ = result(pending).await;
    }
}
pub(super) async fn close_settled(s: &State, thread: &str, tab: &str) {
    let pending = s
        .streams
        .p2
        .preview
        .inner
        .lock()
        .unwrap()
        .pending
        .get(&(thread.to_string(), tab.to_string()))
        .map(|pending| pending.result.clone());
    close(s, thread, Some(tab)).await;
    drain(&s.streams.p2.preview, thread, Some(tab)).await;
    if let Some(pending) = pending {
        let _ = result(pending).await;
    }
}
pub(super) async fn shutdown(m: &Manager) {
    let desktop = m.desktop.lock().unwrap().take();
    if let Some(desktop) = &desktop {
        desktop.stop.send_replace(true);
    }
    {
        let mut inner = m.inner.lock().unwrap();
        inner.stopping = true;
        for pending in inner.pending.values() {
            pending.cancel.send_replace(true);
        }
    }
    let jobs = std::mem::take(&mut *m.jobs.lock().unwrap());
    for job in jobs {
        let _ = job.task.await;
    }
    if let Some(desktop) = desktop {
        if let Some(task) = desktop.task.lock().await.take() {
            let _ = task.await;
        }
    }
    m.browser.shutdown().await;
    let monitors = std::mem::take(&mut *m.monitors.lock().unwrap());
    for monitor in monitors {
        let _ = monitor.await;
    }
}

pub struct DesktopDriver {
    stop: watch::Sender<bool>,
    task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    slot: Arc<Mutex<Option<DesktopSlot>>>,
    id: u64,
}
impl Drop for DesktopDriver {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        let mut slot = self.slot.lock().unwrap();
        if slot.as_ref().is_some_and(|slot| slot.id == self.id) {
            slot.take();
        }
    }
}
impl DesktopDriver {
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}
pub async fn install_desktop(
    state: Arc<State>,
    channel: crate::desktop_browser_channel::Channel,
) -> DesktopDriver {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let m = &state.streams.p2.preview;
    let (stop, mut stopped) = watch::channel(false);
    let task_slot = Arc::new(tokio::sync::Mutex::new(None));
    if let Some(previous) = m.desktop.lock().unwrap().take() {
        previous.stop.send_replace(true);
    }
    *m.desktop.lock().unwrap() = Some(DesktopSlot {
        id,
        channel: channel.clone(),
        stop: stop.clone(),
        task: task_slot.clone(),
    });
    let slot = m.desktop.clone();
    let weak = Arc::downgrade(&state);
    let mut changes = channel.subscribe();
    let mut turns = m.activity.subscribe();
    let mut turn_stopped = stopped.clone();
    let turn_channel = channel.clone();
    let task = tokio::spawn(async move {
        let attachments = async {
            loop {
                let change =
                    tokio::select! {_=stopped.changed()=>break,change=changes.next()=>change};
                let Some(change) = change else {
                    break;
                };
                if !change.attached {
                    continue;
                }
                let Some(state) = weak.upgrade() else {
                    break;
                };
                let snapshot = state
                    .streams
                    .p2
                    .preview
                    .inner
                    .lock()
                    .unwrap()
                    .kernel
                    .get(&change.key.thread_id, &change.key.tab_id)
                    .cloned();
                if let Some(snapshot) = snapshot.filter(|snapshot| snapshot["runtime"] == "server")
                {
                    let _ = ensure(&state, &snapshot).await;
                }
            }
        };
        let activity = async {
            loop {
                let event =
                    tokio::select! {_=turn_stopped.changed()=>break,event=turns.recv()=>event};
                let Some((thread, active)) = event else {
                    break;
                };
                tokio::select! {_=turn_stopped.changed()=>break,_=turn_channel.turn(&thread,active)=>{}}
            }
        };
        tokio::join!(attachments, activity);
    });
    *task_slot.lock().await = Some(task);
    DesktopDriver {
        stop,
        task: task_slot,
        slot,
        id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    struct Peer {
        channel: crate::desktop_browser_channel::Channel,
        events: tokio::sync::mpsc::UnboundedSender<Value>,
        messages: Arc<Mutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }
    fn state() -> Arc<State> {
        crate::test_sandbox::assert_isolated();
        let state = Arc::new(
            State::open(
                &std::env::temp_dir().join(format!("preview-coordinator-{}", random_id().unwrap())),
            )
            .unwrap(),
        );
        *state
            .streams
            .p2
            .preview
            .browser
            .fixture_binary
            .lock()
            .unwrap() = Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/browser-cdp.py"),
        );
        state
    }
    fn peer() -> Peer {
        let (mut launcher, input) = tokio::io::duplex(65536);
        let (control, commands) = tokio::io::duplex(65536);
        let channel = crate::desktop_browser_channel::Channel::start(input, control);
        let (events, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let observed = messages.clone();
        let task = tokio::spawn(async move {
            let mut commands = BufReader::new(commands);
            loop {
                let mut line = String::new();
                let event = tokio::select! {event=events_rx.recv()=>{let Some(event)=event else{break;};event},read=commands.read_line(&mut line)=>{if !read.is_ok_and(|n|n>0){break;}let command:Value=serde_json::from_str(&line).unwrap();observed.lock().unwrap().push(command.clone());if command["type"]!="cdp"{continue;}let packet:Value=serde_json::from_str(command["message"].as_str().unwrap()).unwrap();let result=match packet["method"].as_str().unwrap(){"Target.getTargets"=>json!({"targetInfos":[{"type":"page","targetId":"page"}]}),"Target.attachToTarget"=>json!({"sessionId":"opaque"}),_=>json!({})};json!({"type":"cdp","threadId":command["threadId"],"tabId":command["tabId"],"message":json!({"id":packet["id"],"result":result}).to_string()})}};
                if launcher
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Peer {
            channel,
            events,
            messages,
            task,
        }
    }
    async fn wait(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn published(state: &State) -> Value {
        wait(|| {
            !state
                .streams
                .p2
                .preview
                .inner
                .lock()
                .unwrap()
                .kernel
                .all()
                .is_empty()
        })
        .await;
        state.streams.p2.preview.inner.lock().unwrap().kernel.all()[0].clone()
    }
    fn opening(state: Arc<State>) -> tokio::task::JoinHandle<Reply> {
        tokio::spawn(async move {
            call(
                &state,
                "preview.open",
                &json!({"threadId":"thread","runtime":"server"}),
            )
            .await
        })
    }
    #[tokio::test]
    async fn published_pending_open_closes_without_waiting_for_desktop_or_resurrecting() {
        let state = state();
        let peer = peer();
        let driver = install_desktop(state.clone(), peer.channel.clone()).await;
        let mut cursor = open_events(&state);
        let open = opening(state.clone());
        let snapshot = published(&state).await;
        assert!(tab_of(&snapshot).ends_with(
            state
                .streams
                .p2
                .preview
                .inner
                .lock()
                .unwrap()
                .kernel
                .epoch()
        ));
        let Step::Values(events) = cursor.next(&state) else {
            panic!("opened must publish before attachment wait");
        };
        assert_eq!(events[0]["type"], "opened");
        assert_eq!(events[0]["revision"], 1);
        assert_eq!(
            state.streams.p2.preview.inner.lock().unwrap().pending.len(),
            1
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            call(
                &state,
                "preview.close",
                &json!({"threadId":"thread","tabId":tab_of(&snapshot)}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), open)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        peer.events
            .send(json!({"type":"attached","threadId":"thread","tabId":tab_of(&snapshot)}))
            .unwrap();
        tokio::task::yield_now().await;
        assert!(state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .all()
            .is_empty());
        assert!(!state.dir.join("browser-profiles").exists());
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .all(|message| message["type"] != "cdp"));
        let next = opening(state.clone());
        let next_snapshot = published(&state).await;
        tokio::time::timeout(
            Duration::from_secs(1),
            super::super::cleanup(&state, "thread"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(next.await.unwrap().is_err());
        assert!(state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .get("thread", tab_of(&next_snapshot))
            .is_none());
        shutdown(&state.streams.p2.preview).await;
        driver.shutdown().await;
        peer.task.abort();
    }
    #[tokio::test]
    async fn aborted_caller_fences_owned_local_target_creation_and_thread_cleanup_reaps_it() {
        let state = state();
        use std::os::unix::fs::PermissionsExt;
        let slow = state.dir.join("slow-cdp.py");
        let source=include_str!("../../tests/fixtures/browser-cdp.py").replace("import json,os,sys","import json,os,sys,time").replace("        os.write(4,","        if v['method']=='Target.createTarget':time.sleep(0.2)\n        os.write(4,");
        std::fs::write(&slow, source).unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o700)).unwrap();
        *state
            .streams
            .p2
            .preview
            .browser
            .fixture_binary
            .lock()
            .unwrap() = Some(slow);
        let open = opening(state.clone());
        let snapshot = published(&state).await;
        let base = state.dir.join("browser-profiles");
        wait(|| {
            std::fs::read_dir(&base).ok().is_some_and(|mut dirs| {
                dirs.any(|entry| {
                    entry.is_ok_and(|entry| entry.path().join("commands.jsonl").exists())
                })
            })
        })
        .await;
        open.abort();
        assert!(open.await.unwrap_err().is_cancelled());
        assert!(state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .get("thread", tab_of(&snapshot))
            .is_none());
        super::super::cleanup(&state, "thread").await.unwrap();
        let profile = std::fs::read_dir(base)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let pid: i32 = std::fs::read_to_string(profile.join("fixture.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        let commands = std::fs::read_to_string(profile.join("commands.jsonl")).unwrap();
        assert!(commands.contains("Target.closeTarget"));
        assert!(profile.exists(), "fixture tree is retained");
        shutdown(&state.streams.p2.preview).await;
    }
    #[tokio::test]
    async fn desktop_disconnect_retains_kernel_session_and_attach_reuses_it() {
        let state = state();
        let peer = peer();
        let driver = install_desktop(state.clone(), peer.channel.clone()).await;
        assert!(state.streams.p2.preview.desktop_available());
        let open = opening(state.clone());
        let snapshot = published(&state).await;
        let tab = tab_of(&snapshot).to_string();
        peer.events
            .send(json!({"type":"attached","threadId":"thread","tabId":tab}))
            .unwrap();
        let opened = open.await.unwrap().unwrap();
        assert_eq!(opened["tabId"], tab);
        assert_eq!(
            state
                .streams
                .p2
                .preview
                .browser
                .connection("thread", &tab)
                .await,
            Some((crate::server_browser::Origin::Desktop, true))
        );
        let activity = state.streams.p2.preview.activity();
        let mut first = activity.begin("thread");
        let second = activity.begin("thread");
        first.end();
        drop(second);
        wait(|| {
            peer.messages
                .lock()
                .unwrap()
                .iter()
                .filter(|message| message["type"] == "turn")
                .count()
                == 2
        })
        .await;
        let turns = peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message["type"] == "turn")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(turns[0]["active"], true);
        assert_eq!(turns[1]["active"], false);
        state
            .streams
            .p2
            .preview
            .browser
            .command(
                "thread",
                &tab,
                "Input.dispatchMouseEvent",
                json!({"type":"mousePressed","x":12.5,"y":34.0}),
            )
            .await
            .unwrap();
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "pointer"
                && message["phase"] == "click"
                && message["x"] == 12.5));
        super::super::request_reveal(&state, "thread", &tab, false).unwrap();
        let reveal = state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .get("thread", &tab)
            .unwrap()["revealRequest"]
            .clone();
        super::super::request_reveal(&state, "thread", &tab, true).unwrap();
        let next_reveal = state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .get("thread", &tab)
            .unwrap()["revealRequest"]
            .clone();
        assert_ne!(reveal["id"], next_reveal["id"]);
        assert_eq!(next_reveal["force"], true);
        assert_eq!(call(&state,"preview.reportStatus",&json!({"threadId":"thread","tabId":tab,"serverControlled":true,"navStatus":{"_tag":"Idle"}})).await.unwrap_err()["_tag"],"PreviewControlRequiredError");
        peer.events
            .send(json!({"type":"detached","threadId":"thread","tabId":tab}))
            .unwrap();
        let mut done = state
            .streams
            .p2
            .preview
            .browser
            .completion("thread", &tab)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !*done.borrow() {
                done.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(state
            .streams
            .p2
            .preview
            .inner
            .lock()
            .unwrap()
            .kernel
            .get("thread", &tab)
            .is_some());
        peer.events
            .send(json!({"type":"attached","threadId":"thread","tabId":tab}))
            .unwrap();
        wait(|| {
            peer.messages
                .lock()
                .unwrap()
                .iter()
                .filter(|message| {
                    message["type"] == "cdp"
                        && message["message"].as_str().unwrap().contains("Page.enable")
                })
                .count()
                == 2
        })
        .await;
        close(&state, "thread", Some(&tab)).await;
        shutdown(&state.streams.p2.preview).await;
        driver.shutdown().await;
        assert!(!state.streams.p2.preview.desktop_available());
        assert!(!state.dir.join("browser-profiles").exists());
        peer.task.abort();
    }
    #[tokio::test]
    async fn late_desktop_attachment_does_not_adopt_a_sticky_headless_fallback() {
        let state = state();
        let peer = peer();
        let driver = install_desktop(state.clone(), peer.channel.clone()).await;
        let opened = tokio::time::timeout(
            Duration::from_secs(13),
            call(
                &state,
                "preview.open",
                &json!({"threadId":"thread","runtime":"server"}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(opened["headless"], true);
        let tab = tab_of(&opened);
        super::super::request_reveal(&state, "thread", tab, true).unwrap();
        assert_eq!(
            state
                .streams
                .p2
                .preview
                .inner
                .lock()
                .unwrap()
                .kernel
                .get("thread", tab)
                .unwrap()["headless"],
            true
        );
        peer.events
            .send(json!({"type":"attached","threadId":"thread","tabId":tab}))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            state
                .streams
                .p2
                .preview
                .browser
                .connection("thread", tab)
                .await,
            Some((crate::server_browser::Origin::Local, true))
        );
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .all(|message| message["type"] != "cdp"));
        close(&state, "thread", Some(tab)).await;
        shutdown(&state.streams.p2.preview).await;
        driver.shutdown().await;
        peer.task.abort();
    }
    #[tokio::test]
    async fn protocol2_http_disconnect_and_interrupt_cancel_published_pending_opens() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
        for interrupt in [false, true] {
            let state = state();
            let peer = peer();
            let driver = install_desktop(state.clone(), peer.channel.clone()).await;
            let token = state
                .issue_session(
                    "private-preview-wire",
                    &crate::store::STANDARD_SCOPES.map(str::to_owned),
                    60000,
                    "Private fixture",
                )
                .unwrap();
            let (client, server) = tokio::io::duplex(256 * 1024);
            let serving = state.clone();
            let http = tokio::spawn(async move {
                crate::http::serve_io(server, serving).await.unwrap();
            });
            let mut request = "ws://localhost/ws?orchestrationProtocol=2"
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
            let (mut socket, _) = tokio_tungstenite::client_async(request, client)
                .await
                .unwrap();
            socket.send(Message::Text(json!({"_tag":"Request","id":"events","tag":"subscribePreviewEvents","payload":{}}).to_string().into())).await.unwrap();
            socket
                .send(Message::Text(json!({"_tag":"Ping"}).to_string().into()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(Ok(Message::Text(text))) = socket.next().await {
                        let frame: Value = serde_json::from_str(&text).unwrap();
                        if frame["_tag"] == "Pong" {
                            break;
                        }
                    }
                }
            })
            .await
            .unwrap();
            socket.send(Message::Text(json!({"_tag":"Request","id":"open","tag":"preview.open","payload":{"threadId":"thread","runtime":"server"}}).to_string().into())).await.unwrap();
            let opened = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(Ok(Message::Text(text))) = socket.next().await {
                        let frame: Value = serde_json::from_str(&text).unwrap();
                        if frame["_tag"] == "Chunk" && frame["requestId"] == "events" {
                            break frame["values"][0].clone();
                        }
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(opened["type"], "opened");
            let tab = opened["tabId"].as_str().unwrap().to_string();
            if interrupt {
                socket
                    .send(Message::Text(
                        json!({"_tag":"Interrupt","requestId":"open","interruptors":[123]})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Some(Ok(Message::Text(text))) = socket.next().await {
                            let frame: Value = serde_json::from_str(&text).unwrap();
                            if frame["_tag"] == "Exit" && frame["requestId"] == "open" {
                                assert_eq!(frame["exit"]["cause"][0]["_tag"], "Interrupt");
                                break;
                            }
                        }
                    }
                })
                .await
                .unwrap();
            } else {
                socket.close(None).await.unwrap();
            }
            wait(|| {
                state
                    .streams
                    .p2
                    .preview
                    .inner
                    .lock()
                    .unwrap()
                    .kernel
                    .get("thread", &tab)
                    .is_none()
            })
            .await;
            if interrupt {
                let mut completed = std::collections::BTreeSet::new();
                for index in 0..32 {
                    let id = format!("quick-{index}");
                    socket.send(Message::Text(json!([{"_tag":"Request","id":id,"tag":"preview.open","payload":{"threadId":"thread"}},{"_tag":"Interrupt","requestId":id,"interruptors":[123]}]).to_string().into())).await.unwrap();
                    tokio::time::timeout(Duration::from_secs(2), async {
                        loop {
                            if let Some(Ok(Message::Text(text))) = socket.next().await {
                                let frame: Value = serde_json::from_str(&text).unwrap();
                                if frame["_tag"] == "Exit" {
                                    let key = frame["requestId"].as_str().unwrap();
                                    assert!(
                                        !completed.contains(key),
                                        "one terminal response per request"
                                    );
                                    if key == id {
                                        break;
                                    }
                                }
                            }
                        }
                    })
                    .await
                    .unwrap();
                    completed.insert(id);
                    close(&state, "thread", None).await;
                    wait(|| {
                        state
                            .streams
                            .p2
                            .preview
                            .inner
                            .lock()
                            .unwrap()
                            .kernel
                            .all()
                            .is_empty()
                    })
                    .await;
                }
                socket
                    .send(Message::Text(json!({"_tag":"Ping"}).to_string().into()))
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Some(Ok(Message::Text(text))) = socket.next().await {
                            let frame: Value = serde_json::from_str(&text).unwrap();
                            if frame["_tag"] == "Pong" {
                                break;
                            }
                            assert!(
                                !(frame["_tag"] == "Exit"
                                    && frame["requestId"]
                                        .as_str()
                                        .is_some_and(|id| completed.contains(id))),
                                "no duplicate completion after interruption"
                            );
                        }
                    }
                })
                .await
                .unwrap();
            }
            peer.events
                .send(json!({"type":"attached","threadId":"thread","tabId":tab}))
                .unwrap();
            if interrupt {
                socket.close(None).await.unwrap();
            }
            drop(socket);
            let _ = http.await;
            shutdown(&state.streams.p2.preview).await;
            driver.shutdown().await;
            assert!(!state.dir.join("browser-profiles").exists());
            assert!(peer
                .messages
                .lock()
                .unwrap()
                .iter()
                .all(|message| message["type"] != "cdp"));
            peer.task.abort();
        }
    }
}
