//! Server operations: client activity, provider refresh, secret answers, process diagnostics,
//! resource telemetry, usage and storage cleanup. Each returns what the server really knows;
//! where it has nothing (no providers, no usage ledger) the answer is empty, not invented.
use crate::{
    events::{append, get, put},
    store::{iso, now_ms, random_id, State},
    terminal::{bump, Notifier},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{atomic::AtomicBool, Mutex},
    time::Duration,
};

type Reply = Result<Value, Value>;

const MAX_CLIENTS: usize = 64;
const SAMPLE_EVERY: Duration = Duration::from_secs(10);
const MAX_SAMPLES: usize = 360;
const LOG_RETENTION_MS: i64 = 14 * 24 * 3600 * 1000;
const MAX_REPORT_ENTRIES: usize = 200;

pub struct Ops {
    clients: Mutex<HashMap<String, (i64, Value)>>,
    samples: Mutex<VecDeque<Sample>>,
    sampler: AtomicBool,
    report: Mutex<(u64, Option<Value>)>,
    pub scheduler: AtomicBool,
    pub scheduler_wake: tokio::sync::Notify,
    notify: Notifier,
}

#[derive(Clone)]
struct Sample {
    at: i64,
    cpu_percent: f64,
    rss: u64,
    processes: u64,
    io_read: u64,
    io_write: u64,
}

impl Ops {
    pub fn new(notify: Notifier) -> Self {
        Self {
            clients: Mutex::default(),
            samples: Mutex::default(),
            sampler: AtomicBool::new(false),
            report: Mutex::new((0, None)),
            scheduler: AtomicBool::new(false),
            scheduler_wake: tokio::sync::Notify::new(),
            notify,
        }
    }
    /// Live reports from clients that said they are looking at the app.
    pub fn active_clients(&self) -> usize {
        let now = now_ms();
        self.clients
            .lock()
            .unwrap()
            .values()
            .filter(|(until, r)| *until > now && r["visible"] == true)
            .count()
    }
    pub fn report_version(&self) -> (u64, Option<Value>) {
        self.report.lock().unwrap().clone()
    }
    pub fn wake_scheduler(&self) {
        self.scheduler_wake.notify_one();
    }
}

fn invalid(message: &str) -> Value {
    json!({"_tag":"RpcInvalidPayload","message":message})
}

pub async fn call(s: &std::sync::Arc<State>, tag: &str, p: &Value) -> Option<Reply> {
    Some(match tag {
        "server.reportClientActivity" => report_activity(s, p),
        "server.refreshProviders" => refresh_providers(s, p).await,
        "secrets.answerRequest" => answer_secret(s, p),
        "server.getProcessDiagnostics" => Ok(diagnostics()),
        "server.getResourceTelemetryHistory" => telemetry(s, p),
        "server.signalProcess" => signal(p),
        "server.getUsageSummary" => usage(p),
        "server.runStorageCleanup" => cleanup(s),
        _ => return crate::scheduled::call(s, tag, p),
    })
}

// ---------------------------------------------------------------- activity and providers

fn report_activity(s: &State, p: &Value) -> Reply {
    let id = p["clientId"]
        .as_str()
        .map(str::trim)
        .filter(|c| !c.is_empty() && c.len() <= 128)
        .ok_or_else(|| invalid("clientId must be 1-128 characters"))?;
    if !matches!(
        p["clientKind"].as_str(),
        Some("web" | "desktop-renderer" | "mobile" | "unknown")
    ) {
        return Err(invalid("clientKind"));
    }
    for k in ["visible", "focused", "recentlyInteracted"] {
        if !p[k].is_boolean() {
            return Err(invalid(k));
        }
    }
    if !p["scopes"].is_array() || !p["observedAt"].is_string() {
        return Err(invalid("scopes and observedAt required"));
    }
    let ttl = p["ttlMs"]
        .as_f64()
        .filter(|t| *t >= 0.0)
        .unwrap_or(60_000.0)
        .min(600_000.0) as i64;
    let mut clients = s.streams.ops.clients.lock().unwrap();
    let now = now_ms();
    clients.retain(|_, (until, _)| *until > now);
    if clients.len() >= MAX_CLIENTS && !clients.contains_key(id) {
        let oldest = clients
            .iter()
            .min_by_key(|(_, (u, _))| *u)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            clients.remove(&k);
        }
    }
    clients.insert(id.to_owned(), (now + ttl, p.clone()));
    Ok(Value::Null)
}

