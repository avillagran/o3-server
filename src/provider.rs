//! Adapter boundary: local asynchronous tasks, bounded output, explicit cancellation.
use std::{future::Future, pin::Pin};
use tokio::sync::{mpsc, watch};
pub struct RunInput {
    pub native_session_id: Option<String>,
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
    Codex(serde_json::Value),
    Started,
    Native(serde_json::Value),
    TextDelta(String),
    ApprovalRequest {
        request_id: String,
        command: String,
    },
    Completed,
    Interrupted,
    Failed(String),
    /// A command item and its transient output; see `command_output`.
    Command(crate::command_output::CommandEvent),
}
pub enum ProviderControl {
    Steer(String),
    Interrupt,
    Respond {
        request_id: String,
        decision: serde_json::Value,
        feedback: Option<String>,
        answers: Option<serde_json::Value>,
    },
}
/// Implementations must await output capacity, stop on cancellation/receiver drop,
/// and reap owned per-run subprocesses before resolving. Shared process adapters expose
/// an explicit shutdown that reaps the process after all runs. Futures must be Send.
pub trait Provider: Send + Sync + 'static {
    fn config(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
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
            loop {
                tokio::select! {
                    c=control.recv()=>match c {
                        Some(ProviderControl::Respond{..})=>return output.send(ProviderEvent::Completed).await.map_err(|_|"receiver closed".into()),
                        Some(ProviderControl::Steer(text))=>output.send(ProviderEvent::TextDelta(text)).await.map_err(|_|"receiver closed")?,
                        _=>return Ok(()),
                    },
                    _=cancel.changed()=>return Ok(()),
                }
            }
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
                native_session_id: t["activeProviderThreadId"].as_str().map(str::to_owned),
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
        let mut failure = None;
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
        tokio::select! {r=&mut task=>{match r {Ok(Ok(()))=>finished=true,Ok(Err(e))=>failure=Some(e),Err(e)=>failure=Some(e.to_string())}},_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{task.abort();let _=task.await;}}
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
                    ProviderEvent::Failed(failure.unwrap_or_else(|| {
                        if finished {
                            "provider ended without completion"
                        } else {
                            "provider failed"
                        }
                        .into()
                    })),
                );
            }
        }
        crate::thread_commands::advance(&s, &tid);
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
    if let ProviderEvent::Command(command) = event {
        return crate::command_output::persist(s, tid, rid, command);
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut thread = get(&tx, "thread", tid)?;
    let mut run = get(&tx, "runs", rid)?;
    if matches!(
        run["status"].as_str(),
        Some("interrupted" | "completed" | "failed")
    ) {
        return Ok(());
    }
    let at = iso(now_ms());
    match event {
        ProviderEvent::Codex(frame) => {
            crate::codex_projection::persist(&tx, tid, rid, &thread, &mut run, &at, frame)?;
        }
        ProviderEvent::Native(frame) => {
            crate::claude_projection::persist(&tx, tid, rid, &mut thread, &mut run, &at, &frame)?;
        }
        ProviderEvent::Started => {
            run["status"] = json!("running");
            run["startedAt"] = json!(at);
        }
        ProviderEvent::TextDelta(delta) => {
            let mid = format!("assistant-{rid}");
            let mut message=get(&tx,"messages",&mid).unwrap_or_else(|_|json!({"id":mid,"threadId":tid,"runId":rid,"nodeId":null,"role":"assistant","text":"","attachments":[],"streaming":true,"createdBy":"agent","creationSource":"web","createdAt":at,"updatedAt":at}));
            message["nodeId"] = run["rootNodeId"].clone();
            message["providerThreadId"] = run["providerThreadId"].clone();
            message["providerTurnId"] = run["codexProviderTurnId"].clone();
            let mut text = message["text"].as_str().unwrap_or("").to_owned();
            if text.len().saturating_add(delta.len()) > 1024 * 1024 {
                return Err("assistant message exceeds 1 MiB".into());
            }
            text.push_str(&delta);
            if text.len() > 1024 * 1024 {
                return Err("assistant message exceeds 1 MiB".into());
            }
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
        ProviderEvent::Command(_) => unreachable!(),
        ProviderEvent::Completed | ProviderEvent::Interrupted | ProviderEvent::Failed(_) => {
            run["status"] = json!(if matches!(event, ProviderEvent::Completed) {
                "completed"
            } else if matches!(event, ProviderEvent::Interrupted) {
                "interrupted"
            } else {
                "failed"
            });
            run["completedAt"] = json!(at);
            if let ProviderEvent::Failed(ref error) = event {
                run["error"] = json!(error);
            }
            crate::claude_projection::finish(
                &tx,
                tid,
                rid,
                &thread,
                &at,
                run["status"].as_str().unwrap_or("failed"),
            )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn real_frames_reduce_to_renderable_items_and_native_identity() {
        for (name, fixture) in [
            (
                "permission",
                include_str!("../tests/fixtures/claude/permission.jsonl"),
            ),
            (
                "subagent",
                include_str!("../tests/fixtures/claude/subagent.jsonl"),
            ),
        ] {
            let dir = std::env::temp_dir()
                .join(format!("o3-replay-{}", crate::store::random_id().unwrap()));
            let s = crate::store::State::open(&dir).unwrap();
            {
                let db = s.db.lock().unwrap();
                crate::events::put(&db,"thread","t","t",&json!({"id":"t","projectId":"p","modelSelection":{"instanceId":"claude","model":"haiku"},"providerInstanceId":"claude"})).unwrap();
                crate::events::put(
                    &db,
                    "runs",
                    "r",
                    "t",
                    &json!({"id":"r","threadId":"t","ordinal":1,"status":"preparing"}),
                )
                .unwrap();
            }
            for line in fixture.lines() {
                let v: Value = serde_json::from_str(line).unwrap();
                if v["type"] == "control_request" {
                    persist(&s,"t","r",ProviderEvent::Native(json!({"type":"permission","request_id":"r:approval","native_id":v["request_id"],"request":v["request"]}))).unwrap();
                }
                for event in crate::claude::decode(&v) {
                    persist(&s, "t", "r", event).unwrap();
                }
            }
            let db = s.db.lock().unwrap();
            assert_eq!(
                crate::events::get(&db, "runs", "r").unwrap()["status"],
                "completed"
            );
            let thread = crate::events::get(&db, "thread", "t").unwrap();
            assert!(thread["activeProviderThreadId"].is_string());
            let turn = crate::events::get(&db, "providerTurns", "r").unwrap();
            assert_eq!(turn["status"], "completed");
            assert!(turn["tokenUsage"]["outputTokens"].as_u64().unwrap() > 0);
            let items: Vec<Value> = db
                .prepare("SELECT body FROM records WHERE kind='turnItems'")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
                .collect();
            assert!(items.iter().any(|i| i["type"] == "assistant_message"));
            if name == "permission" {
                assert!(items.iter().any(|i| i["type"] == "approval_request"));
                assert!(items
                    .iter()
                    .any(|i| i["type"] == "dynamic_tool" && i["toolName"] == "Write"));
            } else {
                assert!(items
                    .iter()
                    .any(|i| i["type"] == "reasoning" && i["streaming"] == false));
                assert!(items
                    .iter()
                    .any(|i| i["type"] == "subagent" && i["status"] == "completed"));
            }
            drop(db);
            drop(s);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
