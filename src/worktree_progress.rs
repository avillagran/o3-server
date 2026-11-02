//! On-demand setup snapshots and bounded, cancellable setup process groups.
use crate::store::{iso, now_ms, State};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Read,
    os::fd::AsRawFd,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Registry(pub HashMap<String, Job>);
pub struct Job {
    pub snapshot: Value,
    pub cancel: Arc<AtomicBool>,
    pub token: Option<String>,
}
pub fn begin(s: &State, tid: &str, strategy: &Value) -> Result<(), String> {
    let mut runtime = s.streams.p2.worktrees.lock().unwrap();
    let jobs = &mut runtime.0;
    if jobs.len() >= 64 {
        jobs.retain(|_, j| j.snapshot["phase"] == "running");
    }
    if jobs.len() >= 64 {
        return Err("Setup capacity reached".into());
    }
    let at = iso(now_ms());
    jobs.insert(tid.into(),Job{token:None,cancel:Arc::new(AtomicBool::new(false)), snapshot:json!({"threadId":tid,"phase":"running","startedAt":at,"endedAt":null,"branch":strategy.get("branch").unwrap_or(&Value::Null),"baseRef":strategy.get("baseRef").unwrap_or(&Value::Null),"worktreePath":null,"setupScript":null,"error":null,"sequence":0,"stages":(["fetch","checkout","submodules","setup-script","agent"].iter().map(|id|json!({"id":id,"status":"pending","startedAt":null,"endedAt":null,"percent":null,"detail":null,"tail":[]})).collect::<Vec<_>>())})});
    persist(s, &runtime)?;
    drop(runtime);
    s.notify();
    Ok(())
}
pub fn update(s: &State, tid: &str, f: impl FnOnce(&mut Value)) {
    let mut runtime = s.streams.p2.worktrees.lock().unwrap();
    if let Some(job) = runtime.0.get_mut(tid) {
        f(&mut job.snapshot);
        let n = job.snapshot["sequence"].as_u64().unwrap_or(0) + 1;
        job.snapshot["sequence"] = json!(n);
    }
    let _ = persist(s, &runtime);
    drop(runtime);
    s.notify();
}
pub fn stage(s: &State, tid: &str, id: &str, status: &str) {
    update(s, tid, |v| {
        for st in v["stages"].as_array_mut().unwrap() {
            if st["id"] == id {
                st["status"] = json!(status);
                st[if status == "running" {
                    "startedAt"
                } else {
                    "endedAt"
                }] = json!(iso(now_ms()));
            }
        }
    });
}
pub fn finish(s: &State, tid: &str, result: &Result<(), String>) {
    update(s, tid, |v| {
        v["phase"] = json!(match result {
            Ok(_) => "done",
            Err(e) if e == "Setup cancelled" => "cancelled",
            Err(_) => "failed",
        });
        v["endedAt"] = json!(iso(now_ms()));
        for stage in v["stages"].as_array_mut().unwrap() {
            if stage["status"] == "running" {
                stage["status"] = json!(if result.is_ok() { "done" } else { "failed" });
                stage["endedAt"] = json!(iso(now_ms()));
            }
        }
        if let Err(e) = result {
            v["error"] = json!(e.chars().take(1000).collect::<String>());
        }
    });
}
pub fn cancel(s: &State, tid: &str) -> bool {
    let r = s.streams.p2.worktrees.lock().unwrap();
    if let Some(j) = r.0.get(tid).filter(|j| j.snapshot["phase"] == "running") {
        j.cancel.store(true, Ordering::Release);
        true
    } else {
        false
    }
}
pub fn check(s: &State, tid: &str) -> Result<(), String> {
    if s.streams
        .p2
        .worktrees
        .lock()
        .unwrap()
        .0
        .get(tid)
        .is_some_and(|j| j.cancel.load(Ordering::Acquire))
    {
        Err("Setup cancelled".into())
    } else {
        Ok(())
    }
}
pub fn run(s: &State, tid: &str, stage_id: &str, cmd: &mut Command) -> Result<(), String> {
    check(s, tid)?;
    use std::os::unix::process::CommandExt;
    let token = crate::store::random_id()?;
    {
        let mut runtime = s.streams.p2.worktrees.lock().unwrap();
        runtime.0.get_mut(tid).ok_or("Setup job missing")?.token = Some(token.clone());
        persist(s, &runtime)?;
    }
    let mut child = cmd
        .env("O3_SETUP_OWNER", &token)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut readers: [Box<dyn Pipe>; 2] = [
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    for reader in &readers {
        let fd = reader.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            crate::setup_process::stop(&token);
            let _ = child.try_wait();
            return Err("Cannot make setup output nonblocking".into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    let result = loop {
        if let Err(e) = check(s, tid) {
            break Err(e);
        }
        if Instant::now() >= deadline {
            break Err("Setup process timed out".into());
        }
        for reader in &mut readers {
            let mut bytes = [0; 1024];
            if let Ok(n) = reader.read(&mut bytes) {
                if n > 0 {
                    let line = String::from_utf8_lossy(&bytes[..n])
                        .chars()
                        .take(400)
                        .collect::<String>();
                    update(s, tid, |v| {
                        for st in v["stages"].as_array_mut().unwrap() {
                            if st["id"] == stage_id {
                                let tail = st["tail"].as_array_mut().unwrap();
                                tail.push(json!(line));
                                if tail.len() > 20 {
                                    tail.remove(0);
                                }
                            }
                        }
                    });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err("Setup process failed".into())
                }
            }
            Err(e) => break Err(e.to_string()),
            _ => {}
        }
    };
    let stopped = crate::setup_process::stop(&token);
    // Neither wait nor pipe reads may extend the cancellation bound.
    let _ = child.try_wait();
    drop(readers);
    if !stopped {
        return Err("Setup cleanup could not be confirmed".into());
    }
    result
}
trait Pipe: Read + AsRawFd {}
impl<T: Read + AsRawFd> Pipe for T {}

fn persist(s: &State, registry: &Registry) -> Result<(), String> {
    let values: HashMap<_, _> = registry
        .0
        .iter()
        .map(|(id, job)| (id, json!({"snapshot":job.snapshot,"token":job.token})))
        .collect();
    let path = s.dir.join("worktree-setup.json");
    let temp = s.dir.join("worktree-setup.new");
    std::fs::write(
        &temp,
        serde_json::to_vec(&values).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
pub fn restore(s: &State) -> Result<(), String> {
    let path = s.dir.join("worktree-setup.json");
    if !path.exists() {
        return Ok(());
    }
    let values: HashMap<String, Value> =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let mut runtime = s.streams.p2.worktrees.lock().unwrap();
    for (id, value) in values {
        let mut snapshot = value["snapshot"].clone();
        if snapshot["phase"] == "running" {
            if let Some(token) = value["token"].as_str() {
                if !crate::setup_process::stop(token) {
                    return Err("Cannot stop interrupted setup".into());
                }
            }
            for stage in snapshot["stages"]
                .as_array_mut()
                .ok_or("Invalid setup stages")?
            {
                if stage["status"] == "running" {
                    stage["status"] = json!("failed");
                    stage["endedAt"] = json!(iso(now_ms()));
                }
            }
            snapshot["phase"] = json!("failed");
            snapshot["endedAt"] = json!(iso(now_ms()));
            snapshot["error"] = json!("Setup interrupted by server restart; retry reruns setup");
        }
        runtime.0.insert(
            id,
            Job {
                snapshot,
                token: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
    }
    persist(s, &runtime)
}

pub struct Cursor {
    tid: String,
    last: Option<Value>,
    waiting: bool,
}
impl Cursor {
    pub fn new(tid: String) -> Self {
        Self {
            tid,
            last: None,
            waiting: false,
        }
    }
    pub fn next(&mut self, s: &State) -> crate::terminal::Step {
        if self.waiting {
            return crate::terminal::Step::Idle;
        }
        let value = s
            .streams
            .p2
            .worktrees
            .lock()
            .unwrap()
            .0
            .get(&self.tid)
            .map(|j| j.snapshot.clone())
            .unwrap_or(Value::Null);
        if self.last.as_ref() == Some(&value) {
            return crate::terminal::Step::Idle;
        }
        self.last = Some(value.clone());
        self.waiting = true;
        crate::terminal::Step::Values(vec![value])
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
}