async fn refresh_providers(s: &State, _p: &Value) -> Reply {
    let provider = s.provider.lock().unwrap().clone();
    if let Some(provider) = provider {
        provider
            .refresh()
            .await
            .map_err(|m| json!({"_tag":"ProviderSetupError","message":m}))?;
    }
    s.notify();
    let config =
        crate::settings::config(s).map_err(|m| json!({"_tag":"ProviderSetupError","message":m}))?;
    Ok(json!({"providers":config["providers"]}))
}

// ---------------------------------------------------------------- secrets

fn secret_error(reason: &str) -> Value {
    json!({"_tag":"SecretRequestError","reason":reason})
}

/// The answer to a `secret_request` item. A saved value goes to an owner-only file in the profile
/// and never into the database, the transcript or a log; the item records only the status.
fn answer_secret(s: &State, p: &Value) -> Reply {
    let (thread, item_id) = (
        p["threadId"].as_str().filter(|v| !v.is_empty()),
        p["turnItemId"].as_str().filter(|v| !v.is_empty()),
    );
    let (Some(thread), Some(item_id)) = (thread, item_id) else {
        return Err(invalid("threadId and turnItemId required"));
    };
    let save = match p["answer"]["type"].as_str() {
        Some("save") => Some(
            p["answer"]["secret"]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| invalid("secret must not be empty"))?
                .to_owned(),
        ),
        Some("decline") => None,
        _ => return Err(invalid("answer.type")),
    };
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|_| secret_error("load_failed"))?;
    let mut item = get(&tx, "turnItems", item_id)
        .ok()
        .filter(|i| i["threadId"] == thread && i["type"] == "secret_request")
        .ok_or_else(|| secret_error("not_found"))?;
    if item["secretStatus"] != "pending" {
        return Err(secret_error("already_answered"));
    }
    if let Some(run) = item["runId"]
        .as_str()
        .and_then(|r| get(&tx, "runs", r).ok())
    {
        if matches!(
            run["status"].as_str(),
            Some("interrupted" | "failed" | "cancelled" | "rolled_back")
        ) {
            return Err(secret_error("agent_stopped"));
        }
    }
    let mut written = None;
    if let Some(secret) = &save {
        let path = secret_path(s, thread, item_id);
        write_private(&path, secret).map_err(|_| secret_error("store_failed"))?;
        written = Some(path);
    }
    item["secretStatus"] = json!(if save.is_some() { "saved" } else { "declined" });
    item["updatedAt"] = json!(iso(now_ms()));
    let recorded = (|| -> Result<(), String> {
        let thread_rec = get(&tx, "thread", thread)?;
        put(&tx, "turnItems", item_id, thread, &item)?;
        let ordinal = item["ordinal"].clone();
        put(&tx, "visibleTurnItems", item_id, thread, &json!({"position":ordinal,"visibility":"local","sourceThreadId":thread,"sourceItemId":item_id,"item":item}))?;
        append(&tx, thread, "turn-item.updated", item.clone(), &thread_rec)?;
        Ok(())
    })()
    .and_then(|_| tx.commit().map_err(|e| e.to_string()));
    if recorded.is_err() {
        if let Some(path) = written {
            let _ = std::fs::remove_file(path);
        }
        return Err(secret_error("record_failed"));
    }
    drop(db);
    s.notify();
    Ok(Value::Null)
}

/// Where a saved secret lives: a one-use ref derived from the request and the profile's signing key.
pub fn secret_path(s: &State, thread: &str, item: &str) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&s.signing_key);
    h.update(thread.as_bytes());
    h.update([0]);
    h.update(item.as_bytes());
    let hex: String = h.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    s.dir.join("secrets").join(format!("secret-request-{hex}"))
}

fn write_private(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(content.as_bytes())
}

// ---------------------------------------------------------------- processes

pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub state: char,
    pub start_ms: i64,
    pub cpu_ticks: u64,
    pub rss: u64,
    pub command: String,
}

fn ticks() -> f64 {
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f64 }.max(1.0)
}
fn boot_ms() -> i64 {
    std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("btime ")
                    .and_then(|v| v.trim().parse::<i64>().ok())
            })
        })
        .unwrap_or(0)
        * 1000
}

