//! Per-target retained Page integration with the existing native control scheduler.
//! This is an owner API, never an MCP argument-driven driver selector.
use super::{source_deadline::FrozenDeadline, toolkit_owner::SourceCallback, NativePage};
use super::{CloseReceipt, Fault, OperationResult, Worker};
use crate::preview_control::runtime::Control;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::watch;

struct State {
    worker: Worker,
    control: Control,
    generation: tokio::sync::Mutex<Option<u64>>,
    stop: watch::Sender<bool>,
}
impl Drop for State {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
/// One Page/Keyboard survives ordinary controller generations. A replacement
/// browser target requires a new owner grant and a new ControlledPage.
#[derive(Clone)]
pub struct ControlledPage(Arc<State>);
#[derive(Debug, PartialEq)]
pub enum Error {
    Control(Value),
    Transport(Fault),
}
impl ControlledPage {
    pub fn new(worker: Worker, control: Control) -> Self {
        let (stop, mut stopping) = watch::channel(false);
        let state = Arc::new(State {
            worker,
            control,
            generation: tokio::sync::Mutex::new(None),
            stop,
        });
        let weak = Arc::downgrade(&state);
        let mut changes = state.control.subscribe();
        tokio::spawn(async move {
            let initially_closed = changes.borrow().closed;
            if initially_closed {
                if let Some(state) = weak.upgrade() {
                    state.control.close().wait().await.ok();
                    state.worker.close().await;
                }
                return;
            }
            loop {
                tokio::select! {
                    _ = stopping.changed() => break,
                    changed = changes.changed() => {
                        let Some(state) = weak.upgrade() else { break; };
                        let closed = changed.is_err() || changes.borrow().closed;
                        if closed {
                            // Native close fences arrivals and drains the entered tail.
                            let _ = state.control.close().wait().await;
                            state.worker.close().await;
                            break;
                        }
                        if synchronize(&state).await.is_err() {
                            state.worker.close().await;
                            break;
                        }
                    }
                }
            }
        });
        Self(state)
    }
    /// Caller admission/read-vs-act scope checks belong to the actual broker
    /// before this owner-only entry point. The existing scheduler still proves
    /// agent ownership, generation and deadline at execution, not submission.
    pub async fn agent(
        &self,
        caller: &str,
        operation: &str,
        input: Value,
        native_timeout: Duration,
        update_current_tab: bool,
    ) -> Result<OperationResult, Error> {
        self.run(
            Some(caller),
            operation,
            input,
            native_timeout,
            update_current_tab,
        )
        .await
    }
    /// Source readonly operations observe without joining the agent tail. The
    /// broker chooses this entry from its fixed operation, never a model flag.
    pub async fn observe(
        &self,
        operation: &str,
        input: Value,
        native_timeout: Duration,
        update_current_tab: bool,
    ) -> Result<OperationResult, Error> {
        if !matches!(operation, "status" | "snapshot" | "waitFor") {
            return Err(Error::Control(
                json!({"reason":"readOperationRequired","message":"Expected a native readonly operation"}),
            ));
        }
        self.run(None, operation, input, native_timeout, update_current_tab)
            .await
    }
    async fn run(
        &self,
        caller: Option<&str>,
        operation: &str,
        input: Value,
        native_timeout: Duration,
        update_current_tab: bool,
    ) -> Result<OperationResult, Error> {
        if native_timeout.is_zero() || native_timeout > Duration::from_secs(120) {
            return Err(Error::Transport(Fault::Deadline));
        }
        let response_deadline = tokio::time::Instant::now()
            + native_timeout
            + if update_current_tab {
                super::HOST_RESPONSE_GRACE
            } else {
                Duration::ZERO
            };
        let result = Arc::new(Mutex::new(None));
        let output = result.clone();
        let state = self.0.clone();
        let operation = operation.to_owned();
        let work = move |admission: crate::preview_control::runtime::RunContext| async move {
            let generation = admission.generation();
            let mut value = match synchronize(&state).await {
                Ok(()) => {
                    if update_current_tab {
                        state
                            .worker
                            .invoke_native(&operation, input, native_timeout, true)
                            .await
                    } else {
                        state
                            .worker
                            .invoke_execution(&operation, input, native_timeout)
                            .await
                    }
                }
                Err(error) => Err(error),
            };
            // A takeover invalidates immediately and again after the old
            // callback drains, as in the native runOperation finally block.
            if generation != state.control.snapshot().generation {
                if let Err(error) = state.worker.invalidate_refs().await {
                    if value.is_ok() {
                        value = Err(error);
                    }
                }
            }
            *output.lock().unwrap() = Some(value);
            // The scheduler receives no scalar approximation of JavaScript
            // output. This per-action sidecar is read only after its receipt.
            Ok(Value::Null)
        };
        let action = match caller {
            Some(caller) => self.0.control.agent_admitted(
                caller,
                Some(tokio::time::Instant::now() + native_timeout),
                work,
            ),
            None => self.0.control.observe_admitted(work),
        };
        match tokio::time::timeout_at(response_deadline, action.wait()).await {
            Ok(result) => {
                result.map_err(Error::Control)?;
            }
            Err(_) => {
                // The call can already have mutated the page. Fence and close
                // only its captured grant; never replay or certify its outcome.
                if update_current_tab {
                    self.0.worker.close().await;
                }
                return Err(Error::Transport(Fault::Deadline));
            }
        }
        let completed = result.lock().unwrap().take().ok_or_else(|| {
            Error::Control(json!({"message":"Native browser action ended without a result"}))
        })?;
        completed.map_err(Error::Transport)
    }
    /// Owner-side acknowledgment of generation reference invalidation. This
    /// grants no right to act and never replaces the scheduler's admission.
    pub async fn synchronize_references(&self) -> Result<(), Fault> {
        synchronize(&self.0).await
    }
    /// The actual source callback and physical tab are captured by the owned
    /// factory. No JSON flag chooses this admission or bypasses its scheduler.
    pub(crate) async fn source(
        &self,
        callback: SourceCallback,
        page: NativePage,
        physical: crate::server_browser::Page,
        owner: Weak<crate::store::State>,
        deadline: FrozenDeadline,
    ) -> Result<OperationResult, Error> {
        if !self.0.control.same_identity(&page.control())
            || !self.0.control.same_identity(&physical.control())
        {
            return Err(Error::Transport(Fault::Revoked));
        }
        callback.validate().map_err(Error::Control)?;
        let request = callback.request().clone();
        let snapshot = self.0.control.snapshot();
        let read = matches!(
            request.operation.as_str(),
            "status" | "snapshot" | "waitFor"
        );
        let can_act_now = snapshot
            .agent_id
            .as_deref()
            .is_none_or(|agent| agent == request.agent_session_id)
            && snapshot.controller.is_none();
        let observe = read
            && snapshot.agent_id.as_deref() != Some(request.agent_session_id.as_str())
            && !can_act_now;
        let output = Arc::new(Mutex::new(None));
        let result = output.clone();
        let state = self.0.clone();
        let agent = request.agent_session_id.clone();
        let entry = callback.entry_gate();
        let work = move |admission: crate::preview_control::runtime::RunContext| async move {
            let generation = admission.generation();
            let value = async {
                callback.validate().map_err(Error::Control)?;
                let owner = owner.upgrade().ok_or(Error::Transport(Fault::Revoked))?;
                if !owner.streams.p2.preview.browser.current(&physical) {
                    return Err(Error::Transport(Fault::Revoked));
                }
                if !physical.dialog_status().map_err(Error::Control)?.is_null() {
                    return Err(Error::Control(json!({
                        "_tag":"BrowserControlInterrupted",
                        "name":"BrowserControlInterrupted",
                        "message":if observe {
                            "A browser dialog is pending. Read preview_status."
                        } else {
                            "A browser dialog is pending. Read preview_status and use preview_dialog first."
                        },
                        "reason":"dialogPending"
                    })));
                }
                let invocation = callback.admitted(&page, &admission).map_err(Error::Control)?;
                synchronize(&state).await.map_err(Error::Transport)?;
                // The original callback is now entered. Its admission deadline
                // is never restarted or checked again after this await.
                let value = state
                    .worker
                    .invoke_execution_scoped_raw(
                        &invocation,
                        &request.operation,
                        request.input,
                        Duration::from_millis(request.timeout_ms),
                    )
                    .await
                    .map_err(Error::Transport)?;
                callback.validate().map_err(Error::Control)?;
                Ok(value)
            }
            .await;
            let value = if generation != state.control.snapshot().generation {
                match state.worker.invalidate_refs().await {
                    Err(error) if value.is_ok() => Err(Error::Transport(error)),
                    _ => value,
                }
            } else {
                value
            };
            *result.lock().unwrap() = Some(value);
            Ok(Value::Null)
        };
        let action = if observe {
            self.0.control.source_observe_admitted(entry, work)
        } else {
            self.0.control.source_agent_admitted(
                &agent,
                tokio::time::Instant::from_std(deadline.std_instant()),
                entry,
                work,
            )
        };
        // Caller observation never completes or cancels an entered callback.
        // Its real worker terminal receipt is retained inside the Control job.
        action.wait().await.map_err(Error::Control)?;
        let value = output.lock().unwrap().take().ok_or_else(|| {
            Error::Control(json!({"message":"Native browser action ended without a result"}))
        })?;
        value
    }
    /// Resource retirement is separate from closing a physical UI tab. The
    /// toolkit owner drains its callback ledger before requesting this receipt.
    pub(crate) async fn retire_worker(&self) -> CloseReceipt {
        let _ = self.0.stop.send(true);
        self.0.worker.close().await
    }
    /// Fence new callbacks and drain the admitted native tail before retiring
    /// this worker. Hard transport/watchdog faults still retire it directly.
    pub async fn close(&self) -> CloseReceipt {
        let _ = self.0.stop.send(true);
        let _ = self.0.control.close().wait().await;
        self.0.worker.close().await
    }
}
async fn synchronize(state: &State) -> Result<(), Fault> {
    let mut previous = state.generation.lock().await;
    loop {
        let snapshot = state.control.snapshot();
        if *previous == Some(snapshot.generation) {
            return Ok(());
        }
        state.worker.invalidate_refs().await?;
        *previous = Some(snapshot.generation);
        // A takeover can occur while the acknowledgment is in flight. Never
        // certify the newer generation with the older invalidation receipt.
    }
}
