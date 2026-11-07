//! Preview session registry and lazy system-browser runtime.
use crate::{
    p2::{invalid, str_field, Reply},
    store::{iso, now_ms, random_id, State},
    terminal::{bump, Step},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

const RING: usize = 256;
const MAX_SESSIONS: usize = 256;
const BATCH: usize = 32;

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "preview.open"
            | "preview.navigate"
            | "preview.resize"
            | "preview.adjust"
            | "preview.refresh"
            | "preview.close"
            | "preview.list"
            | "preview.reportStatus"
            | "preview.clearProfile"
    )
}

mod navigation;
mod runtime;
pub use runtime::{install_desktop, DesktopDriver};
struct Inner {
    kernel: crate::preview_manager::Kernel,
    next_tab: u64,
    next_job: u64,
    pending: BTreeMap<(String, String), runtime::Pending>,
    ring: VecDeque<(u64, Value)>,
    stopping: bool,
}
impl Default for Inner {
    fn default() -> Self {
        Self {
            kernel: crate::preview_manager::Kernel::new(
                random_id().unwrap_or_else(|_| "epoch".into()),
            ),
            next_tab: 0,
            next_job: 0,
            pending: BTreeMap::new(),
            ring: VecDeque::new(),
            stopping: false,
        }
    }
}
impl Inner {
    fn flush(&mut self) {
        for event in self.kernel.drain_events() {
            self.ring
                .push_back((event["revision"].as_u64().unwrap(), event));
        }
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
    }
}
#[derive(Default)]
pub struct Manager {
    pub(crate) activity: crate::preview_activity::Activity,
    inner: Arc<Mutex<Inner>>,
    lifecycle: tokio::sync::Mutex<()>,
    pub(crate) browser: Arc<crate::server_browser::Browsers>,
    desktop: Arc<Mutex<Option<runtime::DesktopSlot>>>,
    jobs: Arc<Mutex<Vec<runtime::Job>>>,
    monitors: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
impl Manager {
    pub fn activity(&self) -> crate::preview_activity::Activity {
        self.activity.clone()
    }
    pub fn set_profile_removal(&self, removal: Arc<dyn crate::cleanup::Removal>) {
        self.browser.set_profile_removal(removal);
    }
    pub fn desktop_available(&self) -> bool {
        self.desktop.lock().unwrap().is_some()
    }
}

fn lookup_error(thread: &str, tab: &str) -> Value {
    json!({"_tag":"PreviewSessionLookupError","threadId":thread,"tabId":tab,"message":format!("Unknown preview session: thread={thread}, tab={tab}")})
}
fn url_error(input: &str, reason: &str, protocol: Option<&str>) -> Value {
    let length = input.encode_utf16().count();
    let suffix = protocol
        .map(|value| format!(": {value}"))
        .unwrap_or_default();
    let mut v = json!({"_tag":"PreviewInvalidUrlError","inputLength":length,"reason":reason,
        "message":format!("Invalid preview URL ({reason}{suffix}; input length {length}). Use an http(s) URL or a schemeless host such as localhost:5173. With preview_open, omit url when creating a new tab to open a blank tab.")});
    if let Some(p) = protocol {
        v["protocol"] = json!(p);
    }
    v
}

fn url_protocol(raw: &str) -> Option<String> {
    let (scheme, _) = raw.split_once(':')?;
    if scheme.starts_with(|value: char| value.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || "+.-".contains(value))
    {
        Some(format!("{}:", scheme.to_ascii_lowercase()))
    } else {
        None
    }
}

/// T3 `normalizePreviewUrl`: bare hosts become https (http for loopback); only http(s) is previewed.
pub fn normalize_url(raw: &str) -> Result<String, Value> {
    let trimmed = crate::native_goal::trim(raw);
    if trimmed.is_empty() {
        return Err(url_error(raw, "empty", None));
    }
    let protocol = url_protocol(trimmed);
    if protocol.as_deref().is_some_and(|value| {
        matches!(
            value,
            "about:"
                | "blob:"
                | "chrome:"
                | "data:"
                | "file:"
                | "ftp:"
                | "javascript:"
                | "mailto:"
                | "sms:"
                | "ssh:"
                | "tel:"
                | "view-source:"
                | "ws:"
                | "wss:"
        )
    }) {
        return Err(url_error(raw, "unsupported-protocol", protocol.as_deref()));
    }
    let lower = trimmed.to_lowercase();
    let loopback = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]
        .iter()
        .any(|h| {
            lower
                .strip_prefix(h)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
    let has_scheme = trimmed.find("://").is_some_and(|i| {
        let scheme = &trimmed[..i];
        !scheme.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    });
    let candidate = if has_scheme {
        trimmed.to_owned()
    } else {
        format!("{}://{trimmed}", if loopback { "http" } else { "https" })
    };
    let parsed = url::Url::parse(&candidate)
        .map_err(|_| url_error(raw, "parse", url_protocol(&candidate).as_deref()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(url_error(
            raw,
            "unsupported-protocol",
            Some(&format!("{}:", parsed.scheme())),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(url_error(raw, "parse", None));
    }
    Ok(parsed.to_string())
}

fn viewport_ok(v: &Value) -> bool {
    match v["_tag"].as_str() {
        Some("fill") => true,
        Some("freeform" | "preset") => {
            let (w, h) = (v["width"].as_i64(), v["height"].as_i64());
            matches!((w, h), (Some(w), Some(h)) if (240..=3840).contains(&w) && (240..=3840).contains(&h) && w * h <= 3840 * 2160)
        }
        _ => false,
    }
}

const ZOOMS: [f64; 17] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

/// Reconcile CDP's actual history with the client registry (also after viewer navigation).
pub async fn sync_navigation(s: &State, thread: &str, tab: &str) {
    let browser = &s.streams.p2.preview.browser;
    if let Some(watch) = browser.navigation(thread, tab).await {
        navigation::sync(
            browser,
            &s.streams.p2.preview.inner,
            &s.streams.changed,
            thread,
            tab,
            &watch,
        )
        .await;
        return;
    }
    let Ok(history) = browser
        .command(thread, tab, "Page.getNavigationHistory", json!({}))
        .await
    else {
        return;
    };
    let Some(index) = history["currentIndex"].as_u64() else {
        return;
    };
    let Some(entries) = history["entries"].as_array() else {
        return;
    };
    let Some(entry) = entries.get(index as usize) else {
        return;
    };
    let Ok(page) = browser
        .command(
            thread,
            tab,
            "Runtime.evaluate",
            json!({"expression":"document.readyState","returnByValue":true,"timeout":1000}),
        )
        .await
    else {
        return;
    };
    let mut inner = s.streams.p2.preview.inner.lock().unwrap();
    let Some(snapshot) = inner.kernel.get(thread, tab).cloned() else {
        return;
    };
    let nav = json!({"_tag":if page["result"]["value"]=="loading"{"Loading"}else{"Success"},"url":entry["url"],"title":entry["title"]});
    let back = index > 0;
    let forward = (index as usize) + 1 < entries.len();
    if snapshot["navStatus"] == nav
        && snapshot["canGoBack"] == back
        && snapshot["canGoForward"] == forward
    {
        return;
    }
    let _=inner.kernel.report_status(&json!({"threadId":thread,"tabId":tab,"navStatus":nav,"canGoBack":back,"canGoForward":forward,"serverControlled":true}),||iso(now_ms()));
    inner.flush();
    drop(inner);
    bump(&s.streams.changed);
}

pub async fn call(s: &State, tag: &str, p: &Value) -> Reply {
    let thread = if tag == "preview.clearProfile" {
        ""
    } else {
        str_field(p, "threadId")?
    };
    let m = &s.streams.p2.preview;
    if tag == "preview.open" {
        return runtime::open(s, p).await;
    }
    if tag == "preview.clearProfile" {
        let profile = str_field(p, "profileId")?;
        if profile != "incognito" {
            let targets = m
                .inner
                .lock()
                .unwrap()
                .kernel
                .all()
                .into_iter()
                .filter(|v| {
                    v["runtime"] == "server"
                        && v["profileId"].as_str().unwrap_or("default") == profile
                })
                .collect::<Vec<_>>();
            for snapshot in targets {
                runtime::close_settled(s, thread_of(&snapshot), tab_of(&snapshot)).await;
            }
        }
        m.browser.clear(&s.dir, profile).await?;
        return Ok(Value::Null);
    }
    if tag == "preview.close" {
        runtime::close(s, thread, p["tabId"].as_str()).await;
        return Ok(Value::Null);
    }
    if tag == "preview.list" {
        let sessions = m.inner.lock().unwrap().kernel.list(thread);
        for snapshot in sessions["sessions"].as_array().unwrap() {
            if snapshot["runtime"] == "server"
                && m.browser
                    .connection(thread, tab_of(snapshot))
                    .await
                    .is_some_and(|(_, connected)| connected)
            {
                sync_navigation(s, thread, tab_of(snapshot)).await;
            }
        }
        return Ok(m.inner.lock().unwrap().kernel.list(thread));
    }
    let tab = str_field(p, "tabId")?;
    let snapshot = m
        .inner
        .lock()
        .unwrap()
        .kernel
        .get(thread, tab)
        .cloned()
        .ok_or_else(|| lookup_error(thread, tab))?;
    let server_tab = snapshot["runtime"] == "server";
    if tag == "preview.reportStatus" && server_tab {
        return Err(crate::preview_manager::Error::ControlRequired { tab_id: tab.into() }.value());
    }
    if tag == "preview.resize" && !viewport_ok(&p["viewport"]) {
        return Err(invalid("viewport is invalid"));
    }
    if tag == "preview.adjust" {
        validate_adjust(p)?;
    }
    if server_tab
        && matches!(
            tag,
            "preview.navigate" | "preview.refresh" | "preview.resize" | "preview.adjust"
        )
    {
        runtime::ensure(s, &snapshot).await?;
        match tag {
            "preview.navigate" => {
                let navigation = m
                    .browser
                    .command(
                        thread,
                        tab,
                        "Page.navigate",
                        json!({"url":normalize_url(str_field(p,"url")?)?}),
                    )
                    .await?;
                if let Some(message) = navigation["errorText"].as_str().filter(|s| !s.is_empty()) {
                    return Err(json!({"_tag":"ServerBrowserLaunchError","message":message}));
                }
            }
            "preview.refresh" => {
                m.browser
                    .command(thread, tab, "Page.reload", json!({}))
                    .await?;
            }
            "preview.resize" => {
                if !viewport_ok(&p["viewport"]) {
                    return Err(invalid("viewport is invalid"));
                }
                m.browser.command(thread,tab,"Emulation.setDeviceMetricsOverride",json!({"width":p["viewport"]["width"].as_u64().unwrap_or(1280),"height":p["viewport"]["height"].as_u64().unwrap_or(800),"deviceScaleFactor":1,"mobile":false})).await?;
            }
            "preview.adjust" => {
                if let Some(color) = p["colorScheme"].as_str() {
                    if !matches!(color, "system" | "light" | "dark") {
                        return Err(invalid("colorScheme is invalid"));
                    }
                    m.browser.command(thread,tab,"Emulation.setEmulatedMedia",json!({"features":if color=="system" {json!([])} else {json!([{"name":"prefers-color-scheme","value":color}])}})).await?;
                }
                if let Some(zoom) = p["zoomFactor"].as_f64() {
                    if !ZOOMS.iter().any(|z| (*z - zoom).abs() < 1e-9) {
                        return Err(invalid("zoomFactor is invalid"));
                    }
                    m.browser
                        .command(
                            thread,
                            tab,
                            "Emulation.setPageScaleFactor",
                            json!({"pageScaleFactor":zoom}),
                        )
                        .await?;
                }
                match p["clear"].as_str() {
                    Some("cookies") => {
                        m.browser
                            .command(thread, tab, "Network.clearBrowserCookies", json!({}))
                            .await?;
                    }
                    Some("cache") => {
                        m.browser
                            .command(thread, tab, "Network.clearBrowserCache", json!({}))
                            .await?;
                    }
                    _ => {}
                }
                if p["hardReload"] == true {
                    m.browser
                        .command(thread, tab, "Page.reload", json!({"ignoreCache":true}))
                        .await?;
                }
            }
            _ => {}
        }
    }
    let mut inner = m.inner.lock().unwrap();
    let result = match tag {
        "preview.refresh" => Ok(Value::Null),
        "preview.navigate" => {
            let nav = json!({"_tag":"Success","url":normalize_url(str_field(p,"url")?)?,"title":p["resolvedTitle"].as_str().unwrap_or(snapshot["navStatus"]["title"].as_str().unwrap_or(""))});
            inner.kernel.report_status(&json!({"threadId":thread,"tabId":tab,"navStatus":nav,"canGoBack":snapshot["canGoBack"],"canGoForward":snapshot["canGoForward"],"serverControlled":server_tab}),||iso(now_ms())).map_err(|error|error.value())?;
            Ok(inner
                .kernel
                .get(thread, tab)
                .cloned()
                .ok_or_else(|| lookup_error(thread, tab))?)
        }
        "preview.resize" => inner
            .kernel
            .resize(p, || iso(now_ms()))
            .map_err(|error| error.value()),
        "preview.adjust" => inner
            .kernel
            .adjust(p, || iso(now_ms()))
            .map_err(|error| error.value()),
        "preview.reportStatus" => {
            if !matches!(
                p["navStatus"]["_tag"].as_str(),
                Some("Idle" | "Loading" | "Success" | "LoadFailed")
            ) {
                return Err(invalid("navStatus is invalid"));
            }
            inner
                .kernel
                .report_status(p, || iso(now_ms()))
                .map(|()| Value::Null)
                .map_err(|error| error.value())
        }
        _ => Err(invalid("Unknown preview operation")),
    };
    inner.flush();
    drop(inner);
    bump(&s.streams.changed);
    result
}
/// Publish a native reveal request without changing sticky headless ownership.
pub fn request_reveal(s: &State, thread: &str, tab: &str, force: bool) -> Result<(), Value> {
    let mut inner = s.streams.p2.preview.inner.lock().unwrap();
    if inner.kernel.get(thread, tab).is_none() {
        return Err(lookup_error(thread, tab));
    }
    let uuid = random_id().map_err(invalid)?;
    inner
        .kernel
        .request_reveal(
            &json!({"threadId":thread,"tabId":tab,"force":force}),
            || iso(now_ms()),
            || uuid,
        )
        .map_err(|error| error.value())?;
    inner.flush();
    drop(inner);
    bump(&s.streams.changed);
    Ok(())
}
fn thread_of(snapshot: &Value) -> &str {
    snapshot["threadId"].as_str().unwrap_or("")
}
fn tab_of(snapshot: &Value) -> &str {
    snapshot["tabId"].as_str().unwrap_or("")
}
fn validate_adjust(p: &Value) -> Result<(), Value> {
    if p["colorScheme"]
        .as_str()
        .is_some_and(|color| !matches!(color, "system" | "light" | "dark"))
    {
        return Err(invalid("colorScheme is invalid"));
    }
    if p["zoomFactor"]
        .as_f64()
        .is_some_and(|zoom| !ZOOMS.iter().any(|value| (*value - zoom).abs() < 1e-9))
    {
        return Err(invalid("zoomFactor is invalid"));
    }
    Ok(())
}

fn radix36(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(std::char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
    }
    out.iter().rev().collect()
}

// ---- event stream --------------------------------------------------------------------------

pub struct Cursor {
    seen: u64,
    waiting: bool,
}

pub fn open_events(s: &State) -> Cursor {
    Cursor {
        seen: s.streams.p2.preview.inner.lock().unwrap().kernel.revision(),
        waiting: false,
    }
}

impl Cursor {
    pub fn next(&mut self, s: &State) -> Step {
        if self.waiting {
            return Step::Idle;
        }
        let g = s.streams.p2.preview.inner.lock().unwrap();
        let batch: Vec<Value> = g
            .ring
            .iter()
            .filter(|(r, _)| *r > self.seen)
            .take(BATCH)
            .map(|(_, e)| e.clone())
            .collect();
        let Some(last) = batch.last().and_then(|e| e["revision"].as_u64()) else {
            return Step::Idle;
        };
        self.seen = last;
        self.waiting = true;
        Step::Values(batch)
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
}

// ---- discovered local servers --------------------------------------------------------------

type Shared = Arc<Mutex<Option<(u64, Value)>>>;

/// Polls `/proc/net/tcp*` every three seconds for as long as a subscriber holds it.
pub struct Servers {
    shared: Shared,
    seen: u64,
    waiting: bool,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Servers {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Servers {
    pub fn new(configured: &Value) -> Self {
        let shared: Shared = Arc::default();
        let urls: Vec<String> = configured
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .take(32)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let probing = !urls.is_empty();
        let task = tokio::spawn(scan_loop(shared.clone(), urls, probing));
        Self {
            shared,
            seen: 0,
            waiting: false,
            task,
        }
    }
    pub fn next(&mut self, _s: &State) -> Step {
        if self.waiting {
            return Step::Idle;
        }
        let g = self.shared.lock().unwrap();
        match g.as_ref() {
            Some((version, value)) if *version != self.seen => {
                self.seen = *version;
                self.waiting = true;
                Step::Values(vec![value.clone()])
            }
            _ => Step::Idle,
        }
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
}

async fn scan_loop(shared: Shared, urls: Vec<String>, probing: bool) {
    let mut last: Option<Vec<Value>> = None;
    let mut version = 0;
    let mut cache = ScanCache::default();
    loop {
        let urls2 = urls.clone();
        let (servers, kept) = tokio::task::spawn_blocking(move || {
            let servers = scan_servers(&urls2, &mut cache, std::time::Instant::now());
            (servers, cache)
        })
        .await
        .unwrap_or_default();
        cache = kept;
        if last.as_ref() != Some(&servers) {
            version += 1;
            let mut list = json!({"servers":servers,"scannedAt":iso(now_ms())});
            if probing {
                list["configuredUrlProbing"] = json!(true);
            }
            *shared.lock().unwrap() = Some((version, list));
            last = Some(servers);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn listening() -> Vec<(u16, String)> {
    let mut out = Vec::new();
    for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" {
                continue;
            }
            let Some((addr, port)) = f[1].rsplit_once(':') else {
                continue;
            };
            // Only sockets a local browser could reach: loopback or any address.
            let local = addr.chars().all(|c| c == '0')
                || addr == "0100007F"
                || addr == "00000000000000000000000001000000";
            let Ok(port) = u16::from_str_radix(port, 16) else {
                continue;
            };
            if local && port >= 1024 {
                out.push((port, f[9].to_owned()));
            }
        }
    }
    out.sort();
    out.dedup_by_key(|(p, _)| *p);
    out
}

/// inode -> (pid, comm) for the sockets in `want`, walking at most 50 000 descriptors.
fn owners(want: &[String]) -> BTreeMap<String, (u32, String)> {
    let mut found = BTreeMap::new();
    let mut budget = 50_000;
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in procs.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if budget == 0 {
                return found;
            }
            budget -= 1;
            let Ok(link) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(inode) = link
                .to_str()
                .and_then(|l| l.strip_prefix("socket:["))
                .and_then(|l| l.strip_suffix(']'))
            else {
                continue;
            };
            if want.iter().any(|w| w == inode) && !found.contains_key(inode) {
                let name = std::fs::read_to_string(entry.path().join("comm"))
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                found.insert(inode.to_owned(), (pid, name));
            }
        }
        if found.len() == want.len() {
            break;
        }
    }
    found
}

fn probe(port: u16) -> bool {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(400)));
    if s.write_all(b"HEAD / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 5];
    s.read_exact(&mut buf).is_ok() && &buf == b"HTTP/"
}

/// What a scan learned about one listening socket. A socket that answered HTTP keeps its
/// answer while it listens; ownership is looked up once, when the socket first appears.
struct Known {
    http: bool,
    owner: Option<(u32, String)>,
    first: std::time::Instant,
    probed: std::time::Instant,
}
/// Per-subscription scan state, so a steady machine costs one `/proc/net/tcp` read per scan
/// instead of a walk of every process's descriptors and an HTTP probe of every port.
#[derive(Default)]
struct ScanCache(std::collections::HashMap<(u16, String), Known>);
const RETRY_WINDOW: Duration = Duration::from_secs(60);
/// A socket that does not answer yet is retried on every scan for its first minute (a dev
/// server may bind before it serves), then once a minute.
fn needs_probe(k: &Known, now: std::time::Instant) -> bool {
    !k.http
        && (now.duration_since(k.first) < RETRY_WINDOW
            || now.duration_since(k.probed) >= RETRY_WINDOW)
}

fn scan_servers(
    configured: &[String],
    cache: &mut ScanCache,
    now: std::time::Instant,
) -> Vec<Value> {
    let listeners: Vec<(u16, String)> = listening().into_iter().take(64).collect();
    cache
        .0
        .retain(|key, _| key.1.starts_with("configured:") || listeners.contains(key));
    let fresh: Vec<String> = listeners
        .iter()
        .filter(|key| !cache.0.contains_key(*key))
        .map(|(_, inode)| inode.clone())
        .collect();
    let owner = if fresh.is_empty() {
        BTreeMap::new()
    } else {
        owners(&fresh)
    };
    let mut out = Vec::new();
    for (port, inode) in listeners {
        let key = (port, inode.clone());
        let known = cache.0.entry(key).or_insert_with(|| Known {
            http: false,
            owner: owner.get(&inode).cloned(),
            first: now,
            probed: now,
        });
        if needs_probe(known, now) {
            known.http = probe(port);
            known.probed = now;
        }
        if !known.http {
            continue;
        }
        let (pid, name) = known
            .owner
            .clone()
            .map(|(p, n)| (Some(p), Some(n)))
            .unwrap_or((None, None));
        out.push(json!({"host":"localhost","port":port,"url":format!("http://localhost:{port}"),"processName":name.filter(|n|!n.is_empty()),"pid":pid,"terminal":null}));
    }
    // Configured URLs are probed only on loopback hosts.
    for raw in configured {
        let Ok(u) = url::Url::parse(raw) else {
            continue;
        };
        let Some(host) = u.host_str() else { continue };
        if !matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
            continue;
        }
        let Some(port) = u.port_or_known_default() else {
            continue;
        };
        if out.iter().any(|s| s["port"] == port) {
            continue;
        }
        let known = cache
            .0
            .entry((port, format!("configured:{raw}")))
            .or_insert_with(|| Known {
                http: false,
                owner: None,
                first: now,
                probed: now,
            });
        // A configured URL can stop answering without its socket changing: always re-probe.
        known.http = probe(port);
        known.probed = now;
        if !known.http {
            continue;
        }
        out.push(json!({"host":host,"port":port,"url":raw,"processName":null,"pid":null,"terminal":null}));
    }
    out.sort_by_key(|s| s["port"].as_u64());
    out
}

pub(crate) async fn cleanup(s: &State, thread: &str) -> Result<(), String> {
    let _gate = s.streams.p2.preview.lifecycle.lock().await;
    runtime::cleanup_thread(s, thread).await;
    Ok(())
}
pub async fn shutdown(s: &State) {
    runtime::shutdown(&s.streams.p2.preview).await;
}

#[cfg(test)]
mod scan_cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    /// A loopback listener that counts accepted probes; answers HTTP only when asked to.
    fn listener(http: bool) -> (u16, Arc<AtomicUsize>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                use std::io::{Read, Write};
                counter.fetch_add(1, Ordering::SeqCst);
                let mut b = [0u8; 256];
                let _ = s.read(&mut b);
                if http {
                    let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\n");
                }
            }
        });
        (port, hits)
    }
    #[test]
    fn steady_sockets_are_not_reprobed_and_silent_ones_back_off() {
        let (silent, silent_hits) = listener(false);
        let (web, web_hits) = listener(true);
        let mut cache = ScanCache::default();
        let t0 = std::time::Instant::now();
        let ports = |v: &[Value]| {
            v.iter()
                .filter_map(|s| s["port"].as_u64())
                .collect::<Vec<_>>()
        };
        let first = scan_servers(&[], &mut cache, t0);
        assert!(ports(&first).contains(&(web as u64)) && !ports(&first).contains(&(silent as u64)));
        assert_eq!(
            (
                silent_hits.load(Ordering::SeqCst),
                web_hits.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
        // Within the retry window a silent socket is retried; an HTTP one is remembered.
        let again = scan_servers(&[], &mut cache, t0 + Duration::from_secs(3));
        assert!(ports(&again).contains(&(web as u64)));
        assert_eq!(
            (
                silent_hits.load(Ordering::SeqCst),
                web_hits.load(Ordering::SeqCst)
            ),
            (2, 1)
        );
        // After the window: no probe until a minute since the last one.
        scan_servers(&[], &mut cache, t0 + Duration::from_secs(61));
        assert_eq!(silent_hits.load(Ordering::SeqCst), 2);
        scan_servers(&[], &mut cache, t0 + Duration::from_secs(64));
        assert_eq!(silent_hits.load(Ordering::SeqCst), 3);
        assert_eq!(web_hits.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod url_oracle_tests {
    use super::*;

    #[test]
    fn native_url_outputs_typed_errors_and_public_schema_budget() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/preview-url-4288.json")).unwrap();
        let mut outside_public_budget = 0;
        for case in fixture["urlCases"].as_array().unwrap() {
            let input = case["input"].as_str().unwrap();
            if case["openSchemaAccepted"] == false && case["expected"]["url"].is_string() {
                // Native public inputs reject this oversized value before URL normalization.
                // Rust IDNA also refuses its oversized Unicode label; do not claim raw-helper parity.
                assert!(input.encode_utf16().count() > 2048);
                assert!(normalize_url(input).is_err());
                outside_public_budget += 1;
            } else {
                match normalize_url(input) {
                    Ok(url) => assert_eq!(json!({"url":url}), case["expected"], "{input:?}"),
                    Err(mut error) => {
                        // The manager wraps the same shared helper in its public error class.
                        error["_tag"] = json!("PreviewUrlNormalizationError");
                        assert_eq!(json!({"error":error}), case["expected"], "{input:?}");
                    }
                }
            }
        }
        assert_eq!(outside_public_budget, 1);
    }
}