pub fn read_proc(pid: i32, boot: i64) -> Option<ProcInfo> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok());
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
    let rss = std::fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * page;
    let comm = stat
        .split_once('(')
        .and_then(|(_, r)| r.rsplit_once(')'))
        .map(|(c, _)| c.to_owned())
        .unwrap_or_default();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|b| {
            String::from_utf8_lossy(&b)
                .replace('\0', " ")
                .trim()
                .to_owned()
        })
        .filter(|c| !c.is_empty());
    let command: String = cmdline
        .unwrap_or_else(|| format!("[{comm}]"))
        .chars()
        .take(512)
        .collect();
    Some(ProcInfo {
        pid,
        ppid: num(1)? as i32,
        pgid: num(2)? as i32,
        state: f.first()?.chars().next()?,
        start_ms: boot + (num(19)? as f64 * 1000.0 / ticks()) as i64,
        cpu_ticks: num(11)? + num(12)?,
        rss,
        command,
    })
}

/// The server process and everything below it (terminal shells, their jobs, provider children).
pub fn tree() -> Vec<(ProcInfo, usize)> {
    let boot = boot_ms();
    let server = std::process::id() as i32;
    let mut all: HashMap<i32, ProcInfo> = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            if let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) {
                if let Some(p) = read_proc(pid, boot) {
                    all.insert(pid, p);
                }
            }
        }
    }
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for p in all.values() {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    for v in children.values_mut() {
        v.sort_unstable();
    }
    let mut out = Vec::new();
    let mut stack = vec![(server, 0usize)];
    while let Some((pid, depth)) = stack.pop() {
        if let Some(p) = all.remove(&pid) {
            for c in children.get(&pid).into_iter().flatten().rev() {
                stack.push((*c, depth + 1));
            }
            out.push((p, depth));
        }
    }
    out
}

fn elapsed(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    let (d, h, m, sec) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d}-{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{h:02}:{m:02}:{sec:02}")
    }
}

fn diagnostics() -> Value {
    let list = tree();
    let now = now_ms();
    let children = |pid: i32| {
        list.iter()
            .filter(|(p, _)| p.ppid == pid)
            .map(|(p, _)| p.pid)
            .collect::<Vec<_>>()
    };
    let entries: Vec<Value> = list
        .iter()
        .map(|(p, depth)| {
            let age = (now - p.start_ms).max(1);
            let cpu = p.cpu_ticks as f64 / ticks() * 1000.0 / age as f64 * 100.0;
            json!({"pid":p.pid,"startTimeMs":p.start_ms.max(0),"ppid":p.ppid,"pgid":{"_tag":"Some","value":p.pgid},"status":p.state.to_string(),"cpuPercent":(cpu * 10.0).round() / 10.0,"rssBytes":p.rss,"elapsed":elapsed(age),"command":p.command,"depth":depth,"childPids":children(p.pid)})
        })
        .collect();
    let rss: u64 = list.iter().map(|(p, _)| p.rss).sum();
    let cpu: f64 = entries
        .iter()
        .filter_map(|e| e["cpuPercent"].as_f64())
        .sum();
    json!({"serverPid":std::process::id(),"readAt":iso(now),"processCount":entries.len(),"totalRssBytes":rss,"totalCpuPercent":cpu,"processes":entries,"error":{"_tag":"None"}})
}

fn signal(p: &Value) -> Reply {
    let pid = p["pid"]
        .as_i64()
        .filter(|n| *n > 0 && *n <= i32::MAX as i64)
        .ok_or_else(|| invalid("pid"))? as i32;
    let start = p["startTimeMs"]
        .as_i64()
        .filter(|n| *n >= 0)
        .ok_or_else(|| invalid("startTimeMs"))?;
    let (name, sig) = match p["signal"].as_str() {
        Some("SIGINT") => ("SIGINT", libc::SIGINT),
        Some("SIGKILL") => ("SIGKILL", libc::SIGKILL),
        _ => return Err(invalid("signal must be SIGINT or SIGKILL")),
    };
    let reply = |signaled: bool, message: Option<&str>| {
        Ok(
            json!({"pid":pid,"signal":name,"signaled":signaled,"message":match message {Some(m)=>json!({"_tag":"Some","value":m}),None=>json!({"_tag":"None"})}}),
        )
    };
    // Only descendants of this server, identified by pid and start time so a recycled pid is safe.
    let target = tree()
        .into_iter()
        .find(|(proc, depth)| proc.pid == pid && *depth > 0);
    match target {
        None => reply(false, Some("The process is not owned by this server.")),
        Some((proc, _)) if proc.start_ms.max(0) != start => reply(
            false,
            Some("The process no longer matches the requested start time."),
        ),
        Some(_) => {
            let rc = unsafe { libc::kill(pid, sig) };
            if rc == 0 {
                reply(true, None)
            } else {
                reply(false, Some("The signal could not be delivered."))
            }
        }
    }
}

