//! Adapter boundary: local asynchronous tasks, bounded output, explicit cancellation.
use std::{future::Future, pin::Pin};
use tokio::sync::{mpsc, watch};
#[derive(Default)]
pub(crate) struct Executions(std::sync::Mutex<std::collections::BTreeMap<String, Execution>>);
#[derive(Clone)]
struct Execution {
    thread: String,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}
struct ExecutionGuard {
    state: std::sync::Arc<crate::store::State>,
    run: String,
    done: watch::Sender<bool>,
}
impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.state.active.lock().unwrap().remove(&self.run);
        self.state.executions.0.lock().unwrap().remove(&self.run);
        self.done.send_replace(true);
        self.state.notify();
    }
}
impl Executions {
    pub(crate) fn cancel_run(&self, run: &str) {
        if let Some(run) = self.0.lock().unwrap().get(run) {
            run.cancel.send_replace(true);
        }
    }
    pub(crate) async fn wait_run(&self, run: &str) {
        let run = self.0.lock().unwrap().get(run).cloned();
        if let Some(mut run) = run {
            while !*run.done.borrow() {
                if run.done.changed().await.is_err() {
                    break;
                }
            }
        }
    }
    pub(crate) fn cancel(&self, thread: &str) {
        for r in self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.thread == thread)
        {
            r.cancel.send_replace(true);
        }
    }
    pub(crate) async fn stopped(&self, thread: &str) -> Result<(), String> {
        let runs: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.thread == thread)
            .cloned()
            .collect();
        for mut run in runs {
            run.cancel.send_replace(true);
            while !*run.done.borrow() {
                if run.done.changed().await.is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
}
pub struct RunInput {
    pub native_mcp: Option<std::sync::Arc<crate::native_mcp::Context>>,
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
#[derive(Clone, Copy)]
pub enum NativeRecoveryReason {
    UncertainHistory,
    ResumeFailed,
    ForkFailed,
    HandoffBudget,
}
impl NativeRecoveryReason {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UncertainHistory => "uncertain_history_delivery",
            Self::ResumeFailed => "resume_failed",
            Self::ForkFailed => "native_fork_failed",
            Self::HandoffBudget => "handoff_budget_exhausted",
        }
    }
}
pub enum ProviderEvent {
    RecoverNative {
        current_text: String,
        previous_native: Option<String>,
        reason: NativeRecoveryReason,
        reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
    },
    NativeRecoveryReleased {
        id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    ContextDelivery {
        data: serde_json::Value,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
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
    /// Structured native failure fields; normalized before durable publication.
    FailedWithDetails(serde_json::Value),
    /// A command item and its transient output; see `command_output`.
    Command(crate::command_output::CommandEvent),
}
impl ProviderEvent {
    pub(crate) fn failure_input(&self) -> Option<serde_json::Value> {
        match self {
            Self::Failed(message) => Some(serde_json::json!({"message":message})),
            Self::FailedWithDetails(failure) => Some(if failure.is_object() {
                failure.clone()
            } else {
                serde_json::json!({"cause":failure})
            }),
            _ => None,
        }
    }
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
    /// Cached capabilities only: called while the command transaction owns the database.
    fn capabilities(&self, instance: &str) -> Option<serde_json::Value> {
        crate::selection_transition::native_capabilities(instance)
    }
    fn plan_selection_transition(
        &self,
        _instance: &str,
        _current: &serde_json::Value,
        _target: &serde_json::Value,
        _capabilities: &serde_json::Value,
    ) -> Result<Option<crate::selection_transition::Selection>, String> {
        Ok(None)
    }

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
    fn reload_secrets<'a>(
        &'a self,
        _instance: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn refresh_defaults<'a>(
        &'a self,
        _excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
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
    fn detach<'a>(
        &'a self,
        _instance: &'a str,
        _key: &'a str,
        _native: Option<&'a str>,
        _revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    /// Dispose one builtin scope without ending other instances served by this router.
    fn dispose_instance<'a>(
        &'a self,
        _instance: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        self.dispose()
    }
    /// Close native resources owned by this instance scope before a replacement opens.
    fn dispose<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
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
    fn capabilities(&self, instance: &str) -> Option<serde_json::Value> {
        (instance == "scripted").then(|| serde_json::json!({}))
    }
    fn plan_selection_transition(
        &self,
        _instance: &str,
        _current: &serde_json::Value,
        _target: &serde_json::Value,
        _capabilities: &serde_json::Value,
    ) -> Result<Option<crate::selection_transition::Selection>, String> {
        Ok(crate::selection_transition::turn_scoped())
    }

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
    if s.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let denied = if s.managed_policy.document.is_some() {
        let db = s.db.lock().unwrap();
        let thread = crate::events::get(&db, "thread", &tid).unwrap_or_default();
        let run = crate::events::get(&db, "runs", &rid).unwrap_or_default();
        let instance = run["providerInstanceId"]
            .as_str()
            .or_else(|| thread["providerInstanceId"].as_str())
            .unwrap_or("");
        let mode = run["executionPolicy"]["runtimeMode"]
            .as_str()
            .or_else(|| thread["runtimeMode"].as_str())
            .unwrap_or("full-access");
        crate::managed_policy::check_command(
            &s,
            &db,
            &serde_json::json!({
                "type":"message.dispatch","threadId":tid,"runtimeMode":mode,
                "modelSelection":{"instanceId":instance}
            }),
        )
        .err()
        .map(|reason| (reason, run["activeAttemptId"].as_str().map(str::to_owned)))
    } else {
        None
    };
    if let Some((reason, attempt)) = denied {
        let _ = persist_attempt(
            &s,
            &tid,
            &rid,
            attempt.as_deref(),
            ProviderEvent::Failed(reason),
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                crate::thread_commands::advance(&s, &tid);
            });
        }
        return;
    }
    if let Err(error) = crate::command_shared::provider_fence(&s, &rid) {
        crate::command_shared::defer_provider(&s, &tid, &rid, &error);
        return;
    }
    let Some(provider) = s.provider.lock().unwrap().clone() else {
        return;
    };
    let (output, mut events) = mpsc::channel(16);
    let (control, controls) = mpsc::channel(8);
    let (cancel, cancellation) = watch::channel(false);
    let (done, finished) = watch::channel(false);
    s.executions.0.lock().unwrap().insert(
        rid.clone(),
        Execution {
            thread: tid.clone(),
            cancel: cancel.clone(),
            done: finished,
        },
    );
    s.active.lock().unwrap().insert(rid.clone(), control);
    let guard = ExecutionGuard {
        state: s.clone(),
        run: rid.clone(),
        done,
    };
    let mut stopped = cancellation.clone();
    tokio::task::spawn(async move {
        let _guard = guard;

        let (mut input, attempt) = {
            let db = s.db.lock().unwrap();
            let t = crate::events::get(&db, "thread", &tid).unwrap_or_default();
            let r = crate::events::get(&db, "runs", &rid).unwrap_or_default();
            if !t["deletedAt"].is_null()
                || crate::workspace_transition::pending(&db, &tid).unwrap_or(true)
                || matches!(
                    r["status"].as_str(),
                    Some("cancelled" | "completed" | "failed" | "interrupted" | "rolled_back")
                )
            {
                return;
            }

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
            let mut selection = r
                .get("modelSelection")
                .filter(|v| v.is_object())
                .unwrap_or(&t["modelSelection"])
                .clone();
            if let Some(options) = selection.as_object_mut() {
                options.remove("o3ColdCompact");
                options.remove("o3HistoricalContext");
                for key in [
                    "o3TransferHandoffs",
                    "nativeForkSource",
                    "nativeForkLastTurnId",
                    "o3NewSessionId",
                    "o3HandoffError",
                    "o3DeliveredHistoryItemIds",
                    "o3HandoffBudget",
                    "o3ForceNewNativeThread",
                ] {
                    options.remove(key);
                }
            }
            if fresh {
                selection["nativeForkSource"] = t["nativeForkSource"].clone();
                selection["nativeForkLastTurnId"] = t["nativeForkLastTurnId"].clone();
            }
            let cold_compact = fresh
                && crate::provider_input::driver(&s, &t["providerInstanceId"]).as_deref()
                    != Ok("codex")
                && !t["nativeForkSource"].is_string()
                && t["providerHandoff"].is_string()
                && text.trim() == "/compact"
                && m["attachments"].as_array().is_some_and(|a| a.is_empty());
            if cold_compact {
                selection["o3ColdCompact"] = serde_json::json!(true);
            }
            // A Codex binding is published before turn/start is accepted. A rejected
            // first input leaves a native ID but has not consumed the handoff.
            let codex_unseeded = crate::provider_input::driver(&s, &t["providerInstanceId"]).as_deref() == Ok("codex")
                && !db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=?1 AND json_extract(body,'$.providerThreadId')=?2 AND ((kind='runs' AND json_type(body,'$.codexProviderTurnId')='text') OR (kind='providerTurns' AND json_type(body,'$.nativeTurnRef.nativeId')='text')))", rusqlite::params![tid,r["providerThreadId"].as_str()], |row|row.get::<_,bool>(0)).unwrap_or(false);
            let rejected_binding = codex_unseeded && t["providerHandoff"].is_string()
                && db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='runs' AND thread_id=?1 AND json_extract(body,'$.providerThreadId')=?2 AND json_extract(body,'$.status')='failed' AND (json_extract(body,'$.error') LIKE '%input_too_large%' OR json_extract(body,'$.error') LIKE '%Input exceeds the maximum length%' OR json_extract(body,'$.error') LIKE '%Codex outgoing frame exceeds limit%'))", rusqlite::params![tid,r["providerThreadId"].as_str()], |row|row.get::<_,bool>(0)).unwrap_or(false);
            if (fresh || codex_unseeded) && !t["nativeForkSource"].is_string() {
                if let Some(history) = t["providerHandoff"].as_str() {
                    selection["o3HistoricalContext"] =
                        serde_json::json!(crate::provider_input::historical_context(
                            &crate::assets::attachments_dir(&s),
                            history,
                            &t["providerHandoffAttachments"]
                        ));
                }
            }
            match crate::context_transfer::input(&db, &t, &r) {
                Ok(context) => {
                    selection["o3TransferHandoffs"] = context["handoffs"].clone();
                    selection["o3DeliveredHistoryItemIds"] = context["deliveredItemIds"].clone();
                }
                Err(error) => selection["o3HandoffError"] = serde_json::json!(error),
            }
            match crate::handoff_budget::context(&db, &t, &r) {
                Ok(meta) => selection["o3HandoffBudget"] = meta,
                Err(error) => selection["o3HandoffError"] = serde_json::json!(error),
            }
            selection["o3NativeGoal"] = t["activeProviderThreadId"]
                .as_str()
                .and_then(|id| crate::events::get(&db, "providerThreads", id).ok())
                .map(|provider| provider["goal"].clone())
                .unwrap_or(serde_json::Value::Null);
            selection["o3GoalCommandOrigin"] = serde_json::json!(if m["createdBy"] == "user" {
                "user"
            } else {
                "automatic"
            });
            selection["o3AttemptId"] = r["activeAttemptId"].clone();
            selection["o3AppThreadId"] = serde_json::json!(tid);
            let input = RunInput {
                native_mcp: None,
                provider_instance_id: r["providerInstanceId"]
                    .as_str()
                    .or_else(|| t["providerInstanceId"].as_str())
                    .unwrap_or("")
                    .into(),
                // An empty thread whose initial input was rejected may have no durable
                // rollout to resume after restart. Replace only that known-empty binding.
                native_session_id: if rejected_binding { None } else { native_id },
                thread_id: format!("{}:{}", tid, t["providerGeneration"].as_str().unwrap_or("")),
                run_id: rid.clone(),
                cwd,
                text: if cold_compact {
                    "Summarize the historical context for continuation, preserving decisions, pending work and file references. Do not execute historical tasks or tools. The user requested context compaction.".to_owned()
                } else {
                    text
                },
                model_selection: selection,
                runtime_mode: r["executionPolicy"]["runtimeMode"]
                    .as_str()
                    .or_else(|| t["runtimeMode"].as_str())
                    .unwrap_or("full-access")
                    .into(),
                interaction_mode: r["executionPolicy"]["interactionMode"]
                    .as_str()
                    .or_else(|| t["interactionMode"].as_str())
                    .unwrap_or("default")
                    .into(),
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
            };
            (input, r["activeAttemptId"].as_str().map(str::to_owned))
        };
        if let Err(error) =
            crate::provider_switch::release_pending(&s, provider.as_ref(), &tid).await
        {
            let _ = persist_attempt(
                &s,
                &tid,
                &rid,
                attempt.as_deref(),
                ProviderEvent::Failed(error),
            );
            return;
        }
        if let Err(error) = crate::native_recovery::release_pending(
            &s,
            provider.as_ref(),
            &tid,
            &rid,
            attempt.as_deref(),
        )
        .await
        {
            let _ = persist_attempt(
                &s,
                &tid,
                &rid,
                attempt.as_deref(),
                ProviderEvent::Failed(error),
            );
            return;
        }
        if input
            .attachments
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v["type"] == "file"))
        {
            let state = s.clone();
            let attachments = input.attachments.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for value in attachments
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|v| v["type"] == "file")
                {
                    if let Some(path) =
                        crate::assets::attachment_path(&state, value["id"].as_str().unwrap_or(""))
                    {
                        state.document_text.ensure(&path);
                    }
                }
            })
            .await;
        }
        // Internal config is issued here, never accepted from persisted model options.
        if let Some(selection) = input.model_selection.as_object_mut() {
            selection.remove("o3BrowserMcp");
            selection.remove("o3NativeMcp");
        }
        let native_context = if input.runtime_mode != "text-only"
            && input.model_selection["o3ColdCompact"] != true
            && s.native_mcp.configured()
        {
            match s
                .native_mcp
                .obtain(
                    &s,
                    &tid,
                    &input.provider_instance_id,
                    &input.cwd,
                    &input.model_selection["options"],
                )
                .await
            {
                Ok(context) => Some(context),
                Err(message) => {
                    let _ = persist_attempt(
                        &s,
                        &tid,
                        &rid,
                        attempt.as_deref(),
                        ProviderEvent::Failed(message),
                    );
                    s.active.lock().unwrap().remove(&rid);
                    s.notify();
                    return;
                }
            }
        } else {
            None
        };
        if let Some(context) = &native_context {
            input.model_selection["o3NativeMcp"] = context.config();
            input.native_mcp = Some(context.clone());
        }
        let checkpoint_cwd = input.cwd.clone();
        let checkpoint_state = s.clone();
        let ct = tid.clone();
        let cr = rid.clone();
        let cc = checkpoint_cwd.clone();
        let checkpoint_attempt = attempt.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::checkpoint::capture_attempt(
                &checkpoint_state,
                &ct,
                &cr,
                &cc,
                true,
                checkpoint_attempt.as_deref(),
            )
        })
        .await;
        if crate::run_restart::admission(&s, &rid, attempt.as_deref()).is_err() {
            return;
        }
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
            if *stopped.borrow() {
                break;
            }
            let event = tokio::select! {
                biased;
                _=stopped.changed()=>break,
                event = events.recv() => match event { Some(event) => event, None => break },
                _ = changed.changed() => {
                    if crate::events::get(&s.db.lock().unwrap(), "runs", &rid).is_ok_and(|r| matches!(r["status"].as_str(), Some("completed" | "failed" | "interrupted" | "cancelled"))) { break; }
                    continue;
                }
            };
            if !crate::run_restart::matches(&s.db.lock().unwrap(), &rid, attempt.as_deref()) {
                buffered.clear();
                if let Some(attempt) = attempt.as_deref() {
                    let _ = crate::run_restart::observed(&s, &rid, attempt, &event);
                }
                continue;
            }
            let event = if let Some(mut failure) = event.failure_input() {
                if s.agent_scopes.oom_killed(&tid).await {
                    failure["message"] = serde_json::json!("Killed: out of memory.");
                    failure["code"] = serde_json::json!("oom_kill");
                    ProviderEvent::FailedWithDetails(failure)
                } else {
                    event
                }
            } else {
                event
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
                        if persist_attempt(
                            &s,
                            &tid,
                            &rid,
                            attempt.as_deref(),
                            ProviderEvent::TextDelta(text),
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                }
                continue;
            }
            let flush = !buffered.is_empty()
                && (streaming == "paragraph"
                    || matches!(&event, ProviderEvent::Codex(frame) if frame["method"] == "o3/goal-turn-completed")
                    || matches!(
                        event,
                        ProviderEvent::Completed
                            | ProviderEvent::Failed(_)
                            | ProviderEvent::FailedWithDetails(_)
                            | ProviderEvent::Interrupted
                    ));
            if flush
                && persist_attempt(
                    &s,
                    &tid,
                    &rid,
                    attempt.as_deref(),
                    ProviderEvent::TextDelta(std::mem::take(&mut buffered)),
                )
                .is_err()
            {
                break;
            }
            if matches!(
                event,
                ProviderEvent::Completed
                    | ProviderEvent::Interrupted
                    | ProviderEvent::Failed(_)
                    | ProviderEvent::FailedWithDetails(_)
            ) {
                checkpoint_finished = true;
                let cs = s.clone();
                let ct = tid.clone();
                let cr = rid.clone();
                let cc = checkpoint_cwd.clone();
                let ca = attempt.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::checkpoint::capture_attempt(&cs, &ct, &cr, &cc, false, ca.as_deref())
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
            let (ps, pt, pr, pa) = (s.clone(), tid.clone(), rid.clone(), attempt.clone());
            if tokio::task::spawn_blocking(move || {
                persist_attempt(&ps, &pt, &pr, pa.as_deref(), event)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
            .is_err()
            {
                break;
            }
            if let Some(id) = request_id.filter(|_| {
                crate::run_restart::matches(&s.db.lock().unwrap(), &rid, attempt.as_deref())
            }) {
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
                .is_ok_and(|r| matches!(r["status"].as_str(), Some("interrupted" | "cancelled")));
            if interrupted {
                break;
            }
        }
        if !buffered.is_empty() {
            let _ = persist_attempt(
                &s,
                &tid,
                &rid,
                attempt.as_deref(),
                ProviderEvent::TextDelta(buffered),
            );
        }
        let _ = cancel.send(true);
        tokio::select! {r=&mut task=>{match r {Ok(Ok(()))=>finished=true,Ok(Err(e))=>failure=Some(e),Err(e)=>failure=Some(e.to_string())}},_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{task.abort();let _=task.await;}}
        s.native_mcp.finish_turn(&tid).await;
        if !checkpoint_finished
            && crate::run_restart::matches(&s.db.lock().unwrap(), &rid, attempt.as_deref())
            && !crate::workspace_transition::pending(&s.db.lock().unwrap(), &tid).unwrap_or(true)
            && crate::events::get(&s.db.lock().unwrap(), "thread", &tid)
                .is_ok_and(|t| t["deletedAt"].is_null())
        {
            let cs = s.clone();
            let ct = tid.clone();
            let cr = rid.clone();
            let cc = checkpoint_cwd.clone();
            let ca = attempt.clone();
            let _ = tokio::task::spawn_blocking(move || {
                crate::checkpoint::capture_attempt(&cs, &ct, &cr, &cc, false, ca.as_deref())
            })
            .await;
        }
        s.active.lock().unwrap().remove(&rid);
        let final_run = crate::events::get(&s.db.lock().unwrap(), "runs", &rid);
        if let Ok(run) = final_run {
            if attempt
                .as_deref()
                .is_none_or(|a| run["activeAttemptId"].as_str() == Some(a))
                && !matches!(
                    run["status"].as_str(),
                    Some("completed" | "interrupted" | "failed" | "cancelled")
                )
            {
                let message = failure.unwrap_or_else(|| {
                    if finished {
                        "provider ended without completion"
                    } else {
                        "provider failed"
                    }
                    .into()
                });
                let failure = if s.agent_scopes.oom_killed(&tid).await {
                    ProviderEvent::FailedWithDetails(
                        serde_json::json!({"message":"Killed: out of memory.","code":"oom_kill"}),
                    )
                } else {
                    ProviderEvent::Failed(message)
                };
                let _ = persist_attempt(&s, &tid, &rid, attempt.as_deref(), failure);
            }
        }
        if crate::run_restart::matches(&s.db.lock().unwrap(), &rid, attempt.as_deref())
            && crate::events::has_live_requests(&s, &tid, &rid).unwrap_or(false)
        {
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
    let attempt = crate::events::get(&s.db.lock().unwrap(), "runs", &rid)
        .ok()
        .and_then(|r| r["activeAttemptId"].as_str().map(str::to_owned));
    tokio::spawn(async move {
        let mut changed = s.changed.subscribe();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            if !crate::run_restart::matches(&s.db.lock().unwrap(), &rid, attempt.as_deref()) {
                return;
            }
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
            let _ = persist_attempt(
                &s,
                &tid,
                &rid,
                attempt.as_deref(),
                ProviderEvent::Failed(
                    "Interrupt acknowledgement timed out; native delivery unknown".into(),
                ),
            );
        })
        .await;
    })
}

#[cfg(test)]
fn persist(
    s: &crate::store::State,
    tid: &str,
    rid: &str,
    event: ProviderEvent,
) -> Result<(), String> {
    persist_attempt(s, tid, rid, None, event)
}
pub(crate) fn persist_attempt(
    s: &crate::store::State,
    tid: &str,
    rid: &str,
    expected: Option<&str>,
    event: ProviderEvent,
) -> Result<(), String> {
    if s.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    if let ProviderEvent::NativeRecoveryReleased { id, reply } = event {
        let result = crate::native_recovery::release(s, tid, rid, expected, &id);
        let _ = reply.send(result.clone());
        return result;
    }
    if let ProviderEvent::RecoverNative {
        current_text,
        previous_native,
        reason,
        reply,
    } = event
    {
        let result = crate::native_recovery::prepare(
            s,
            tid,
            rid,
            expected,
            previous_native,
            reason,
            current_text,
        );
        let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let _ = reply.send(result);
        return outcome;
    }
    if let ProviderEvent::ContextDelivery { data, reply } = event {
        let result = crate::context_transfer::delivery(s, tid, rid, expected, data);
        let _ = reply.send(result.clone());
        return result;
    }
    s.native_mcp.touch(tid);
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
        let root = crate::events::get(&s.db.lock().unwrap(), "runs", rid)
            .ok()
            .and_then(|r| r["rootNodeId"].as_str().map(str::to_owned))
            .unwrap_or_else(|| rid.into());
        if root != rid {
            if let Some(native) = id.strip_prefix(&format!("codex-item-{rid}-")) {
                *id = format!("codex-item-{root}-{native}");
            }
        }
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
        return crate::command_output::persist_attempt(s, tid, rid, expected, command);
    }
    let settings = crate::settings::read(s)?;
    let mut db = s.db.lock().unwrap();
    if s.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut thread = get(&tx, "thread", tid)?;
    let mut run = get(&tx, "runs", rid)?;
    if !crate::run_restart::matches(&tx, rid, expected) {
        if let Some(attempt) = expected {
            crate::run_restart::note_terminal(&tx, rid, attempt, &event)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(db);
        s.notify();
        return Ok(());
    }
    if matches!(&event,ProviderEvent::Codex(f) if f["method"]=="turn/started") {
        if let Some(attempt) = expected {
            crate::run_restart::admitted(&tx, rid, attempt)?;
        }
    }
    if !thread["deletedAt"].is_null() {
        return Ok(());
    }
    if matches!(
        run["status"].as_str(),
        Some("interrupted" | "completed" | "failed" | "cancelled")
    ) {
        return Ok(());
    }
    let at = iso(now_ms());
    match event {
        ProviderEvent::ContextDelivery { .. }
        | ProviderEvent::RecoverNative { .. }
        | ProviderEvent::NativeRecoveryReleased { .. } => {
            unreachable!()
        }
        ProviderEvent::Codex(frame) => {
            let frame_binding = frame["method"] == "o3/binding";
            crate::codex_projection::persist(&tx, tid, rid, &thread, &mut run, &at, frame)?;
            thread = get(&tx, "thread", tid)?;
            if frame_binding {
                crate::provider_switch::bound(&tx, &thread, &run)?;
            }
            crate::context_transfer::bound(&tx, &mut thread, &run, &at)?;
        }
        ProviderEvent::Native(frame) => {
            crate::claude_projection::persist(&tx, tid, rid, &mut thread, &mut run, &at, &frame)?;
            if frame["type"] == "session" {
                crate::provider_switch::bound(&tx, &thread, &run)?;
                crate::context_transfer::bound(&tx, &mut thread, &run, &at)?;
            }
        }
        ProviderEvent::Started => {
            run["status"] = json!("running");
            run["startedAt"] = json!(at);
            run["recoveryPending"] = json!(false);
        }
        ProviderEvent::TextDelta(delta) => {
            let mid = assistant_message_id(&run, rid);
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
        ProviderEvent::Completed
        | ProviderEvent::Interrupted
        | ProviderEvent::Failed(_)
        | ProviderEvent::FailedWithDetails(_) => {
            run["status"] = json!(if matches!(event, ProviderEvent::Completed) {
                "completed"
            } else if matches!(event, ProviderEvent::Interrupted) {
                "interrupted"
            } else {
                "failed"
            });
            run["completedAt"] = json!(at);
            if let Some(mut input) = event.failure_input() {
                if input["class"].is_null() {
                    input["class"] = run["errorClass"].clone();
                }
                if input["resetAt"].is_null() {
                    input["resetAt"] = thread["usageLimitResetAt"].clone();
                }
                let failure = crate::provider_failure::make(&input);
                run["error"] = failure["message"].clone();
                run["errorClass"] = failure["class"].clone();
                crate::provider_failure::persist(&tx, tid, &thread, &run, &failure, &at)?;
            }
            if event.failure_input().is_some() {
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
            finish_assistant_message(&tx, tid, rid, &thread, &run, &at)?;
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

fn assistant_message_id(run: &serde_json::Value, rid: &str) -> String {
    if let Some(turn) = run["codexGoalMessageSuffix"].as_str() {
        format!("assistant-{rid}-goal-{turn}")
    } else if run["rootNodeId"].as_str().is_some_and(|root| root != rid) {
        format!(
            "assistant-{rid}-{}",
            run["activeAttemptId"].as_str().unwrap_or("")
        )
    } else {
        format!("assistant-{rid}")
    }
}

pub(crate) fn finish_assistant_message(
    db: &rusqlite::Connection,
    tid: &str,
    rid: &str,
    thread: &serde_json::Value,
    run: &serde_json::Value,
    at: &str,
) -> Result<(), String> {
    use crate::events::{append, get, put};
    use serde_json::json;
    let mid = assistant_message_id(run, rid);
    if let Ok(mut message) = get(db, "messages", &mid) {
        message["streaming"] = json!(false);
        message["updatedAt"] = json!(at);
        put(db, "messages", &mid, tid, &message)?;
        append(db, tid, "message.updated", message.clone(), thread)?;
        let item = crate::events::message_item(db, tid, &message)?;
        append(db, tid, "turn-item.updated", item, thread)?;
    }
    Ok(())
}

/// Routes sessions to a driver without imposing subprocess ownership on adapters.
pub struct Providers {
    pub claude: Option<std::sync::Arc<crate::claude::ClaudeProvider>>,
    pub codex: std::sync::Arc<crate::codex::CodexProvider>,
}
impl Provider for Providers {
    fn dispose_instance<'a>(
        &'a self,
        instance: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        match instance {
            "codex" => self.codex.dispose(),
            "claudeAgent" | "claude" | "claude-code" => match &self.claude {
                Some(p) => p.dispose(),
                None => Box::pin(async { Ok(()) }),
            },
            _ => Box::pin(async { Ok(()) }),
        }
    }
    fn capabilities(&self, instance: &str) -> Option<serde_json::Value> {
        match instance {
            "codex" => self.codex.capabilities(instance),
            "claudeAgent" | "claude" | "claude-code" => {
                self.claude.as_ref()?.capabilities(instance)
            }
            _ => None,
        }
    }
    fn plan_selection_transition(
        &self,
        instance: &str,
        current: &serde_json::Value,
        target: &serde_json::Value,
        capabilities: &serde_json::Value,
    ) -> Result<Option<crate::selection_transition::Selection>, String> {
        match instance {
            "codex" => {
                self.codex
                    .plan_selection_transition(instance, current, target, capabilities)
            }
            "claudeAgent" | "claude" | "claude-code" => self
                .claude
                .as_ref()
                .map(|p| p.plan_selection_transition(instance, current, target, capabilities))
                .unwrap_or(Ok(None)),
            _ => Ok(None),
        }
    }

    fn detach<'a>(
        &'a self,
        instance: &'a str,
        key: &'a str,
        native: Option<&'a str>,
        revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        match instance {
            "codex" => self.codex.detach(instance, key, native, revoke),
            "claudeAgent" | "claude" | "claude-code" => match &self.claude {
                Some(p) => p.detach(instance, key, native, revoke),
                None => Box::pin(async { Ok(()) }),
            },
            _ => Box::pin(async { Ok(()) }),
        }
    }

    fn refresh_defaults<'a>(
        &'a self,
        excluded: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if !excluded.iter().any(|id| id == "codex") {
                self.codex.probe().await?;
            }
            Ok(())
        })
    }
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
        // Fixture trees are retained for private container teardown.
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
        // Fixture trees are retained for private container teardown.
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
        // Fixture trees are retained for private container teardown.
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
            // Fixture trees are retained for private container teardown.
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
        // Fixture trees are retained for private container teardown.
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
            // Fixture trees are retained for private container teardown.
        }
    }
}

