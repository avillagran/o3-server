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
    SteerContent(Vec<serde_json::Value>),
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
    fn discover<'a>(
        &'a self,
        _cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.refresh()
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.refresh()
    }
    fn discover_defaults<'a>(
        &'a self,
        cwd: std::path::PathBuf,
        _excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.discover(cwd)
    }
    fn poll_defaults<'a>(
        &'a self,
        _excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.poll_usage()
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
                        Some(ProviderControl::SteerContent(content))=>output.send(ProviderEvent::TextDelta(content[0]["text"].as_str().unwrap_or("").into())).await.map_err(|_|"receiver closed")?,
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
        let mut input = {
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
            let native_id = crate::projection::native_id(&db, &t);
            let fresh = native_id.is_none();
            let mut selection = t["modelSelection"].clone();
            if fresh {
                selection["nativeForkSource"] = t["nativeForkSource"].clone();
            }
            RunInput {
                provider_instance_id: t["providerInstanceId"].as_str().unwrap_or("").into(),
                native_session_id: native_id,
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
                    let mut media = if fresh && !t["nativeForkSource"].is_string() {
                        t["providerHandoffAttachments"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    media.extend(m["attachments"].as_array().into_iter().flatten().cloned());
                    serde_json::json!(media)
                },
                context: if r["recoveryPending"] == true {
                    None
                } else {
                    m.get("context").cloned()
                },
            }
        };
        // Internal config is issued here, never accepted from persisted model options.
        if let Some(selection) = input.model_selection.as_object_mut() {
            selection.remove("o3BrowserMcp");
        }
        let broker = if input.runtime_mode != "text-only" {
            crate::agent_browser::Broker::start(s.clone(), &tid).await
        } else {
            None
        };
        if let Some(broker) = &broker {
            input.model_selection["o3BrowserMcp"] = broker.config.clone();
        }
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
        let streaming = crate::source_control::effective(&s, &checkpoint_cwd)
            ["responseStreamingMode"]
            .as_str()
            .unwrap_or("paragraph")
            .to_owned();
        let mut buffered = String::new();
        let mut checkpoint_finished = false;
        let mut finished = false;
        let mut failure = None;
        let mut changed = s.changed.subscribe();
        loop {
            let event = tokio::select! {
                event = events.recv() => match event { Some(event) => event, None => break },
                _ = changed.changed() => {
                    if crate::events::get(&s.db.lock().unwrap(), "runs", &rid).is_ok_and(|r| matches!(r["status"].as_str(), Some("completed" | "failed" | "interrupted" | "cancelled"))) { break; }
                    continue;
                }
            };
            if let ProviderEvent::TextDelta(delta) = event {
                if buffered.len().saturating_add(delta.len()) > 1024 * 1024 {
                    failure = Some("assistant message exceeds 1 MiB".into());
                    break;
                }
                buffered.push_str(&delta);
                if streaming == "paragraph" {
                    if let Some(end) = buffered.rfind("\n\n") {
                        let tail = buffered.split_off(end + 2);
                        let text = std::mem::replace(&mut buffered, tail);
                        if persist(&s, &tid, &rid, ProviderEvent::TextDelta(text)).is_err() {
                            break;
                        }
                    }
                }
                continue;
            }
            let flush = !buffered.is_empty()
                && (streaming == "paragraph"
                    || matches!(
                        event,
                        ProviderEvent::Completed
                            | ProviderEvent::Failed(_)
                            | ProviderEvent::Interrupted
                    ));
            if flush
                && persist(
                    &s,
                    &tid,
                    &rid,
                    ProviderEvent::TextDelta(std::mem::take(&mut buffered)),
                )
                .is_err()
            {
                break;
            }
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
            let (ps, pt, pr) = (s.clone(), tid.clone(), rid.clone());
            if tokio::task::spawn_blocking(move || persist(&ps, &pt, &pr, event))
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
                .is_err()
            {
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
        if !buffered.is_empty() {
            let _ = persist(&s, &tid, &rid, ProviderEvent::TextDelta(buffered));
        }
        let _ = cancel.send(true);
        tokio::select! {r=&mut task=>{match r {Ok(Ok(()))=>finished=true,Ok(Err(e))=>failure=Some(e),Err(e)=>failure=Some(e.to_string())}},_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{task.abort();let _=task.await;}}
        if let Some(broker) = &broker {
            broker.close().await;
        }
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
        if crate::events::has_live_requests(&s, &tid, &rid).unwrap_or(false) {
            let state = s.clone();
            let (thread, run) = (tid.clone(), rid.clone());
            tokio::spawn(async move {
                let ms = state
                    .request_release_ms
                    .load(std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                if let Err(error) = crate::events::release_requests(&state, &thread, &run) {
                    eprintln!("runtime request release failed: {error}");
                }
            });
        }
        crate::thread_commands::advance(&s, &tid);
    });
}
/// A native acknowledgement may never arrive. Failure is explicit, never native success.
pub(crate) fn interrupt_deadline(s: std::sync::Arc<crate::store::State>, tid: String, rid: String) {
    interrupt_deadline_after(s, tid, rid, std::time::Duration::from_secs(75));
}
fn interrupt_deadline_after(
    s: std::sync::Arc<crate::store::State>,
    tid: String,
    rid: String,
    timeout: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut changed = s.changed.subscribe();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            if crate::events::get(&s.db.lock().unwrap(), "runs", &rid).is_ok_and(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("completed" | "failed" | "interrupted" | "cancelled")
                )
            }) {
                return;
            }
            tokio::select! {
                _ = &mut deadline => break,
                _ = changed.changed() => {},
            }
        }
        let _ = tokio::task::spawn_blocking(move || {
            let _ = persist(
                &s,
                &tid,
                &rid,
                ProviderEvent::Failed(
                    "Interrupt acknowledgement timed out; native delivery unknown".into(),
                ),
            );
        })
        .await;
    })
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
                let item_id = match &command {
                    C::Finished { item_id, .. } => item_id.as_str(),
                    _ => unreachable!(),
                };
                let owned:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='backgroundTasks' AND thread_id=? AND json_extract(body,'$.itemId')=? AND json_extract(body,'$.status')='running')",rusqlite::params![tid,item_id],|r|r.get(0)).map_err(|e|e.to_string())?;
                if owned {
                    return Ok(());
                }
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
    let settings = crate::settings::read(s)?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut thread = get(&tx, "thread", tid)?;
    let mut run = get(&tx, "runs", rid)?;
    if matches!(
        run["status"].as_str(),
        Some("interrupted" | "completed" | "failed" | "cancelled")
    ) {
        return Ok(());
    }
    let at = iso(now_ms());
    match event {
        ProviderEvent::Codex(frame) => {
            crate::codex_projection::persist(&tx, tid, rid, &thread, &mut run, &at, frame)?;
            thread = get(&tx, "thread", tid)?;
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
            let request = json!({"id":request_id,"nodeId":rid,"providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":{"type":"live"},"createdAt":at,"resolvedAt":null});
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
            if matches!(event, ProviderEvent::Failed(_)) {
                tx.execute("UPDATE records SET body=json_set(body,'$.status','delivery-unknown','$.error','provider ended without interrupt acknowledgement') WHERE kind='controlOutbox' AND thread_id=?1 AND json_extract(body,'$.command.runId')=?2 AND json_extract(body,'$.status') IN ('pending','handed-off')", rusqlite::params![tid,rid]).map_err(|e|e.to_string())?;
            }
            if matches!(event, ProviderEvent::Interrupted) {
                crate::events::interrupt_result(
                    &tx,
                    tid,
                    &run,
                    &thread,
                    &at,
                    "Run interrupted by user",
                    true,
                )?;
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
    if run["errorClass"] == "usage_limit"
        && run["status"] == "failed"
        && thread["usageLimitResetAt"].is_string()
    {
        let overrides =
            &settings["projectSettingsOverrides"][thread["projectId"].as_str().unwrap_or("")];
        let flag = |key: &str| overrides.get(key).unwrap_or(&settings[key]) == true;
        if flag("snoozeLimitedThreads") || flag("autoResumeLimitedThreads") {
            thread["limitRecovery"] =
                json!({"runId":rid,"resumeAt":thread["usageLimitResetAt"],"status":"limited"});
            if flag("snoozeLimitedThreads") && thread["snoozedUntil"].is_null() {
                thread["snoozedAt"] = json!(at);
                thread["snoozedUntil"] = thread["usageLimitResetAt"].clone();
                thread["limitRecovery"]["snoozed"] = json!(true);
            }
            put(&tx, "thread", tid, tid, &thread)?;
            append(&tx, tid, "thread.metadata-updated", thread.clone(), &thread)?;
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
    fn discover_defaults<'a>(
        &'a self,
        cwd: std::path::PathBuf,
        excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if !excluded.iter().any(|i| i == "codex") {
                let _ = self.codex.discover(cwd.clone()).await;
            }
            if !excluded.iter().any(|i| i == "claudeAgent") {
                if let Some(p) = &self.claude {
                    let _ = p.discover(cwd).await;
                }
            }
            Ok(())
        })
    }
    fn poll_defaults<'a>(
        &'a self,
        excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if !excluded.iter().any(|i| i == "codex") {
                let _ = self.codex.poll_usage().await;
            }
            if !excluded.iter().any(|i| i == "claudeAgent") {
                if let Some(p) = &self.claude {
                    let _ = p.poll_usage().await;
                }
            }
            Ok(())
        })
    }

    fn discover<'a>(
        &'a self,
        cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.codex.discover(cwd.clone()).await;
            if let Some(claude) = &self.claude {
                let _ = claude.discover(cwd).await;
            }
            Ok(())
        })
    }

    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.codex.poll_usage().await;
            if let Some(claude) = &self.claude {
                let _ = claude.poll_usage().await;
            }
            Ok(())
        })
    }

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
    #[tokio::test]
    async fn interrupt_timer_releases_terminal_run_immediately() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let s = std::sync::Arc::new(crate::store::State::open(&dir).unwrap());
        crate::events::put(
            &s.db.lock().unwrap(),
            "runs",
            "r",
            "t",
            &json!({"id":"r","status":"interrupted"}),
        )
        .unwrap();
        let task = interrupt_deadline_after(
            s.clone(),
            "t".into(),
            "r".into(),
            std::time::Duration::from_secs(75),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(std::sync::Arc::strong_count(&s), 1);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn missing_interrupt_acknowledgement_fails_with_unknown_delivery() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let s = std::sync::Arc::new(crate::store::State::open(&dir).unwrap());
        {
            let db = s.db.lock().unwrap();
            for (kind, id, v) in [
                (
                    "thread",
                    "t",
                    json!({"id":"t","providerInstanceId":"claude"}),
                ),
                (
                    "runs",
                    "r",
                    json!({"id":"r","threadId":"t","status":"running"}),
                ),
                (
                    "controlOutbox",
                    "stop",
                    json!({"id":"stop","status":"handed-off","command":{"runId":"r"}}),
                ),
            ] {
                crate::events::put(&db, kind, id, "t", &v).unwrap();
            }
        }
        let mut changed = s.changed.subscribe();
        interrupt_deadline_after(
            s.clone(),
            "t".into(),
            "r".into(),
            std::time::Duration::from_millis(5),
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), changed.changed())
            .await
            .unwrap()
            .unwrap();
        let db = s.db.lock().unwrap();
        assert_eq!(
            crate::events::get(&db, "runs", "r").unwrap()["status"],
            "failed"
        );
        assert_eq!(
            crate::events::get(&db, "controlOutbox", "stop").unwrap()["status"],
            "delivery-unknown"
        );
        assert!(crate::events::get(&db, "turnItems", "r-interrupt-result").is_err());
        drop(db);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn native_interruption_finalizes_result_once_with_claude_root_turn() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let s = crate::store::State::open(&dir).unwrap();
        {
            let db = s.db.lock().unwrap();
            for (kind, id, v) in [
                (
                    "thread",
                    "t",
                    json!({"id":"t","providerInstanceId":"claude"}),
                ),
                (
                    "runs",
                    "r",
                    json!({"id":"r","threadId":"t","status":"running","rootNodeId":"root","providerThreadId":"owner"}),
                ),
                (
                    "nodes",
                    "root",
                    json!({"id":"root","runId":"r","providerTurnId":"real-turn","status":"running"}),
                ),
            ] {
                crate::events::put(&db, kind, id, "t", &v).unwrap();
            }
        }
        persist(&s, "t", "r", ProviderEvent::Interrupted).unwrap();
        let db = s.db.lock().unwrap();
        assert_eq!(
            crate::events::get(&db, "runs", "r").unwrap()["status"],
            "interrupted"
        );
        let result = crate::events::get(&db, "turnItems", "r-interrupt-result").unwrap();
        assert_eq!(result["providerTurnId"], "real-turn");
        let seq = crate::events::sequence(&db).unwrap();
        drop(db);
        persist(&s, "t", "r", ProviderEvent::Interrupted).unwrap();
        assert_eq!(crate::events::sequence(&s.db.lock().unwrap()).unwrap(), seq);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
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

