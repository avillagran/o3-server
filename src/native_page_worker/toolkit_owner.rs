//! Real private toolkit continuation service. No model input creates a caller,
//! physical Page, Control context, or callback authority.
pub use super::source_deadline::FrozenDeadline;
use super::{
    caller::Caller,
    toolkit_callback::CallbackRequest,
    toolkit_ledger::{Binding, CallbackPhase, Ledger},
    Fault, JavaScriptJson, NativeInvocation, NativeOperation, NativePage, OperationResult,
    ServiceFuture, ServiceMethod, ServiceReply, ServiceRequest, ToolkitOwner, Worker,
};
use crate::{native_mcp::Context, preview_control::runtime::RunContext, store::State};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::watch;
pub enum CallbackOutput {
    Value(OperationResult),
    Undefined,
    StatusNoTab(crate::browser_profile_catalog::Snapshot),
}
pub type CallbackFuture = Pin<Box<dyn Future<Output = Result<CallbackOutput, Fault>> + Send>>;
pub type InvocationFuture =
    Pin<Box<dyn Future<Output = Result<OperationResult, Fault>> + Send + 'static>>;
/// Implemented only by the actual State/tab coordinator; no default Page exists.
pub trait CallbackExecutor: Send + Sync + 'static {
    fn run(&self, callback: SourceCallback, deadline: FrozenDeadline) -> CallbackFuture;
    fn mark_used(&self, request: &CallbackRequest) -> Result<(), Fault>;
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>>;
}
#[derive(Clone)]
pub struct SourceCallback {
    binding: Arc<Binding>,
    request: CallbackRequest,
    active: Arc<AtomicBool>,
    admission: Arc<AtomicU64>,
    entered: Arc<AtomicU8>,
}
pub(crate) struct EnteredStatusCallback {
    callback: SourceCallback,
}
impl EnteredStatusCallback {
    pub(crate) fn callback(&self) -> &SourceCallback {
        &self.callback
    }
}
impl SourceCallback {
    pub fn request(&self) -> &CallbackRequest {
        &self.request
    }
    pub(crate) fn enter_status(&self) -> Result<EnteredStatusCallback, Value> {
        if self.request.class != NativeOperation::Status || self.request.operation != "status" {
            return Err(json!({"code":"capability_denied","reason":"status_callback_class"}));
        }
        self.validate()?;
        admit_entry(&self.admission, &self.entered, &self.active)?;
        self.validate()?;
        Ok(EnteredStatusCallback {
            callback: self.clone(),
        })
    }
    pub fn validate(&self) -> Result<(), Value> {
        if (self.entered.load(Ordering::Acquire) != 2
            && self.admission.load(Ordering::Acquire) & CLOSING != 0)
            || !self.active.load(Ordering::Acquire)
            || self.binding.context.strong_count() == 0
        {
            return Err(json!({"code":"capability_denied","reason":"callback_closed"}));
        }
        self.binding.caller.callback_current(self.request.class)
    }
    pub fn admitted(&self, page: &NativePage, run: &RunContext) -> Result<NativeInvocation, Value> {
        if self.entered.load(Ordering::Acquire) != 2 {
            return Err(json!({"code":"capability_denied","reason":"callback_not_entered"}));
        }
        page.admitted_source_callback(run, self.clone())
    }
    pub(crate) fn validates_thread(&self, thread: &str) -> bool {
        self.entered.load(Ordering::Acquire) == 2
            && self.request.thread_id == thread
            && self.binding.caller.thread_id() == thread
            && self.validate().is_ok()
    }
    /// This gate runs under Control's entry lock. It uses atomics only; fresh
    /// database/provider checks remain outside that lock in admitted/validate.
    pub(crate) fn entry_gate(&self) -> crate::preview_control::runtime::SourceEntryGate {
        let admission = self.admission.clone();
        let entered = self.entered.clone();
        let active = self.active.clone();
        entry_gate(admission, entered, active)
    }
    pub(crate) fn context(&self) -> Result<Arc<Context>, Value> {
        self.validate()?;
        self.binding
            .context
            .upgrade()
            .ok_or_else(|| json!({"code":"capability_denied","reason":"callback_closed"}))
    }
}
fn entry_gate(
    admission: Arc<AtomicU64>,
    entered: Arc<AtomicU8>,
    active: Arc<AtomicBool>,
) -> crate::preview_control::runtime::SourceEntryGate {
    crate::preview_control::runtime::SourceEntryGate::new(Arc::new(move || {
        admit_entry(&admission, &entered, &active)
    }))
}
/// Shared by actual Control entry and the private Status entry. No database,
/// provider callback, Control state or await occurs inside this atomic gate.
fn admit_entry(
    admission: &AtomicU64,
    entered: &AtomicU8,
    active: &AtomicBool,
) -> Result<(), Value> {
    if !active.load(Ordering::Acquire)
        || entered
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err(json!({"code":"capability_denied","reason":"callback_closed"}));
    }
    let mut current = admission.load(Ordering::Acquire);
    loop {
        if current & CLOSING != 0 || current == CLOSING - 1 {
            return Err(json!({"code":"capability_denied","reason":"callback_closed"}));
        }
        match admission.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                entered.store(2, Ordering::Release);
                return Ok(());
            }
            Err(actual) => current = actual,
        }
    }
}