/// Instance registry. Each configured identity owns its own native process/session pool.
/// Changed/removed scopes retire before their replacements admit native work.
pub struct ConfiguredProviders {
    policy: crate::managed_policy::Policy,
    agent_scopes: Option<std::sync::Arc<crate::agent_scope::Registry>>,
    pub defaults: std::sync::Arc<dyn Provider>,
    pub directory: std::path::PathBuf,
    instances: std::sync::Mutex<std::collections::BTreeMap<String, Instance>>,
    last_settings: std::sync::Mutex<serde_json::Value>,
    settings_lock: Option<std::sync::Arc<std::sync::Mutex<()>>>,
    secrets: crate::provider_secrets::Registry,
    discovered_defaults: std::sync::Mutex<std::collections::HashSet<String>>,
    retired: std::sync::Mutex<Vec<std::sync::Arc<crate::provider_lifetime::Pool>>>,
    builtin_scopes: std::sync::Mutex<
        std::collections::BTreeMap<String, std::sync::Arc<crate::provider_lifetime::Pool>>,
    >,
}
struct Instance {
    settings: serde_json::Value,
    secrets: Option<crate::provider_secrets::Secrets>,
    discovered: bool,
    provider: Option<std::sync::Arc<crate::provider_lifetime::Pool>>,
    binary: std::path::PathBuf,
    preparation_error: Option<String>,
    continuation_key: Option<String>,
}
impl ConfiguredProviders {
    pub fn with_policy(mut self, policy: crate::managed_policy::Policy) -> Self {
        self.policy = policy;
        self
    }
    pub fn new(defaults: std::sync::Arc<dyn Provider>, directory: std::path::PathBuf) -> Self {
        Self {
            agent_scopes: None,
            policy: crate::managed_policy::Policy::unmanaged(std::path::PathBuf::new()),
            defaults,
            directory,
            instances: Default::default(),
            last_settings: std::sync::Mutex::new(serde_json::json!({})),
            settings_lock: None,
            secrets: crate::provider_secrets::Registry::new(std::sync::Arc::new(
                crate::secret_resolver::SecretResolver::system(),
            )),
            discovered_defaults: Default::default(),
            retired: Default::default(),
            builtin_scopes: Default::default(),
        }
    }
    /// Share the writer's lock before enabling private environment hydration.
    pub fn with_settings_lock(mut self, lock: std::sync::Arc<std::sync::Mutex<()>>) -> Self {
        self.settings_lock = Some(lock);
        self.secrets = self.secrets.with_private_profile(self.directory.clone());
        self
    }
    pub fn with_secret_resolver(
        mut self,
        resolver: std::sync::Arc<crate::secret_resolver::SecretResolver>,
    ) -> Self {
        self.secrets = self.secrets.with_resolver(resolver);
        self
    }
    pub fn with_notifications(mut self, notify: tokio::sync::watch::Sender<u64>) -> Self {
        self.secrets = self.secrets.with_notifications(notify);
        self
    }
    pub fn with_agent_scopes(
        mut self,
        registry: std::sync::Arc<crate::agent_scope::Registry>,
    ) -> Self {
        self.agent_scopes = Some(registry);
        self
    }
    fn settings(&self) -> serde_json::Value {
        let current = match std::fs::read(self.directory.join("settings.json")) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .filter(serde_json::Value::is_object),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(serde_json::json!({})),
            Err(_) => None,
        };
        let mut last = self.last_settings.lock().unwrap();
        if let Some(current) = current {
            *last = current;
        }
        let mut settings = self.policy.effective_settings(&last).unwrap_or_else(|_| {
            self.policy.effective_settings(&serde_json::json!({})).unwrap_or_else(|_| serde_json::json!({"providerInstances":{"codex":{"driver":"codex","enabled":false},"claudeAgent":{"driver":"claudeAgent","enabled":false}}}))
        });
        if self.policy.document.is_some() {
            if !settings["providerInstances"].is_object() {
                settings["providerInstances"] = serde_json::json!({});
            }
            for driver in ["codex", "claudeAgent"] {
                settings["providerInstances"]
                    .as_object_mut()
                    .unwrap()
                    .entry(driver)
                    .or_insert_with(|| serde_json::json!({"driver":driver}));
            }
        }
        settings
    }
    fn driver_denial(&self, instance: &str, settings: &serde_json::Value) -> Option<String> {
        let id = crate::provider_policy::configured_id(settings, instance);
        let driver = settings["providerInstances"][id]["driver"]
            .as_str()
            .unwrap_or(id);
        self.policy.driver_denial(driver)
    }
    fn builtin_snapshot(&self) -> (Vec<String>, crate::provider_lifetime::Scopes) {
        let instances = self.instances.lock().unwrap();
        let scopes = self.builtin_scopes.lock().unwrap();
        (instances.keys().cloned().collect(), scopes.clone())
    }
    fn builtin_scope(&self, id: &str) -> Option<std::sync::Arc<crate::provider_lifetime::Pool>> {
        let id = if matches!(id, "claude" | "claude-code") {
            "claudeAgent"
        } else {
            id
        };
        self.builtin_scopes.lock().unwrap().get(id).cloned()
    }
    async fn close_retired(&self) -> Result<(), String> {
        let retired = self.retired.lock().unwrap().clone();
        for pool in retired {
            pool.close().await?;
        }
        self.retired.lock().unwrap().retain(|p| !p.is_closed());
        Ok(())
    }
    fn sync(&self) -> serde_json::Value {
        use serde_json::json;
        let _settings_guard = self.settings_lock.as_ref().map(|lock| lock.lock().unwrap());
        let settings = self.settings();
        let hydrated = crate::provider_policy::hydrate(&settings);
        let secret_states = self.secrets.reconcile(&hydrated, &settings, &self.policy);
        let mut cache = self.instances.lock().unwrap();
        let available_builtins = self
            .defaults
            .config()
            .into_iter()
            .filter_map(|p| {
                p["instanceId"]
                    .as_str()
                    .filter(|id| matches!(*id, "codex" | "claudeAgent"))
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        let mut builtin_scopes = self.builtin_scopes.lock().unwrap();
        let mut retired = self.retired.lock().unwrap();
        retired.retain(|pool| !pool.is_closed());
        let mut newly_retired = Vec::new();
        cache.retain(|id, instance| {
            let keep = hydrated
                .get(id)
                .is_some_and(|entry| instance.settings == *entry)
                && match secret_states.get(id) {
                    Some(crate::provider_secrets::Status::Ready { secrets, .. }) => {
                        instance.secrets.as_ref() == Some(secrets)
                    }
                    Some(crate::provider_secrets::Status::Reading) => {
                        instance.preparation_error.as_deref() == Some("Reading secrets…")
                    }
                    Some(crate::provider_secrets::Status::Unavailable(reason)) => {
                        instance.provider.is_none()
                            && instance.preparation_error.as_ref() == Some(reason)
                    }
                    None => false,
                };
            if !keep {
                if let Some(pool) = &instance.provider {
                    pool.retire();
                    retired.push(pool.clone());
                    newly_retired.push(pool.clone());
                }
            }
            keep
        });
        builtin_scopes.retain(|id, pool| {
            let keep = available_builtins.contains(id) && hydrated.get(id).is_none();
            if !keep {
                pool.retire();
                retired.push(pool.clone());
                newly_retired.push(pool.clone());
            }
            keep
        });
        // Metadata inspection remains synchronous; physical close is asynchronous.
        // Every new scope also carries this barrier, including inspection without a runtime.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            for pool in newly_retired {
                runtime.spawn(async move {
                    let _ = pool.close().await;
                });
            }
        }
        let predecessors = retired.clone();
        drop(retired);
        for id in available_builtins {
            if hydrated.get(&id).is_none() && !builtin_scopes.contains_key(&id) {
                let provider = std::sync::Arc::new(crate::provider_lifetime::Builtin::new(
                    self.defaults.clone(),
                    id.clone(),
                ));
                builtin_scopes.insert(
                    id.clone(),
                    crate::provider_lifetime::Pool::new(provider, predecessors.clone()),
                );
                self.discovered_defaults.lock().unwrap().remove(&id);
            }
        }
        drop(builtin_scopes);
        for (id, entry) in hydrated.as_object().into_iter().flatten() {
            if cache.contains_key(id) {
                continue;
            }
            let (resolved_entry, secrets, secret_error) = match secret_states.get(id) {
                Some(crate::provider_secrets::Status::Ready { entry, secrets }) => {
                    (entry.clone(), Some(secrets.clone()), None)
                }
                Some(crate::provider_secrets::Status::Reading) => {
                    (entry.clone(), None, Some("Reading secrets…".to_owned()))
                }
                Some(crate::provider_secrets::Status::Unavailable(reason)) => {
                    (entry.clone(), None, Some(reason.clone()))
                }
                None => (
                    entry.clone(),
                    None,
                    Some("Invalid provider instance settings".to_owned()),
                ),
            };
            let working = &resolved_entry;
            let driver = entry["driver"].as_str().unwrap_or("");
            let name = if driver == "claudeAgent" {
                "claude"
            } else {
                driver
            };
            let binary = instance_binary(working, name);
            let resolved = match secret_error.or_else(|| self.policy.driver_denial(driver)) {
                Some(reason) => Err(reason),
                None => crate::provider_home::resolve(working),
            };
            let preparation_error = resolved.as_ref().err().cloned();
            let layout = resolved.ok().flatten();
            let continuation_key = layout.as_ref().map(|home| home.continuation_key());
            let env = layout
                .as_ref()
                .map(|home| home.environment.clone())
                .unwrap_or_else(|| crate::provider_home::raw_environment(working));
            let home = layout.as_ref().and_then(|home| home.runtime.clone());
            let setup = layout
                .filter(|home| home.driver == "codex")
                .map(crate::provider_home::Setup::new);
            let args = launch_args(working["config"]["launchArgs"].as_str().unwrap_or(""))
                .unwrap_or_default();
            let provider: Option<std::sync::Arc<dyn Provider>> = if preparation_error.is_some() {
                None
            } else {
                match driver {
                    "codex" => Some(std::sync::Arc::new(
                        crate::codex::CodexProvider::new(binary.clone(), home)
                            .with_optional_agent_scopes(self.agent_scopes.clone())
                            .with_environment(env)
                            .with_home_setup(setup)
                            .with_launch_args(args.clone())
                            .with_managed_policy(self.policy.document.is_some()),
                    )),
                    "claudeAgent" => Some(std::sync::Arc::new(
                        crate::claude::ClaudeProvider::new(binary.clone())
                            .with_optional_agent_scopes(self.agent_scopes.clone())
                            .with_environment(env)
                            .with_launch_args(args.clone())
                            .with_managed_policy(self.policy.document.is_some()),
                    )),
                    _ => None,
                }
            };
            cache.insert(
                id.clone(),
                Instance {
                    settings: entry.clone(),
                    secrets,
                    discovered: false,
                    provider: provider
                        .map(|p| crate::provider_lifetime::Pool::new(p, predecessors.clone())),
                    binary,
                    preparation_error,
                    continuation_key,
                },
            );
        }
        json!(settings)
    }
}
impl Provider for ConfiguredProviders {
    fn capabilities(&self, instance: &str) -> Option<serde_json::Value> {
        self.sync();
        let selected = {
            let entries = self.instances.lock().unwrap();
            entries
                .get(instance)
                .or_else(|| {
                    if matches!(instance, "claude" | "claude-code") {
                        entries.get("claudeAgent")
                    } else {
                        None
                    }
                })
                .map(|i| i.provider.clone())
        };
        match selected {
            Some(Some(p)) => p.capabilities(instance),
            Some(None) => None,
            None => self
                .builtin_scope(instance)
                .map(|p| p.capabilities(instance))
                .unwrap_or_else(|| self.defaults.capabilities(instance)),
        }
    }
    fn plan_selection_transition(
        &self,
        instance: &str,
        current: &serde_json::Value,
        target: &serde_json::Value,
        capabilities: &serde_json::Value,
    ) -> Result<Option<crate::selection_transition::Selection>, String> {
        self.sync();
        let selected = {
            let entries = self.instances.lock().unwrap();
            entries
                .get(instance)
                .or_else(|| {
                    if matches!(instance, "claude" | "claude-code") {
                        entries.get("claudeAgent")
                    } else {
                        None
                    }
                })
                .map(|i| i.provider.clone())
        };
        match selected {
            Some(Some(p)) => p.plan_selection_transition(instance, current, target, capabilities),
            Some(None) => Ok(None),
            None => match self.builtin_scope(instance) {
                Some(p) => p.plan_selection_transition(instance, current, target, capabilities),
                None => {
                    self.defaults
                        .plan_selection_transition(instance, current, target, capabilities)
                }
            },
        }
    }