/// Instance registry. Each configured identity owns its own native process/session pool.
/// Replacing settings retires the old pool after its in-flight run releases it.
pub struct ConfiguredProviders {
    pub defaults: std::sync::Arc<dyn Provider>,
    pub directory: std::path::PathBuf,
    instances: std::sync::Mutex<std::collections::BTreeMap<String, Instance>>,
    discovered_defaults: std::sync::atomic::AtomicBool,
}
struct Instance {
    settings: serde_json::Value,
    discovered: bool,
    provider: Option<std::sync::Arc<dyn Provider>>,
    binary: std::path::PathBuf,
}
impl ConfiguredProviders {
    pub fn new(defaults: std::sync::Arc<dyn Provider>, directory: std::path::PathBuf) -> Self {
        Self {
            defaults,
            directory,
            instances: Default::default(),
            discovered_defaults: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn settings(&self) -> serde_json::Value {
        std::fs::read(self.directory.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    }
    fn sync(&self) -> serde_json::Value {
        use serde_json::json;
        let settings = self.settings();
        let mut cache = self.instances.lock().unwrap();
        cache.retain(|id, _| settings["providerInstances"].get(id).is_some());
        for (id, entry) in settings["providerInstances"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if cache.get(id).is_some_and(|i| i.settings == *entry) {
                continue;
            }
            let driver = entry["driver"].as_str().unwrap_or("");
            let name = if driver == "claudeAgent" {
                "claude"
            } else {
                driver
            };
            let binary = instance_binary(entry, name);
            let env = instance_environment(entry);
            let home = env
                .iter()
                .find(|(k, _)| {
                    k == if driver == "codex" {
                        "CODEX_HOME"
                    } else {
                        "CLAUDE_CONFIG_DIR"
                    }
                })
                .map(|(_, v)| std::path::PathBuf::from(v));
            let args = launch_args(entry["config"]["launchArgs"].as_str().unwrap_or(""))
                .unwrap_or_default();
            let provider: Option<std::sync::Arc<dyn Provider>> = match driver {
                "codex" => Some(std::sync::Arc::new(
                    crate::codex::CodexProvider::new(binary.clone(), home)
                        .with_environment(env)
                        .with_launch_args(args.clone()),
                )),
                "claudeAgent" => Some(std::sync::Arc::new(
                    crate::claude::ClaudeProvider::new(binary.clone())
                        .with_environment(env)
                        .with_launch_args(args.clone()),
                )),
                _ => None,
            };
            cache.insert(
                id.clone(),
                Instance {
                    settings: entry.clone(),
                    discovered: false,
                    provider,
                    binary,
                },
            );
        }
        json!(settings)
    }
}
impl Provider for ConfiguredProviders {
    fn discover<'a>(
        &'a self,
        cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            if !self
                .discovered_defaults
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                let excluded = self.instances.lock().unwrap().keys().cloned().collect();
                let _ = self.defaults.discover_defaults(cwd.clone(), excluded).await;
            }
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values_mut()
                .filter_map(|i| {
                    if i.discovered || i.settings["enabled"] == false {
                        return None;
                    }
                    i.discovered = true;
                    i.provider.clone()
                })
                .collect();
            for p in providers {
                let _ = p.discover(cwd.clone()).await;
            }
            Ok(())
        })
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            let settings = self.settings();
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values()
                .filter(|i| i.settings["enabled"] != false)
                .filter_map(|i| i.provider.clone())
                .collect();
            let excluded = settings["providerInstances"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(id, _)| id.clone())
                .collect();
            let _ = self.defaults.poll_defaults(excluded).await;
            for p in providers {
                let _ = p.poll_usage().await;
            }
            Ok(())
        })
    }

    fn config(&self) -> Vec<serde_json::Value> {
        use serde_json::json;
        self.sync();
        let instances = self.instances.lock().unwrap();
        let mut result = self.defaults.config();
        for (id, instance) in instances.iter() {
            result.retain(|p| p["instanceId"] != *id);
            let mut descriptor = instance.provider.as_ref().and_then(|p| p.config().into_iter().next()).unwrap_or_else(|| json!({"models":[],"slashCommands":[],"skills":[],"status":"disabled","installed":false}));
            descriptor["installed"] =
                serde_json::json!(crate::executable::resolve(&instance.binary).is_some());
            if descriptor["installed"] == false {
                descriptor["status"] = serde_json::json!("disabled");
            }
            descriptor["instanceId"] = json!(id);
            descriptor["driver"] = instance.settings["driver"].clone();
            if let Some(color) = instance.settings.get("accentColor") {
                descriptor["accentColor"] = color.clone();
            }
            descriptor["displayName"] = instance
                .settings
                .get("displayName")
                .cloned()
                .unwrap_or_else(|| json!(id));
            descriptor["enabled"] = json!(
                instance.settings["enabled"] != false
                    && instance.settings["config"]["enabled"] != false
            );
            if descriptor["enabled"] == false {
                descriptor["status"] = json!("disabled");
            }
            result.push(descriptor);
        }
        result
    }
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            self.defaults.refresh().await?;
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values()
                .filter_map(|i| i.provider.clone())
                .collect();
            for p in providers {
                let _ = p.refresh().await;
            }
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
        self.sync();
        let selected = {
            let instances = self.instances.lock().unwrap();
            instances
                .get(
                    if matches!(
                        input.provider_instance_id.as_str(),
                        "claude" | "claude-code"
                    ) {
                        "claudeAgent"
                    } else {
                        &input.provider_instance_id
                    },
                )
                .map(|i| {
                    if i.settings["enabled"] == false || i.settings["config"]["enabled"] == false {
                        Err("provider instance is disabled".to_owned())
                    } else if crate::executable::resolve(&i.binary).is_none() {
                        Err("provider binary is unavailable".to_owned())
                    } else {
                        i.provider
                            .clone()
                            .ok_or_else(|| "unsupported provider driver".to_owned())
                    }
                })
        };
        match selected {
            None => self.defaults.execute(input, output, control, cancel),
            Some(selected) => {
                Box::pin(async move { selected?.execute(input, output, control, cancel).await })
            }
        }
    }
}

