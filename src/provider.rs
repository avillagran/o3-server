//! Adapter boundary: local asynchronous tasks, bounded output, explicit cancellation.
use std::{future::Future, pin::Pin};
use tokio::sync::{mpsc, watch};
pub struct RunInput {
    pub provider_instance_id: String,
    pub native_session_id: Option<String>,
    pub thread_id: String,
    pub run_id: String,
    pub cwd: std::path::PathBuf,
    pub text: String,
    pub model_selection: serde_json::Value,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub attachment_root: std::path::PathBuf,
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
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
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
                .unwrap_or_else(|| s.dir.clone());
            let native_key = if t["providerInstanceId"] == "codex" {
                "codexNativeThreadId"
            } else {
                "activeProviderThreadId"
            };
            let fresh = !t[native_key].is_string();
            let mut selection = t["modelSelection"].clone();
            if fresh {
                selection["nativeForkSource"] = t["nativeForkSource"].clone();
            }
            RunInput {
                provider_instance_id: t["providerInstanceId"].as_str().unwrap_or("").into(),
                native_session_id: t[if t["providerInstanceId"] == "codex" {
                    "codexNativeThreadId"
                } else {
                    "activeProviderThreadId"
                }]
                .as_str()
                .map(str::to_owned),
                thread_id: format!("{}:{}", tid, t["providerGeneration"].as_str().unwrap_or("")),
                run_id: rid.clone(),
                cwd,
                text: if fresh && !t["nativeForkSource"].is_string() {
                    t["providerHandoff"]
                        .as_str()
                        .map(|h| format!("{h}\nCurrent user request:\n{text}"))
                        .unwrap_or(text)
                } else {
                    text
                },
                model_selection: selection,
                runtime_mode: t["runtimeMode"].as_str().unwrap_or("full-access").into(),
                interaction_mode: t["interactionMode"].as_str().unwrap_or("default").into(),
                attachment_root: crate::assets::attachments_dir(&s),
                attachments: if r["recoveryPending"] == true {
                    serde_json::json!([])
                } else {
                    m["attachments"].clone()
                },
                context: if r["recoveryPending"] == true {
                    None
                } else {
                    m.get("context").cloned()
                },
            }
        };
        let checkpoint_cwd = input.cwd.clone();
        let checkpoint_state = s.clone();
        let ct = tid.clone();
        let cr = rid.clone();
        let cc = checkpoint_cwd.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::checkpoint::capture(&checkpoint_state, &ct, &cr, &cc, true)
        })
        .await;
        let mut task = tokio::task::spawn(async move {
            provider
                .execute(input, output, controls, cancellation)
                .await
        });
        let mut checkpoint_finished = false;
        let mut finished = false;
        let mut failure = None;
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                ProviderEvent::Completed | ProviderEvent::Interrupted | ProviderEvent::Failed(_)
            ) {
                checkpoint_finished = true;
                let cs = s.clone();
                let ct = tid.clone();
                let cr = rid.clone();
                let cc = checkpoint_cwd.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::checkpoint::capture(&cs, &ct, &cr, &cc, false)
                })
                .await;
            }
            let request_id = match &event {
                ProviderEvent::Native(f) if f["type"] == "permission" => {
                    f["request_id"].as_str().map(str::to_owned)
                }
                ProviderEvent::Codex(f) => f["params"]["o3RequestId"].as_str().map(str::to_owned),
                _ => None,
            };
            if persist(&s, &tid, &rid, event).is_err() {
                break;
            }
            if let Some(id) = request_id {
                let granted = {
                    let db = s.db.lock().unwrap();
                    let request =
                        crate::events::get(&db, "runtimeRequests", &id).unwrap_or_default();
                    let thread = crate::events::get(&db, "thread", &tid).unwrap_or_default();
                    !request["approvalPolicyKey"].is_null() && db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM records WHERE kind='approvalPolicies' AND thread_id=? AND json_extract(body,'$.key')=json(?) AND json_extract(body,'$.providerInstanceId')=? AND json_extract(body,'$.worktreePath') IS json_extract(?,'$.worktreePath') AND json_extract(body,'$.projectId') IS json_extract(?,'$.projectId'))",
                        rusqlite::params![tid,request["approvalPolicyKey"].to_string(),thread["providerInstanceId"].as_str().unwrap_or(""), thread.to_string(), thread.to_string()],|r|r.get::<_,bool>(0)).unwrap_or(false)
                };
                if granted {
                    let _ = crate::events::dispatch(
                        &s,
                        &serde_json::json!({"type":"runtime-request.respond","commandId":format!("auto-approval-{rid}-{id}"),"threadId":tid,"requestId":id,"decision":"accept"}),
                    );
                }
            }
            let interrupted = crate::events::get(&s.db.lock().unwrap(), "runs", &rid)
                .is_ok_and(|r| r["status"] == "interrupted");
            if interrupted {
                break;
            }
        }
        let _ = cancel.send(true);
        tokio::select! {r=&mut task=>{match r {Ok(Ok(()))=>finished=true,Ok(Err(e))=>failure=Some(e),Err(e)=>failure=Some(e.to_string())}},_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{task.abort();let _=task.await;}}
        if !checkpoint_finished {
            let cs = s.clone();
            let ct = tid.clone();
            let cr = rid.clone();
            let cc = checkpoint_cwd.clone();
            let _ = tokio::task::spawn_blocking(move || {
                crate::checkpoint::capture(&cs, &ct, &cr, &cc, false)
            })
            .await;
        }
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
    if let ProviderEvent::Command(mut command) = event {
        use crate::command_output::CommandEvent as C;
        let id = match &mut command {
            C::Started { item_id, .. }
            | C::Output { item_id, .. }
            | C::Finished { item_id, .. } => item_id,
        };
        if let Some(native) = id.strip_prefix("native-tool:") {
            *id = format!("tool-{rid}-{native}");
            if matches!(command, C::Finished { .. }) {
                let db = s.db.lock().unwrap();
                if crate::events::get(
                    &db,
                    "turnItems",
                    match &command {
                        C::Finished { item_id, .. } => item_id,
                        _ => unreachable!(),
                    },
                )
                .is_ok_and(|v| v["type"] != "command_execution")
                {
                    return Ok(());
                }
            }
        }
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
            run["recoveryPending"] = json!(false);
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
            crate::projection::finish(
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

/// Routes sessions to a driver without imposing subprocess ownership on adapters.
pub struct Providers {
    pub claude: Option<std::sync::Arc<crate::claude::ClaudeProvider>>,
    pub codex: std::sync::Arc<crate::codex::CodexProvider>,
}
impl Provider for Providers {
    fn config(&self) -> Vec<serde_json::Value> {
        let mut result = self
            .claude
            .as_ref()
            .map(|p| p.config())
            .unwrap_or_else(|| vec![crate::claude::config(false)]);
        result.extend(self.codex.config());
        result
    }
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.codex.probe().await;
            Ok(())
        })
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        control: mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        match input.provider_instance_id.as_str() {
            "codex" => self.codex.execute(input, output, control, cancel),
            "claudeAgent" | "claude" | "claude-code" => match self.claude.as_ref() {
                Some(p) => p.execute(input, output, control, cancel),
                None => Box::pin(async { Err("Claude CLI is unavailable".into()) }),
            },
            _ => Box::pin(async { Err("unknown providerInstanceId".into()) }),
        }
    }
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

