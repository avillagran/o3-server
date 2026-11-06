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

#[derive(Default)]
struct Inner {
    epoch: String,
    next_tab: u64,
    revision: u64,
    sessions: BTreeMap<(String, String), Value>,
    ring: VecDeque<(u64, Value)>,
}

#[derive(Default)]
pub struct Manager {
    inner: Arc<Mutex<Inner>>,
    lifecycle: tokio::sync::Mutex<()>,
    pub(crate) browser: crate::server_browser::Browsers,
}

impl Inner {
    fn epoch(&mut self) -> String {
        if self.epoch.is_empty() {
            self.epoch = random_id().unwrap_or_else(|_| "epoch".into());
        }
        self.epoch.clone()
    }
    fn publish(&mut self, mut event: Value) {
        self.revision += 1;
        event["serverEpoch"] = json!(self.epoch());
        event["revision"] = json!(self.revision);
        self.ring.push_back((self.revision, event));
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
    }
}

fn lookup_error(thread: &str, tab: &str) -> Value {
    json!({"_tag":"PreviewSessionLookupError","threadId":thread,"tabId":tab,"message":format!("Unknown preview session: thread={thread}, tab={tab}")})
}
fn url_error(input: &str, reason: &str, protocol: Option<&str>) -> Value {
    let mut v = json!({"_tag":"PreviewInvalidUrlError","inputLength":input.chars().count(),"reason":reason,
        "message":format!("Invalid preview URL ({reason}; input length {}).",input.chars().count())});
    if let Some(p) = protocol {
        v["protocol"] = json!(p);
    }
    v
}

/// T3 `normalizePreviewUrl`: bare hosts become https (http for loopback); only http(s) is previewed.
pub fn normalize_url(raw: &str) -> Result<String, Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(url_error(raw, "empty", None));
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
    let parsed = url::Url::parse(&candidate).map_err(|_| url_error(raw, "parse", None))?;
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
    let key = (thread.into(), tab.into());
    let Some(mut snapshot) = inner.sessions.get(&key).cloned() else {
        return;
    };
    let nav = json!({"_tag":if page["result"]["value"]=="loading" {"Loading"} else {"Success"},"url":entry["url"],"title":entry["title"]});
    let back = index > 0;
    let forward = (index as usize) + 1 < entries.len();
    if snapshot["navStatus"] == nav
        && snapshot["canGoBack"] == back
        && snapshot["canGoForward"] == forward
    {
        return;
    }
    snapshot["navStatus"] = nav;
    snapshot["canGoBack"] = json!(back);
    snapshot["canGoForward"] = json!(forward);
    snapshot["updatedAt"] = json!(iso(now_ms()));
    inner.sessions.insert(key, snapshot.clone());
    inner.publish(json!({"type":"navigated","threadId":thread,"tabId":tab,"createdAt":iso(now_ms()),"snapshot":snapshot}));
}