/// Shell-style argument words, without shell evaluation or command substitution.
pub(crate) fn launch_args(text: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut started = false;
    for c in text.chars() {
        if escape {
            word.push(c);
            escape = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            started = true;
            continue;
        }
        if quote == Some(c) {
            quote = None;
            continue;
        }
        if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
            started = true;
            continue;
        }
        if c.is_whitespace() && quote.is_none() {
            if started {
                result.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if escape || quote.is_some() {
        return Err("unterminated provider launch arguments".into());
    }
    if started {
        result.push(word);
    }
    Ok(result)
}

pub(crate) fn instance_environment(entry: &serde_json::Value) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = entry["environment"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["name"].as_str()?.into(), v["value"].as_str()?.into())))
        .collect();
    let key = if entry["driver"] == "codex" {
        "CODEX_HOME"
    } else {
        "CLAUDE_CONFIG_DIR"
    };
    if let Some(home) = entry["config"]
        .get("shadowHomePath")
        .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        .unwrap_or(&entry["config"]["homePath"])
        .as_str()
        .filter(|s| !s.is_empty())
    {
        env.retain(|(k, _)| k != key);
        env.push((key.into(), expand_home(home).to_string_lossy().into_owned()));
    }
    env
}
pub(crate) fn expand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    path.into()
}
pub(crate) fn instance_binary(entry: &serde_json::Value, name: &str) -> std::path::PathBuf {
    let text = entry["config"]["binaryPath"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(name);
    let path = expand_home(text);
    if path.components().count() == 1 {
        if let Some(paths) = instance_environment(entry)
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v)
        {
            return std::env::split_paths(paths)
                .find_map(|dir| crate::executable::resolve(&dir.join(&path)))
                .unwrap_or(path);
        }
    }
    crate::executable::resolve(&path).unwrap_or(path)
}

