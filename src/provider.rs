//! Adapter boundary: local asynchronous tasks, bounded output, explicit cancellation.
use std::{future::Future, pin::Pin};
use tokio::sync::{mpsc, watch};
pub struct RunInput {
    pub thread_id: String,
    pub run_id: String,
    pub cwd: std::path::PathBuf,
    pub text: String,
    pub model_selection: serde_json::Value,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub attachments: serde_json::Value,
    pub context: Option<serde_json::Value>,
}
pub enum ProviderEvent {
    Started,
    TextDelta(String),
    ApprovalRequest { request_id: String, command: String },
    Completed,
    Failed(String),
}
pub enum ProviderControl {
    Interrupt,
    Respond {
        request_id: String,
        decision: serde_json::Value,
        feedback: Option<String>,
        answers: Option<serde_json::Value>,
    },
}
/// Implementations must await output capacity, stop on cancellation/receiver drop,
/// and reap every subprocess before resolving. Futures must be Send for the shared two-worker runtime.
pub trait Provider: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        control: mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}
pub struct ScriptedProvider;
impl Provider for ScriptedProvider {
    fn execute<'a>(
        &'a self,
        _input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        mut control: mpsc::Receiver<ProviderControl>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            for event in [
                ProviderEvent::Started,
                ProviderEvent::TextDelta("scripted response".into()),
                ProviderEvent::ApprovalRequest {
                    request_id: "approval-1".into(),
                    command: "echo test".into(),
                },
            ] {
                tokio::select! {r=output.send(event)=>r.map_err(|_|"receiver closed")?,_=cancel.changed()=>return Ok(())}
            }
            tokio::select! {c=control.recv()=>match c {Some(ProviderControl::Respond{..})=>output.send(ProviderEvent::Completed).await.map_err(|_|"receiver closed".into()),_=>Ok(())},_=cancel.changed()=>Ok(())}
        })
    }
}

/// Run supervisor: only bounded channels and handles live in RAM; every output is persisted.
pub(crate) fn start(
    s: std::sync::Arc<crate::store::State>,
    tid: String,
    rid: String,
    text: String,
) {
    let Some(provider) = s.provider.lock().unwrap().clone() else {
        return;
    };
    let (output, mut events) = mpsc::channel(16);
    let (control, controls) = mpsc::channel(8);
    let (cancel, cancellation) = watch::channel(false);
    s.active.lock().unwrap().insert(rid.clone(), control);
    tokio::task::spawn(async move {
        let input = {
            let db = s.db.lock().unwrap();
            let t = crate::events::get(&db, "thread", &tid).unwrap_or_default();
            let r = crate::events::get(&db, "runs", &rid).unwrap_or_default();
            let m = crate::events::get(&db, "messages", r["userMessageId"].as_str().unwrap_or(""))
                .unwrap_or_default();
            let cwd = t["worktreePath"]
                .as_str()
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    crate::events::get(&db, "project", t["projectId"].as_str().unwrap_or(""))
                        .ok()
                        .and_then(|p| p["workspaceRoot"].as_str().map(std::path::PathBuf::from))
                })
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            RunInput {
                thread_id: tid.clone(),
                run_id: rid.clone(),
                cwd,
                text,
                model_selection: t["modelSelection"].clone(),
                runtime_mode: t["runtimeMode"].as_str().unwrap_or("full-access").into(),
                interaction_mode: t["interactionMode"].as_str().unwrap_or("default").into(),
                attachments: m["attachments"].clone(),
                context: m.get("context").cloned(),
            }
        };
        let mut task = tokio::task::spawn(async move {
            provider
                .execute(input, output, controls, cancellation)
                .await
        });
        let mut finished = false;
        while let Some(event) = events.recv().await {
            if persist(&s, &tid, &rid, event).is_err() {
                break;
            }
            let interrupted = crate::events::get(&s.db.lock().unwrap(), "runs", &rid)
                .is_ok_and(|r| r["status"] == "interrupted");
            if interrupted {
                break;
            }
        }
        let _ = cancel.send(true);
        tokio::select! {r=&mut task=>{finished=r.is_ok_and(|r|r.is_ok())},_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{task.abort();let _=task.await;}}
        s.active.lock().unwrap().remove(&rid);
        let final_run = crate::events::get(&s.db.lock().unwrap(), "runs", &rid);
        if let Ok(run) = final_run {
            if !matches!(
                run["status"].as_str(),
                Some("completed" | "interrupted" | "failed")
            ) {
                let _ = persist(
                    &s,
                    &tid,
                    &rid,
                    ProviderEvent::Failed(
                        if finished {
                            "provider ended without completion"
                        } else {
                            "provider failed"
                        }
                        .into(),
                    ),
                );
            }
        }
    });
}
fn persist(
    s: &crate::store::State,
    tid: &str,
    rid: &str,
    event: ProviderEvent,
) -> Result<(), String> {
    use crate::{
        events::{append, get, message_item, put},
        store::{iso, now_ms},
    };
    use serde_json::json;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let thread = get(&tx, "thread", tid)?;
    let mut run = get(&tx, "runs", rid)?;
    if matches!(
        run["status"].as_str(),
        Some("interrupted" | "completed" | "failed")
    ) {
        return Ok(());
    }
    let at = iso(now_ms());
    match event {
        ProviderEvent::Started => {
            run["status"] = json!("running");
            run["startedAt"] = json!(at);
        }
        ProviderEvent::TextDelta(delta) => {
            let mid = format!("assistant-{rid}");
            let mut message=get(&tx,"messages",&mid).unwrap_or_else(|_|json!({"id":mid,"threadId":tid,"runId":rid,"nodeId":null,"role":"assistant","text":"","attachments":[],"streaming":true,"createdBy":"agent","creationSource":"web","createdAt":at,"updatedAt":at}));
            let mut text = message["text"].as_str().unwrap_or("").to_owned();
            text.push_str(&delta);
            message["text"] = json!(text);
            message["updatedAt"] = json!(at);
            put(&tx, "messages", &mid, tid, &message)?;
            append(&tx, tid, "message.updated", message.clone(), &thread)?;
            let item = message_item(&tx, tid, &message)?;
            append(&tx, tid, "turn-item.updated", item, &thread)?;
        }
        ProviderEvent::ApprovalRequest {
            request_id,
            command: _,
        } => {
            let request = json!({"id":request_id,"nodeId":rid,"providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":{"type":"message"},"createdAt":at,"resolvedAt":null});
            put(&tx, "runtimeRequests", &request_id, tid, &request)?;
            append(&tx, tid, "runtime-request.updated", request, &thread)?;
            run["status"] = json!("waiting");
        }
        ProviderEvent::Completed | ProviderEvent::Failed(_) => {
            run["status"] = json!(if matches!(event, ProviderEvent::Completed) {
                "completed"
            } else {
                "failed"
            });
            run["completedAt"] = json!(at);
            let mid = format!("assistant-{rid}");
            if let Ok(mut message) = get(&tx, "messages", &mid) {
                message["streaming"] = json!(false);
                message["updatedAt"] = json!(at);
                put(&tx, "messages", &mid, tid, &message)?;
                append(&tx, tid, "message.updated", message.clone(), &thread)?;
                let item = message_item(&tx, tid, &message)?;
                append(&tx, tid, "turn-item.updated", item, &thread)?;
            }
        }
    }
    put(&tx, "runs", rid, tid, &run)?;
    append(&tx, tid, "run.updated", run, &thread)?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(())
}
