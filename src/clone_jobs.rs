//! Tracked clone jobs, created on demand; raw transport output is never published.
use crate::store::{iso, now_ms, State};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Default)]
pub struct Registry(pub Mutex<BTreeMap<String, Job>>);
pub struct Job {
    pub snapshot: Value,
    pub cancel: tokio::sync::watch::Sender<bool>,
}
fn touch(s: &State, id: &str, f: impl FnOnce(&mut Value)) {
    if let Some(j) = s.streams.p2.clones.0.lock().unwrap().get_mut(id) {
        f(&mut j.snapshot);
        let n = j.snapshot["sequence"].as_u64().unwrap_or(0) + 1;
        j.snapshot["sequence"] = json!(n);
    }
    s.notify();
}
pub async fn start(
    s: Arc<State>,
    p: Value,
    destination: PathBuf,
    remote: String,
) -> Result<Value, Value> {
    let id = p["projectId"].as_str().unwrap().to_owned();
    let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
    {
        let mut jobs = s.streams.p2.clones.0.lock().unwrap();
        if jobs.contains_key(&id) {
            return Err(
                json!({"_tag":"SourceControlRepositoryError","message":"Clone job already exists"}),
            );
        }
        if jobs.len() >= 64 {
            jobs.retain(|_, j| j.snapshot["phase"] == "running");
        }
        if jobs.len() >= 64 {
            return Err(
                json!({"_tag":"SourceControlRepositoryError","message":"Clone capacity reached"}),
            );
        }
        let existed = destination.exists();
        if !existed {
            std::fs::create_dir(&destination).map_err(|_|json!({"_tag":"SourceControlRepositoryError","message":"Cannot create clone destination"}))?;
        }

        jobs.insert(id.clone(),Job{cancel,snapshot:json!({"projectId":id,"remoteUrl":remote,"destinationPath":destination,"repository":null,"phase":"running","stage":"connecting","percent":null,"detail":"Connecting to repository","error":null,"startedAt":iso(now_ms()),"endedAt":null,"sequence":0})});
    }
    let result = json!({"projectId":id,"cwd":destination,"remoteUrl":remote,"repository":null});
    let command = json!({"type":"project.create","commandId":format!("project-clone:{id}"),"projectId":id,"title":p["title"],"workspaceRoot":destination,"createdAt":p["createdAt"],"scripts":[]});
    s.notify();
    tokio::spawn(async move {
        let parent = destination.parent().unwrap().to_owned();
        let dest = destination.to_string_lossy().to_string();
        // Dropping the bounded proc runner kills the entire transport/hook group.
        let outcome = {
            let args = ["clone", "--no-tags", "--progress", "--", &remote, &dest];
            let progress_state = s.clone();
            let progress_id = id.clone();
            let observer = Arc::new(move |bytes: &[u8]| {
                let text = String::from_utf8_lossy(bytes);
                let stage = if text.contains("Resolving deltas") {
                    Some("resolving")
                } else if text.contains("Receiving objects") {
                    Some("receiving")
                } else if text.contains("Counting objects") {
                    Some("counting")
                } else if text.contains("Updating files") {
                    Some("checkout")
                } else {
                    None
                };
                if let Some(stage) = stage {
                    touch(&progress_state, &progress_id, |v| {
                        v["stage"] = json!(stage);
                        v["detail"] = Value::Null;
                    });
                }
            });
            let run = crate::proc::git_progress(
                &parent,
                &args,
                crate::proc::Opts {
                    timeout: Duration::from_secs(180),
                    cap: 1024 * 1024,
                    allow_truncate: false,
                },
                observer,
            );
            tokio::pin!(run);
            tokio::select! {_ = cancelled.changed()=>Err("cancelled"),out=&mut run=>match out{Ok(o) if o.ok()=>Ok(()),_=>Err("failed")}}
        };
        let outcome = if outcome.is_ok() && crate::events::mutate_project(&s, &command).is_err() {
            Err("failed")
        } else { outcome };
        let cleanup_failed =
            outcome.is_err() && tokio::fs::remove_dir_all(&destination).await.is_err();
        touch(&s, &id, |v| {
            v["phase"] = json!(if cleanup_failed {
                "failed"
            } else {
                match outcome {
                    Ok(()) => "done",
                    Err(e) => e,
                }
            });
            v["endedAt"] = json!(iso(now_ms()));
            v["detail"] = Value::Null;
            if cleanup_failed {
                v["error"] = json!("Partial clone cleanup failed; destination requires cleanup");
            } else if outcome == Err("failed") {
                v["error"] = json!("Clone failed or exceeded its time/output budget");
            }
        });
        if outcome.is_ok() { Ok(result) } else {
            Err(json!({"_tag":"SourceControlRepositoryError","message":if cleanup_failed {"Clone failed; destination requires cleanup"} else {"Clone failed or cancelled"}}))
        }
    }).await.map_err(|_|json!({"_tag":"SourceControlRepositoryError","message":"Clone worker failed"}))?
}
pub fn cancel(s: &State, id: &str) -> Value {
    let jobs = s.streams.p2.clones.0.lock().unwrap();
    let applied = jobs
        .get(id)
        .filter(|j| j.snapshot["phase"] == "running")
        .is_some_and(|j| j.cancel.send(true).is_ok());
    json!({"applied":applied})
}
#[derive(Default)]
pub struct Cursor {
    last: Option<Value>,
    waiting: bool,
}

impl Cursor {
    pub fn next(&mut self, s: &State) -> crate::terminal::Step {
        if self.waiting {
            return crate::terminal::Step::Idle;
        }
        let v = json!(s
            .streams
            .p2
            .clones
            .0
            .lock()
            .unwrap()
            .values()
            .map(|j| j.snapshot.clone())
            .collect::<Vec<_>>());
        if self.last.as_ref() == Some(&v) {
            return crate::terminal::Step::Idle;
        }
        self.last = Some(v.clone());
        self.waiting = true;
        crate::terminal::Step::Values(vec![v])
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
}