    fn detach<'a>(
        &'a self,
        instance: &'a str,
        key: &'a str,
        native: Option<&'a str>,
        revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        let selected = {
            let instances = self.instances.lock().unwrap();
            instances
                .get(instance)
                .or_else(|| {
                    if matches!(instance, "claude" | "claude-code") {
                        instances.get("claudeAgent")
                    } else {
                        None
                    }
                })
                .and_then(|instance| instance.provider.clone())
        };
        Box::pin(async move {
            self.close_retired().await?;
            match selected {
                Some(p) => p.detach(instance, key, native, revoke).await,
                None => match self.builtin_scope(instance) {
                    Some(p) => p.detach(instance, key, native, revoke).await,
                    None => self.defaults.detach(instance, key, native, revoke).await,
                },
            }
        })
    }

    fn discover<'a>(
        &'a self,
        cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            self.close_retired().await?;
            let (configured, builtin_scopes) = self.builtin_snapshot();
            let pending = {
                let mut discovered = self.discovered_defaults.lock().unwrap();
                ["codex", "claudeAgent"]
                    .into_iter()
                    .filter(|id| !configured.iter().any(|c| c.as_str() == *id))
                    .filter(|id| discovered.insert((*id).into()))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let mut unmanaged = Vec::new();
            for id in pending {
                match builtin_scopes.get(&id) {
                    Some(p) => {
                        let _ = p.discover(cwd.clone()).await;
                    }
                    None => unmanaged.push(id),
                }
            }
            if !unmanaged.is_empty() {
                let excluded = ["codex", "claudeAgent"]
                    .into_iter()
                    .filter(|id| !unmanaged.iter().any(|p| p == id))
                    .map(str::to_owned)
                    .collect();
                let _ = self.defaults.discover_defaults(cwd.clone(), excluded).await;
            }
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values_mut()
                .filter_map(|i| {
                    if i.discovered || !crate::provider_policy::instance_enabled(&i.settings) {
                        return None;
                    }
                    i.discovered = true;
                    i.provider.clone()
                })
                .collect();
            for p in providers {
                let _ = p.discover(cwd.clone()).await;
            }
            if self.secrets.has_pending() {
                self.secrets.wait().await;
                self.discover(cwd).await?;
            }
            Ok(())
        })
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            self.close_retired().await?;
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values()
                .filter(|i| crate::provider_policy::instance_enabled(&i.settings))
                .filter_map(|i| i.provider.clone())
                .collect();
            let (mut excluded, builtins) = self.builtin_snapshot();
            excluded.extend(builtins.keys().cloned());
            let _ = self.defaults.poll_defaults(excluded).await;
            for p in builtins.values() {
                let _ = p.poll_usage().await;
            }
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
            if let Some(key) = &instance.continuation_key {
                descriptor["continuation"] = json!({"groupKey":key});
            }
            if let Some(color) = instance.settings.get("accentColor") {
                descriptor["accentColor"] = color.clone();
            }
            descriptor["displayName"] = instance
                .settings
                .get("displayName")
                .cloned()
                .unwrap_or_else(|| json!(id));
            descriptor["enabled"] =
                json!(crate::provider_policy::instance_enabled(&instance.settings));
            if instance.provider.is_none() {
                let reason = instance
                    .preparation_error
                    .as_deref()
                    .map(crate::provider_home::display_error)
                    .unwrap_or_else(|| {
                        format!(
                            "Driver '{}' is not registered in this build.",
                            instance.settings["driver"].as_str().unwrap_or("")
                        )
                    });
                descriptor["enabled"] = json!(false);
                descriptor["installed"] = json!(false);
                descriptor["status"] = json!("error");
                descriptor["version"] = serde_json::Value::Null;
                descriptor["auth"] = json!({"status":"unknown"});
                descriptor["message"] = json!(reason);
                descriptor["availability"] = json!("unavailable");
                descriptor["unavailableReason"] = json!(reason);
                descriptor.as_object_mut().unwrap().remove("setup");
            }
            if instance.provider.is_some() && descriptor["enabled"] == false {
                descriptor["status"] = json!("disabled");
            }
            result.push(descriptor);
        }
        for provider in &mut result {
            if provider["instanceId"]
                .as_str()
                .is_some_and(|id| !instances.contains_key(id))
                && matches!(provider["driver"].as_str(), Some("codex" | "claudeAgent"))
            {
                if let Ok(Some(home)) =
                    crate::provider_home::resolve(&json!({"driver":provider["driver"]}))
                {
                    provider["continuation"] = json!({"groupKey":home.continuation_key()});
                }
            }
        }
        result
    }
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.sync();
            self.close_retired().await?;
            let (mut excluded, builtins) = self.builtin_snapshot();
            excluded.extend(builtins.keys().cloned());
            self.defaults.refresh_defaults(excluded).await?;
            for p in builtins.values() {
                p.refresh().await?;
            }
            let providers: Vec<_> = self
                .instances
                .lock()
                .unwrap()
                .values()
                .filter(|i| crate::provider_policy::instance_enabled(&i.settings))
                .filter_map(|i| i.provider.clone())
                .collect();
            for p in providers {
                let _ = p.refresh().await;
            }
            Ok(())
        })
    }
    fn reload_secrets<'a>(
        &'a self,
        instance: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let settings = self.sync();
            let instance = instance.map(|id| crate::provider_policy::configured_id(&settings, id));
            self.secrets.reload_reconciled(&settings, instance).await;
            self.sync();
            self.close_retired().await
        })
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: mpsc::Sender<ProviderEvent>,
        control: mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        let settings = self.sync();
        if let Some(reason) = self.driver_denial(&input.provider_instance_id, &settings) {
            return Box::pin(async move { Err(reason) });
        }
        let selected = {
            let instances = self.instances.lock().unwrap();
            instances
                .get(&input.provider_instance_id)
                .or_else(|| {
                    if matches!(
                        input.provider_instance_id.as_str(),
                        "claude" | "claude-code"
                    ) {
                        instances.get("claudeAgent")
                    } else {
                        None
                    }
                })
                .map(|i| {
                    if !crate::provider_policy::instance_enabled(&i.settings) {
                        Err("provider instance is disabled".to_owned())
                    } else if crate::executable::resolve(&i.binary).is_none() {
                        Err("provider binary is unavailable".to_owned())
                    } else {
                        i.provider.clone().ok_or_else(|| {
                            i.preparation_error
                                .clone()
                                .unwrap_or_else(|| "unsupported provider driver".to_owned())
                        })
                    }
                })
                .or_else(|| self.builtin_scope(&input.provider_instance_id).map(Ok))
        };
        match selected {
            None if matches!(
                input.provider_instance_id.as_str(),
                "codex" | "claudeAgent" | "claude" | "claude-code"
            ) =>
            {
                Box::pin(async move {
                    self.close_retired().await?;
                    self.defaults.execute(input, output, control, cancel).await
                })
            }
            None => Box::pin(async { Err("provider instance is not configured".into()) }),
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
    crate::provider_home::resolve(entry)
        .ok()
        .flatten()
        .map(|home| home.environment)
        .unwrap_or_else(|| crate::provider_home::raw_environment(entry))
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
            // Fixture trees are retained for private container teardown.
        }
    }
}