// ---------------------------------------------------------------- telemetry

fn io_bytes() -> (u64, u64) {
    let io = std::fs::read_to_string("/proc/self/io").unwrap_or_default();
    let field = |name: &str| {
        io.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    (field("read_bytes:"), field("write_bytes:"))
}

fn take_sample(previous: &mut Option<(i64, u64)>) -> Sample {
    let list = tree();
    let now = now_ms();
    let server_ticks = list.first().map_or(0, |(p, _)| p.cpu_ticks);
    let cpu = previous
        .map(|(at, t)| {
            (server_ticks.saturating_sub(t)) as f64 / ticks() * 1000.0 / (now - at).max(1) as f64
                * 100.0
        })
        .unwrap_or(0.0);
    *previous = Some((now, server_ticks));
    let (r, w) = io_bytes();
    Sample {
        at: now,
        cpu_percent: cpu,
        rss: list.iter().map(|(p, _)| p.rss).sum(),
        processes: list.len() as u64,
        io_read: r,
        io_write: w,
    }
}

fn push_sample(ops: &Ops, sample: Sample) {
    let mut q = ops.samples.lock().unwrap();
    q.push_back(sample);
    while q.len() > MAX_SAMPLES {
        q.pop_front();
    }
}

/// History starts when somebody first asks: an unqueried server runs no sampler.
fn ensure_sampler(s: &std::sync::Arc<State>) {
    if s.streams
        .ops
        .sampler
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let s = s.clone();
    tokio::spawn(async move {
        let mut previous = None;
        loop {
            push_sample(&s.streams.ops, take_sample(&mut previous));
            tokio::time::sleep(SAMPLE_EVERY).await;
        }
    });
}

fn telemetry(s: &std::sync::Arc<State>, p: &Value) -> Reply {
    ensure_sampler(s);
    let window = p["windowMs"]
        .as_u64()
        .ok_or_else(|| invalid("windowMs"))?
        .clamp(1_000, 3_600_000);
    let bucket = p["bucketMs"]
        .as_u64()
        .ok_or_else(|| invalid("bucketMs"))?
        .clamp(1_000, window)
        .max(window / 120);
    // Make sure the answer includes the present moment even on the first call.
    if s.streams.ops.samples.lock().unwrap().is_empty() {
        push_sample(&s.streams.ops, take_sample(&mut None));
    }
    let now = now_ms();
    let samples: Vec<Sample> = s
        .streams
        .ops
        .samples
        .lock()
        .unwrap()
        .iter()
        .filter(|x| x.at > now - window as i64)
        .cloned()
        .collect();
    let count = window.div_ceil(bucket);
    let start = now - (count * bucket) as i64;
    let mut buckets = Vec::new();
    for i in 0..count as i64 {
        let (from, to) = (start + i * bucket as i64, start + (i + 1) * bucket as i64);
        let inside: Vec<&Sample> = samples
            .iter()
            .filter(|x| x.at > from && x.at <= to)
            .collect();
        let avg = if inside.is_empty() {
            0.0
        } else {
            inside.iter().map(|x| x.cpu_percent).sum::<f64>() / inside.len() as f64
        };
        let span = |f: fn(&Sample) -> u64| {
            inside
                .last()
                .map_or(0, |l| f(l))
                .saturating_sub(inside.first().map_or(0, |x| f(x)))
        };
        buckets.push(json!({"startedAt":iso(from),"endedAt":iso(to),"avgCpuPercent":avg,"maxCpuPercent":inside.iter().map(|x| x.cpu_percent).fold(0.0, f64::max),"maxRssBytes":inside.iter().map(|x| x.rss).max().unwrap_or(0),"ioReadBytes":span(|x| x.io_read),"ioWriteBytes":span(|x| x.io_write),"maxProcessCount":inside.iter().map(|x| x.processes).max().unwrap_or(0)}));
    }
    let list = tree();
    let top: Vec<Value> = list
        .iter()
        .take(10)
        .map(|(proc, depth)| {
            let age = (now - proc.start_ms).max(1);
            let cpu = proc.cpu_ticks as f64 / ticks() * 1000.0 / age as f64 * 100.0;
            json!({"identity":{"pid":proc.pid,"startTimeMs":proc.start_ms.max(0)},"ppid":proc.ppid,"depth":depth,"name":proc.command.split_whitespace().next().unwrap_or(""),"command":proc.command,"category":if *depth == 0 {"server"} else {"server-child"},"firstSeenAt":iso(proc.start_ms),"lastSeenAt":iso(now),"currentCpuPercent":cpu,"avgCpuPercent":cpu,"maxCpuPercent":cpu,"cpuTimeMs":(proc.cpu_ticks as f64 * 1000.0 / ticks()) as u64,"currentRssBytes":proc.rss,"peakRssBytes":proc.rss,"ioReadBytes":0,"ioWriteBytes":0,"ioSemantics":"unavailable","sampleCount":samples.len()})
        })
        .collect();
    let healthy = json!({"status":"healthy","lastSampleAt":{"_tag":"Some","value":iso(now)},"lastError":{"_tag":"None"}});
    let absent =
        json!({"status":"unavailable","lastSampleAt":{"_tag":"None"},"lastError":{"_tag":"None"}});
    Ok(
        json!({"readAt":iso(now),"windowMs":window,"bucketMs":bucket,"sampleIntervalMs":SAMPLE_EVERY.as_millis() as u64,"retainedSampleCount":samples.len(),"buckets":buckets,"topProcesses":top,"health":{"native":healthy,"desktop":absent,"sidecarVersion":{"_tag":"None"},"sidecarPid":{"_tag":"None"},"restartCount":0,"collectionDurationMicros":0,"scannedProcessCount":list.len(),"retainedProcessCount":list.len(),"inaccessibleProcessCount":0}}),
    )
}

// ---------------------------------------------------------------- usage

fn usage(p: &Value) -> Reply {
    let (Some(since), Some(until), Some(zone)) = (
        p["sinceDay"].as_str(),
        p["untilDay"].as_str(),
        p["timeZone"].as_str().filter(|z| !z.trim().is_empty()),
    ) else {
        return Err(invalid("sinceDay, untilDay and timeZone required"));
    };
    // o3-server reads no provider transcripts or ledgers, so it has no usage to report.
    Ok(
        json!({"contractVersion":6,"readAt":iso(now_ms()),"timeZone":zone,"sinceDay":since,"untilDay":until,"buckets":[],"sources":[],"pricing":{"status":"unavailable","source":"none","fetchedAt":null,"knownModels":0},"scanDurationMs":0}),
    )
}

// ---------------------------------------------------------------- storage cleanup

/// Removes log files of this profile older than two weeks; nothing else is ever deleted.
fn cleanup(s: &State) -> Reply {
    let started = iso(now_ms());
    let (mut entries, mut removed, mut kept, mut failed, mut freed) =
        (Vec::new(), 0u64, 0u64, 0u64, 0u64);
    let mut stack = vec![s.dir.join("logs")];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in read.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(e.path());
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let age = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map_or(0, |d| d.as_millis() as i64);
            if age < LOG_RETENTION_MS {
                kept += 1;
                continue;
            }
            let (outcome, reason) = match std::fs::remove_file(e.path()) {
                Ok(()) => {
                    removed += 1;
                    freed += meta.len();
                    ("removed", "older than 14 days")
                }
                Err(_) => {
                    failed += 1;
                    ("failed", "could not delete")
                }
            };
            if entries.len() < MAX_REPORT_ENTRIES {
                entries.push(json!({"kind":"logs","outcome":outcome,"reason":reason,"path":e.path(),"projectPath":null,"threadId":null,"threadTitle":null,"bytes":meta.len(),"files":1}));
            }
        }
    }
    let omitted = (removed + failed).saturating_sub(entries.len() as u64);
    let report = json!({"trigger":"manual","startedAt":started,"finishedAt":iso(now_ms()),"entries":entries,"counts":{"removed":removed,"kept":kept,"failed":failed},"omittedCount":omitted,"bytesFreed":freed});
    {
        let mut r = s.streams.ops.report.lock().unwrap();
        r.0 += 1;
        r.1 = Some(report.clone());
    }
    bump(&s.streams.ops.notify);
    Ok(report)
}

/// Fresh random id for rows this module creates.
pub fn new_id() -> String {
    random_id().unwrap_or_default()
}