pub async fn call(s: &State, tag: &str, p: &Value) -> Reply {
    let _gate = if tag == "preview.open" {
        Some(s.streams.p2.preview.lifecycle.lock().await)
    } else {
        None
    };
    if tag == "preview.open"
        && crate::events::get(
            &s.db.lock().unwrap(),
            "thread",
            p["threadId"].as_str().unwrap_or(""),
        )
        .is_ok_and(|t| !t["deletedAt"].is_null())
    {
        return Err(invalid("The thread was deleted."));
    }
    if tag == "preview.clearProfile" {
        let profile = str_field(p, "profileId")?;
        s.streams.p2.preview.browser.clear(&s.dir, profile).await?;
        if profile != "incognito" {
            let mut g = s.streams.p2.preview.inner.lock().unwrap();
            let keys: Vec<_> = g
                .sessions
                .iter()
                .filter(|(_, v)| {
                    v["runtime"] == "server"
                        && v["profileId"].as_str().unwrap_or("default") == profile
                })
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys {
                g.sessions.remove(&key);
                g.publish(json!({"type":"closed","threadId":key.0,"tabId":key.1,"createdAt":iso(now_ms())}));
            }
        }

        return Ok(Value::Null);
    }
    let m = &s.streams.p2.preview;
    if tag == "preview.list" {
        let keys: Vec<_> = m
            .inner
            .lock()
            .unwrap()
            .sessions
            .iter()
            .filter(|((thread, _), v)| p["threadId"] == *thread && v["runtime"] == "server")
            .map(|(key, _)| key.clone())
            .collect();
        for (thread, tab) in keys {
            sync_navigation(s, &thread, &tab).await;
        }
    }

    if tag == "preview.close" {
        m.browser
            .close(str_field(p, "threadId")?, p["tabId"].as_str())
            .await;
    }
    let server_tab = {
        let g = m.inner.lock().unwrap();
        g.sessions
            .get(&(
                p["threadId"].as_str().unwrap_or("").into(),
                p["tabId"].as_str().unwrap_or("").into(),
            ))
            .is_some_and(|v| v["runtime"] == "server")
    };
    if server_tab {
        let thread = str_field(p, "threadId")?;
        let tab = str_field(p, "tabId")?;
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
    // Reserve the ID before launching without holding the synchronous registry lock.
    let server_id = if tag == "preview.open" && p["runtime"] == "server" {
        if p["viewport"].is_object() && !viewport_ok(&p["viewport"]) {
            return Err(invalid("viewport is invalid"));
        }
        let url = p["url"].as_str().map(normalize_url).transpose()?;
        let tab = {
            let mut g = m.inner.lock().unwrap();
            g.next_tab += 1;
            format!("tab_{}", radix36(g.next_tab))
        };
        m.browser
            .open(
                s,
                str_field(p, "threadId")?,
                &tab,
                p["profileId"].as_str().unwrap_or("default"),
                url.as_deref(),
            )
            .await?;
        if p["viewport"].is_object() && p["viewport"]["_tag"] != "fill" {
            if let Err(e) = m.browser.command(str_field(p,"threadId")?, &tab,
                "Emulation.setDeviceMetricsOverride",
                json!({"width":p["viewport"]["width"],"height":p["viewport"]["height"],"deviceScaleFactor":1,"mobile":false})).await {
                m.browser.close(str_field(p,"threadId")?,Some(&tab)).await;
                return Err(e);
            }
        }
        Some(tab)
    } else {
        None
    };
    let completion = if let Some(tab) = &server_id {
        m.browser.completion(str_field(p, "threadId")?, tab).await
    } else {
        None
    };
    let mut g = m.inner.lock().unwrap();
    let thread = str_field(p, "threadId")?.to_owned();
    let at = iso(now_ms());
    let result = match tag {
        "preview.list" => {
            let mut sessions: Vec<Value> = g
                .sessions
                .iter()
                .filter(|((t, _), _)| *t == thread)
                .map(|(_, v)| v.clone())
                .collect();
            sessions.sort_by(|a, b| a["updatedAt"].as_str().cmp(&b["updatedAt"].as_str()));
            let epoch = g.epoch();
            return Ok(json!({"sessions":sessions,"serverEpoch":epoch,"revision":g.revision}));
        }
        "preview.open" => {
            if p["viewport"].is_object() && !viewport_ok(&p["viewport"]) {
                return Err(invalid("viewport is invalid"));
            }
            let nav = match p["url"].as_str().filter(|u| !u.trim().is_empty()) {
                Some(u) => json!({"_tag":"Loading","url":normalize_url(u)?,"title":""}),
                None => json!({"_tag":"Idle"}),
            };
            if g.sessions.len() >= MAX_SESSIONS {
                return Err(invalid("too many open preview sessions"));
            }
            let tab = server_id.unwrap_or_else(|| {
                g.next_tab += 1;
                format!("tab_{}", radix36(g.next_tab))
            });
            let mut snap = json!({"threadId":thread,"tabId":tab,"navStatus":nav,"canGoBack":false,"canGoForward":false,
                "viewport":if p["viewport"].is_object(){p["viewport"].clone()}else{json!({"_tag":"fill"})},"updatedAt":at});
            for k in ["profileId", "runtime", "reveal"] {
                if !p[k].is_null() {
                    snap[k] = p[k].clone();
                }
            }
            g.sessions
                .insert((thread.clone(), tab.clone()), snap.clone());
            g.publish(json!({"type":"opened","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snap}));
            if let Some(mut done) = completion {
                let inner = m.inner.clone();
                let key = (thread.clone(), tab.clone());
                tokio::spawn(async move {
                    if !*done.borrow() {
                        let _ = done.changed().await;
                    }
                    let mut g = inner.lock().unwrap();
                    if g.sessions.remove(&key).is_some() {
                        g.publish(json!({"type":"closed","threadId":key.0,"tabId":key.1,"createdAt":iso(now_ms())}));
                    }
                });
            }
            Ok(snap)
        }
        "preview.close" => {
            let targets: Vec<(String, String)> = match p["tabId"].as_str() {
                Some(tab) => g
                    .sessions
                    .keys()
                    .filter(|(t, k)| *t == thread && k == tab)
                    .cloned()
                    .collect(),
                None => g
                    .sessions
                    .keys()
                    .filter(|(t, _)| *t == thread)
                    .cloned()
                    .collect(),
            };
            for key in targets {
                g.sessions.remove(&key);
                g.publish(json!({"type":"closed","threadId":key.0,"tabId":key.1,"createdAt":at}));
            }
            Ok(Value::Null)
        }
        _ => {
            let tab = str_field(p, "tabId")?.to_owned();
            let key = (thread.clone(), tab.clone());
            let Some(mut snap) = g.sessions.get(&key).cloned() else {
                return Err(lookup_error(&thread, &tab));
            };
            let result = match tag {
                "preview.refresh" => return Ok(Value::Null),
                "preview.navigate" => {
                    let url = normalize_url(str_field(p, "url")?)?;
                    let previous = snap["navStatus"]["title"].as_str().unwrap_or("").to_owned();
                    let title = p["resolvedTitle"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or(previous);
                    snap["navStatus"] = json!({"_tag":"Success","url":url,"title":title});
                    snap["updatedAt"] = json!(at);
                    g.sessions.insert(key, snap.clone());
                    g.publish(json!({"type":"navigated","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snap}));
                    Ok(snap)
                }
                "preview.resize" => {
                    if !viewport_ok(&p["viewport"]) {
                        return Err(invalid("viewport is invalid"));
                    }
                    snap["viewport"] = p["viewport"].clone();
                    snap["updatedAt"] = json!(at);
                    g.sessions.insert(key, snap.clone());
                    g.publish(json!({"type":"resized","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snap}));
                    Ok(snap)
                }
                "preview.adjust" => {
                    if let Some(c) = p["colorScheme"].as_str() {
                        if !matches!(c, "system" | "light" | "dark") {
                            return Err(invalid("colorScheme is invalid"));
                        }
                        snap["colorScheme"] = json!(c);
                    }
                    if let Some(z) = p["zoomFactor"].as_f64() {
                        if !ZOOMS.iter().any(|k| (*k - z).abs() < 1e-9) {
                            return Err(invalid("zoomFactor is invalid"));
                        }
                        snap["zoomFactor"] = json!(z);
                    }
                    snap["updatedAt"] = json!(at);
                    let mut event = json!({"type":"resized","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snap});
                    let mut request = serde_json::Map::new();
                    if p["hardReload"] == true {
                        request.insert("hardReload".into(), json!(true));
                    }
                    if let Some(c) = p["clear"]
                        .as_str()
                        .filter(|c| matches!(*c, "cookies" | "cache"))
                    {
                        request.insert("clear".into(), json!(c));
                    }
                    if !request.is_empty() {
                        event["request"] = Value::Object(request);
                    }
                    g.sessions.insert(key, snap.clone());
                    g.publish(event);
                    Ok(snap)
                }
                _ => {
                    // preview.reportStatus
                    let nav = &p["navStatus"];
                    if !matches!(
                        nav["_tag"].as_str(),
                        Some("Idle" | "Loading" | "Success" | "LoadFailed")
                    ) {
                        return Err(invalid("navStatus is invalid"));
                    }
                    snap["navStatus"] = nav.clone();
                    snap["canGoBack"] = json!(p["canGoBack"] == true);
                    snap["canGoForward"] = json!(p["canGoForward"] == true);
                    snap["updatedAt"] = json!(at);
                    let event = if nav["_tag"] == "LoadFailed" {
                        let mut e = json!({"type":"failed","threadId":thread,"tabId":tab,"createdAt":at,"url":nav["url"],"title":nav["title"],"code":nav["code"],"description":nav["description"]});
                        if !nav["download"].is_null() {
                            e["download"] = nav["download"].clone();
                        }
                        e
                    } else {
                        json!({"type":"navigated","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snap})
                    };
                    g.sessions.insert(key, snap);
                    g.publish(event);
                    Ok(Value::Null)
                }
            };
            result
        }
    };
    drop(g);
    bump(&s.streams.changed);
    result
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
        seen: s.streams.p2.preview.inner.lock().unwrap().revision,
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
    call(s, "preview.close", &json!({"threadId":thread}))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub async fn shutdown(s: &State) {
    s.streams.p2.preview.browser.shutdown().await;
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