struct CallbackLifetime(Arc<AtomicBool>);
impl Drop for CallbackLifetime {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
/// A single-use invocation minted by the same service installed on the Worker.
/// Dropping an unlaunched ticket proves no Node producer could have existed.
pub(super) struct ToolkitInvocation {
    owner: Arc<Service>,
    receipt: String,
    tool: NativeOperation,
    dispatched: bool,
}
impl ToolkitInvocation {
    pub(super) fn caller(&self) -> &str {
        &self.receipt
    }
    pub(super) fn operation(&self) -> &str {
        self.tool.tool_name()
    }
    pub(super) fn owner(&self) -> Arc<dyn ToolkitOwner> {
        self.owner.clone()
    }
    pub(super) fn dispatched(&mut self) {
        self.dispatched = true;
    }
}
impl Drop for ToolkitInvocation {
    fn drop(&mut self) {
        if !self.dispatched {
            let _ = self.owner.ledger.lock().unwrap().end_source(&self.receipt);
        }
    }
}
struct Observer {
    owner: Arc<Service>,
    receipt: String,
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.owner.ledger.lock().unwrap().returned(&self.receipt);
    }
}
const CLOSING: u64 = 1 << 63;

pub struct Service {
    id: String,
    state: Weak<State>,
    ledger: Mutex<Ledger>,
    executor: Arc<dyn CallbackExecutor>,
    revoked: watch::Sender<bool>,
    admission: Arc<AtomicU64>,
    weak: Weak<Self>,
}
// Weak ownership prevents a second native broker resetting live assignments;
// neither a State nor an authenticated provider is retained by this registry.
static OWNERS: Mutex<Vec<Weak<Service>>> = Mutex::new(Vec::new());
fn raw(value: Value) -> Result<JavaScriptJson, Fault> {
    JavaScriptJson::parse(value.to_string()).map_err(|_| Fault::Protocol)
}
fn refused(value: Value) -> Result<ServiceFuture, Fault> {
    let payload = raw(value)?;
    Ok(Box::pin(async move {
        Ok(ServiceReply {
            failed: true,
            payload,
            callback_receipt: None,
        })
    }))
}
fn reply(value: Value) -> Result<ServiceReply, Fault> {
    Ok(ServiceReply {
        failed: false,
        payload: raw(value)?,
        callback_receipt: None,
    })
}
fn secret() -> Result<String, Fault> {
    let mut bytes = [0; 48];
    getrandom::fill(&mut bytes).map_err(|_| Fault::InvalidInstallation)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Projection {
    thread_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Reserve {
    tool: String,
    request: Box<serde_json::value::RawValue>,
    update_current_tab: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Receipt {
    callback_receipt: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Bounds {
    lower_ns: String,
    upper_ns: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Run {
    callback_receipt: String,
    deadline_ms: f64,
    source_deadline: Bounds,
}
fn decode<T: serde::de::DeserializeOwned>(input: &JavaScriptJson) -> Result<T, Fault> {
    serde_json::from_str(input.json()).map_err(|_| Fault::Protocol)
}
impl Service {
    pub fn new(
        state: &Arc<State>,
        executor: Arc<dyn CallbackExecutor>,
    ) -> Result<Arc<Self>, Fault> {
        let mut owners = OWNERS.lock().map_err(|_| Fault::Closed)?;
        owners.retain(|owner| owner.strong_count() != 0);
        if owners
            .iter()
            .filter_map(Weak::upgrade)
            .any(|owner| Weak::ptr_eq(&owner.state, &Arc::downgrade(state)))
        {
            return Err(Fault::Busy);
        }
        let id = secret()?;
        let owner = Arc::new_cyclic(|weak| Self {
            id,
            state: Arc::downgrade(state),
            ledger: Mutex::new(Ledger::default()),
            executor,
            revoked: watch::channel(false).0,
            admission: Arc::new(AtomicU64::new(0)),
            weak: weak.clone(),
        });
        owners.push(Arc::downgrade(&owner));
        Ok(owner)
    }
    /// Synchronous capture returns an owned future with no borrowed/strong
    /// Context or State retained across observation. Even an unpolled future
    /// owns its never-dispatched cancellation and observer-return receipts.
    pub fn invoke(
        self: &Arc<Self>,
        worker: &Worker,
        context: &Arc<Context>,
        tool: NativeOperation,
        input: JavaScriptJson,
    ) -> InvocationFuture {
        let prepared = (|| {
            let state = self.state.upgrade().ok_or(Fault::Revoked)?;
            let caller = Caller::capture(context).map_err(|_| Fault::Revoked)?;
            if !caller.belongs_to(&state) {
                return Err(Fault::Revoked);
            }
            caller.scope().map_err(|_| Fault::Revoked)?;
            let receipt = secret()?;
            self.ledger
                .lock()
                .unwrap()
                .register(context, tool, receipt.clone())
                .map_err(|_| Fault::Revoked)?;
            let observer = Observer {
                owner: self.clone(),
                receipt: receipt.clone(),
            };
            Ok((
                ToolkitInvocation {
                    owner: self.clone(),
                    receipt,
                    tool,
                    dispatched: false,
                },
                observer,
            ))
        })();
        match prepared {
            Err(error) => Box::pin(async move { Err(error) }),
            Ok((ticket, observer)) => {
                let worker = worker.clone();
                Box::pin(async move {
                    let _observer = observer;
                    worker.invoke_toolkit(ticket, input).await
                })
            }
        }
    }
    fn admit_inner(self: &Arc<Self>, request: ServiceRequest) -> Result<ServiceFuture, Fault> {
        let mut ledger = self.ledger.lock().unwrap();
        if ledger.unknown {
            return Err(Fault::CleanupUnknown);
        }
        let closing = *self.revoked.borrow();
        let invocation = ledger
            .invocations
            .get(&request.caller)
            .ok_or(Fault::Protocol)?;
        let binding = invocation.binding.clone();
        match request.method {
            ServiceMethod::CallerScope => {
                if closing {
                    return refused(json!({"code":"capability_denied","reason":"toolkit_closing"}));
                }
                let _: Empty = decode(&request.input)?;
                if let Err(error) = binding.caller.scope() {
                    return refused(error);
                }
                let value = binding.descriptor.clone();
                Ok(Box::pin(async move { reply(value) }))
            }
            ServiceMethod::Projection => {
                if closing {
                    return refused(json!({"code":"capability_denied","reason":"toolkit_closing"}));
                }
                let input: Projection = decode(&request.input)?;
                let value = match binding.caller.thread_projection(&input.thread_id) {
                    Ok(value) => value,
                    Err(error) => return refused(error),
                };
                Ok(Box::pin(async move { reply(value) }))
            }
            ServiceMethod::ReserveCallback => {
                if closing {
                    return refused(json!({"code":"capability_denied","reason":"toolkit_closing"}));
                }
                let input: Reserve = decode(&request.input)?;
                let tool = invocation.tool;
                if input.tool != tool.tool_name() {
                    return Err(Fault::Protocol);
                }
                if let Err(error) = binding.caller.preview_issued() {
                    return refused(error);
                }
                let agent = match binding.caller.agent_session_key() {
                    Ok(value) => value,
                    Err(error) => return refused(error),
                };
                let decoded = CallbackRequest::decode(
                    &JavaScriptJson(input.request),
                    tool,
                    input.update_current_tab,
                    binding.caller.thread_id(),
                    &agent,
                )
                .map_err(|_| Fault::Protocol)?;
                let receipt = secret()?;
                ledger
                    .reserve(&request.caller, receipt.clone(), decoded)
                    .map_err(|_| Fault::Protocol)?;
                Ok(Box::pin(async move {
                    let mut result = reply(json!({}))?;
                    result.callback_receipt = Some(receipt);
                    Ok(result)
                }))
            }
            ServiceMethod::RunCallback => {
                let input: Run = decode(&request.input)?;
                if !input.deadline_ms.is_finite() {
                    return Err(Fault::Protocol);
                }
                let deadline = FrozenDeadline::capture(
                    &input.source_deadline.lower_ns,
                    &input.source_deadline.upper_ns,
                )?;
                let running = ledger
                    .run(&request.caller, &input.callback_receipt)
                    .map_err(|_| Fault::Protocol)?;
                let callback = SourceCallback {
                    binding: running.binding,
                    request: running.request,
                    active: Arc::new(AtomicBool::new(true)),
                    admission: self.admission.clone(),
                    entered: Arc::new(AtomicU8::new(0)),
                };
                let owner = self.clone();
                drop(ledger);
                Ok(Box::pin(async move {
                    let _lifetime = CallbackLifetime(callback.active.clone());
                    let result = if let Err(error) = callback.validate() {
                        Ok(CallbackOutput::Value(OperationResult::NativeError(
                            json!({"tag":"PreviewAutomationExecutionError","message":error["message"].as_str().unwrap_or("The native preview caller is unavailable.")}),
                        )))
                    } else {
                        owner.executor.run(callback, deadline).await
                    };
                    let mut ledger = owner.ledger.lock().unwrap();
                    let value = match result {
                        Ok(CallbackOutput::StatusNoTab(catalog)) => reply(json!({
                            "ok":true,"statusNoTab":catalog.source_status_catalog()
                        })),
                        Ok(CallbackOutput::Undefined) => reply(json!({"ok":true})),
                        Ok(CallbackOutput::Value(OperationResult::Value(value))) => {
                            reply(json!({"ok":true,"value":value}))
                        }
                        Ok(CallbackOutput::Value(OperationResult::JavaScriptValue(value))) => {
                            let text = format!("{{\"ok\":true,\"value\":{}}}", value.json());
                            Ok(ServiceReply {
                                failed: false,
                                payload: JavaScriptJson::parse(text)
                                    .map_err(|_| Fault::Protocol)?,
                                callback_receipt: None,
                            })
                        }
                        Ok(CallbackOutput::Value(OperationResult::NativeError(error))) => {
                            reply(json!({"ok":false,"error":error}))
                        }
                        Ok(CallbackOutput::Value(OperationResult::JavaScriptError(error))) => {
                            let text = format!("{{\"ok\":false,\"error\":{}}}", error.json());
                            Ok(ServiceReply {
                                failed: false,
                                payload: JavaScriptJson::parse(text)
                                    .map_err(|_| Fault::Protocol)?,
                                callback_receipt: None,
                            })
                        }
                        Err(error) => {
                            ledger.unknown = true;
                            return Err(error);
                        }
                    }?;
                    ledger
                        .responded(&request.caller, &input.callback_receipt)
                        .map_err(|_| Fault::Protocol)?;
                    Ok(value)
                }))
            }
            ServiceMethod::MarkUsed | ServiceMethod::AbandonCallback => {
                let input: Receipt = decode(&request.input)?;
                let callback = ledger
                    .callbacks
                    .get(&input.callback_receipt)
                    .ok_or(Fault::Protocol)?;
                if callback.invocation != request.caller {
                    return Err(Fault::Protocol);
                }
                let abandoned = request.method == ServiceMethod::AbandonCallback;
                if (abandoned && callback.phase != CallbackPhase::Reserved)
                    || (!abandoned && callback.phase != CallbackPhase::Responded)
                {
                    return Err(Fault::Protocol);
                }
                if request.method == ServiceMethod::MarkUsed {
                    self.executor.mark_used(&callback.request)?;
                }
                ledger
                    .finish(
                        &request.caller,
                        &input.callback_receipt,
                        request.method == ServiceMethod::AbandonCallback,
                    )
                    .map_err(|_| Fault::Protocol)?;
                Ok(Box::pin(async { reply(json!({})) }))
            }
            ServiceMethod::EndInvocation => {
                let _: Empty = decode(&request.input)?;
                ledger
                    .end_source(&request.caller)
                    .map_err(|_| Fault::Protocol)?;
                Ok(Box::pin(async { reply(json!({})) }))
            }
        }
    }
}
impl ToolkitOwner for Service {
    fn resource_id(&self) -> &str {
        &self.id
    }
    fn revocation(&self) -> watch::Receiver<bool> {
        self.revoked.subscribe()
    }
    fn revoke(&self) {
        self.admission.fetch_or(CLOSING, Ordering::AcqRel);
        self.revoked.send_replace(true);
        self.ledger.lock().unwrap().closing = true;
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
        self.revoke();
        Box::pin(async move {
            // Never retire physical workers before independently proving all
            // source/observer/finally receipts. Unknown is not an end receipt.
            {
                let mut ledger = self.ledger.lock().unwrap();
                if !ledger.clean() {
                    ledger.unknown = true;
                    return Err(Fault::CleanupUnknown);
                }
            }
            self.executor.close().await
        })
    }
    fn admit(&self, request: ServiceRequest) -> Result<ServiceFuture, Fault> {
        self.weak
            .upgrade()
            .ok_or(Fault::Closed)?
            .admit_inner(request)
    }
}

#[cfg(test)]
mod close_tests {
    use super::*;
    use crate::preview_control::runtime::Control;
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn ordinary_service_close_drains_entered_gate_but_refuses_queued_gate() {
        crate::test_sandbox::assert_isolated();
        let control = Control::new(Some("source".into()));
        let admission = Arc::new(AtomicU64::new(0));
        let entered = Arc::new(AtomicU8::new(0));
        let queued_entered = Arc::new(AtomicU8::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let (started, start) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (finished, finish) = oneshot::channel();
        let tail_active = active.clone();
        let running = control.source_agent_admitted(
            "source",
            tokio::time::Instant::now() + Duration::from_secs(30),
            entry_gate(admission.clone(), entered.clone(), active.clone()),
            move |_| async move {
                started.send(()).unwrap();
                released.await.unwrap();
                assert!(tail_active.load(Ordering::Acquire));
                finished.send(()).unwrap();
                Ok(Value::Null)
            },
        );
        start.await.unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let marker = called.clone();
        let queued = control.source_agent_admitted(
            "source",
            tokio::time::Instant::now() + Duration::from_secs(30),
            entry_gate(admission.clone(), queued_entered.clone(), active.clone()),
            move |_| async move {
                marker.store(true, Ordering::Release);
                Ok(Value::Null)
            },
        );
        admission.fetch_or(CLOSING, Ordering::AcqRel);
        assert_eq!(entered.load(Ordering::Acquire), 2);
        release.send(()).unwrap();
        finish.await.unwrap();
        running.wait().await.unwrap();
        assert!(queued.wait().await.is_err());
        assert_ne!(queued_entered.load(Ordering::Acquire), 2);
        assert!(!called.load(Ordering::Acquire));
        // Ordinary owner closing does not pretend the independent callback's
        // terminal receipt already happened. Terminal lifetime still revokes it.
        let lifetime = CallbackLifetime(active.clone());
        drop(lifetime);
        assert!(!active.load(Ordering::Acquire));
        control.close().wait().await.unwrap();
    }

    #[tokio::test]
    async fn closing_retains_independent_source_observer_and_finally_receipts() {
        crate::test_sandbox::assert_isolated();
        let root = std::env::temp_dir().join(format!(
            "source-close-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"enableAgentBrowserAccess":false}"#,
        )
        .unwrap();
        let state = Arc::new(State::open(&root).unwrap());
        state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
        {
            let db = state.db.lock().unwrap();
            crate::events::put(
                &db,
                "thread",
                "thread",
                "thread",
                &json!({"id":"thread","providerInstanceId":"codex","activeProviderThreadId":"pt"}),
            )
            .unwrap();
            crate::events::put(&db,"providerThreads","pt","thread",&json!({"id":"pt","appThreadId":"thread","providerInstanceId":"codex","providerSessionId":"session","status":"active"})).unwrap();
            crate::events::put(
                &db,
                "providerSessions",
                "session",
                "thread",
                &json!({"id":"session","providerInstanceId":"codex","status":"ready"}),
            )
            .unwrap();
        }
        let context = state
            .native_mcp
            .obtain(&state, "thread", "codex", &root, &Value::Null)
            .await
            .unwrap();
        let mut ledger = Ledger::default();
        let descriptor = ledger
            .register(&context, NativeOperation::Act, "invocation".into())
            .unwrap();
        let agent = format!(
            "{}\0{}\0thread",
            descriptor["environmentId"].as_str().unwrap(),
            context.id()
        );
        let request = CallbackRequest::decode(&raw(json!({"requestId":"request","threadId":"thread","tabId":"tab","tabIdExplicit":true,"agentSessionId":agent,"operation":"click","input":{},"timeoutMs":15000})).unwrap(),NativeOperation::Act,true,"thread",&agent).unwrap();
        ledger
            .reserve("invocation", "callback".into(), request)
            .unwrap();
        ledger.run("invocation", "callback").unwrap();
        ledger.closing = true;
        ledger.end_source("invocation").unwrap();
        ledger.returned("invocation");
        assert!(!ledger.clean());
        assert!(ledger.finish("invocation", "callback", false).is_err());
        ledger.responded("invocation", "callback").unwrap();
        assert!(
            !ledger.clean(),
            "Execution completion is not original markUsed/finally"
        );
        ledger.finish("invocation", "callback", false).unwrap();
        assert!(ledger.clean());
        // This proves bookkeeping against actual credential/session records;
        // no synthetic executor result is represented as a physical CDP proof.
    }

    #[tokio::test]
    async fn source_observer_entry_is_also_closed_and_each_gate_is_single_use() {
        crate::test_sandbox::assert_isolated();
        let control = Control::new(None);
        let admission = Arc::new(AtomicU64::new(0));
        let entered = Arc::new(AtomicU8::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let gate = entry_gate(admission.clone(), entered.clone(), active.clone());
        control
            .source_observe_admitted(gate, |_| async { Ok(Value::Null) })
            .wait()
            .await
            .unwrap();
        assert_eq!(entered.load(Ordering::Acquire), 2);
        assert!(control
            .source_observe_admitted(
                entry_gate(admission.clone(), entered, active.clone()),
                |_| async { Ok(Value::Null) },
            )
            .wait()
            .await
            .is_err());
        admission.fetch_or(CLOSING, Ordering::AcqRel);
        assert!(control
            .source_observe_admitted(
                entry_gate(admission, Arc::new(AtomicU8::new(0)), active),
                |_| async { panic!("closed source observer entered") },
            )
            .wait()
            .await
            .is_err());
        control.close().wait().await.unwrap();
    }
}

#[cfg(test)]
#[path = "toolkit_physical_page_tests.rs"]
mod physical_page_tests;
#[cfg(test)]
#[path = "toolkit_owner_tests.rs"]
mod physical_tests;