#[cfg(test)]
mod command_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn claude_bash_frames_feed_command_output_without_reclassifying_other_tools() {
        let dir =
            std::env::temp_dir().join(format!("o3-command-{}", crate::store::random_id().unwrap()));
        let state = crate::store::State::open(&dir).unwrap();
        {
            let db = state.db.lock().unwrap();
            crate::events::put(
                &db,
                "thread",
                "t",
                "t",
                &json!({"id":"t","providerInstanceId":"claude"}),
            )
            .unwrap();
            crate::events::put(&db, "runs", "r", "t", &json!({"id":"r","status":"running"}))
                .unwrap();
        }
        for block in [
            json!({"type":"tool_use","id":"bash","name":"Bash","input":{"command":"echo hello"}}),
            json!({"type":"tool_result","tool_use_id":"bash","content":"hello\n"}),
            json!({"type":"tool_use","id":"read","name":"Read","input":{"file_path":"proof"}}),
            json!({"type":"tool_result","tool_use_id":"read","content":"file"}),
        ] {
            for event in
                crate::claude::decode(&json!({"type":"assistant","message":{"content":[block]}}))
            {
                persist(&state, "t", "r", event).unwrap();
            }
        }
        let mut stream = crate::command_output::open_stream(
            &state,
            &json!({"threadId":"t","itemId":"tool-r-bash"}),
        )
        .unwrap();
        let crate::terminal::Step::Values(values) = stream.next(&state) else {
            panic!("missing command output")
        };
        assert_eq!(values[0]["text"], "hello\n");
        let db = state.db.lock().unwrap();
        assert_eq!(
            crate::events::get(&db, "turnItems", "tool-r-read").unwrap()["type"],
            "dynamic_tool"
        );
        drop(db);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use crate::{events, store::State};
    use serde_json::json;
    use std::sync::Arc;
    struct Probe(mpsc::Sender<Option<String>>);
    impl Provider for Probe {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            _output: mpsc::Sender<ProviderEvent>,
            _control: mpsc::Receiver<ProviderControl>,
            _cancel: watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async move {
                self.0
                    .send(input.native_session_id)
                    .await
                    .map_err(|e| e.to_string())
            })
        }
    }
    #[tokio::test]
    async fn idle_restart_supplies_durable_native_ids_to_both_adapters() {
        for (instance, key) in [
            ("claudeAgent", "activeProviderThreadId"),
            ("codex", "codexNativeThreadId"),
        ] {
            let dir = std::env::temp_dir()
                .join(format!("o3-resume-{}", crate::store::random_id().unwrap()));
            let state = State::open(&dir).unwrap();
            let mut thread = json!({"id":"t","providerInstanceId":instance});
            thread[key] = json!("native-conversation");
            events::put(&state.db.lock().unwrap(), "thread", "t", "t", &thread).unwrap();
            drop(state);
            let state = Arc::new(State::open(&dir).unwrap());
            crate::recovery::recover(&mut state.db.lock().unwrap()).unwrap();
            let (tx, mut rx) = mpsc::channel(1);
            *state.provider.lock().unwrap() = Some(Arc::new(Probe(tx)));
            start(state.clone(), "t".into(), "r".into(), "next message".into());
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .as_deref(),
                Some("native-conversation")
            );
            while !state.active.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
            drop(state);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
