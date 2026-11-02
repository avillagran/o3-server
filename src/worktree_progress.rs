//! On-demand setup snapshots and bounded, cancellable setup process groups.
use crate::store::{iso, now_ms, State};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Read,
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
    jobs.insert(tid.into(),Job{cancel:Arc::new(AtomicBool::new(false)), snapshot:json!({"threadId":tid,"phase":"running","startedAt":at,"endedAt":null,"branch":strategy.get("branch").unwrap_or(&Value::Null),"baseRef":strategy.get("baseRef").unwrap_or(&Value::Null),"worktreePath":null,"setupScript":null,"error":null,"sequence":0,"stages":(["fetch","checkout","submodules","setup-script","agent"].iter().map(|id|json!({"id":id,"status":"pending","startedAt":null,"endedAt":null,"percent":null,"detail":null,"tail":[]})).collect::<Vec<_>>())})});
    drop(runtime);
    s.notify();
    Ok(())
}
pub fn update(s: &State, tid: &str, f: impl FnOnce(&mut Value)) {
    if let Some(job) = s.streams.p2.worktrees.lock().unwrap().0.get_mut(tid) {
        f(&mut job.snapshot);
        let n = job.snapshot["sequence"].as_u64().unwrap_or(0) + 1;
        job.snapshot["sequence"] = json!(n);
    }
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
    let mut child = cmd
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    let readers: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    let handles: Vec<_> = readers
        .into_iter()
        .map(|mut reader| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0; 1024];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(120);
    let result = loop {
        if let Err(e) = check(s, tid) {
            break Err(e);
        }
        if Instant::now() >= deadline {
            break Err("Setup process timed out".into());
        }
        if let Ok(bytes) = rx.recv_timeout(Duration::from_millis(20)) {
            let line = String::from_utf8_lossy(&bytes)
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
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    drop(rx);
    for h in handles {
        let _ = h.join();
    }
    result
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