#[cfg(test)]
mod limit_policy_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exhausted_native_turn_snoozes_only_when_enabled() {
        for enabled in [false, true] {
            let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
            let s = crate::store::State::open(&dir).unwrap();
            crate::settings::update(&s, &json!({"patch":{"snoozeLimitedThreads":enabled}}))
                .unwrap();
            {
                let db = s.db.lock().unwrap();
                crate::events::put(
                    &db,
                    "thread",
                    "t",
                    "t",
                    &json!({"id":"t","projectId":"p","providerInstanceId":"claudeAgent"}),
                )
                .unwrap();
                crate::events::put(
                    &db,
                    "runs",
                    "r",
                    "t",
                    &json!({"id":"r","status":"running","ordinal":1}),
                )
                .unwrap();
            }
            persist(&s,"t","r",ProviderEvent::Native(json!({"type":"rate_limit","info":{"status":"rejected","resetsAt":(crate::store::now_ms()+60000)/1000}}))).unwrap();
            persist(&s, "t", "r", ProviderEvent::Failed("usage limit".into())).unwrap();
            let t = crate::events::get(&s.db.lock().unwrap(), "thread", "t").unwrap();
            assert_eq!(t["snoozedUntil"].is_string(), enabled);
            assert_eq!(t["limitRecovery"].is_object(), enabled);
            drop(s);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
