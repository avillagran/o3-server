//! Per-target retained Page integration with the existing native control scheduler.
//! This is an owner API, never an MCP argument-driven driver selector.
use super::{CloseReceipt, Fault, OperationResult, Worker};
use crate::preview_control::runtime::Control;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
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
        let action = self.0.control.agent_admitted(
            caller,
            Some(tokio::time::Instant::now() + native_timeout),
            move |admission| async move {
                let generation = admission.generation();
                let mut value = match synchronize(&state).await {
                    Ok(()) => {
                        state
                            .worker
                            .invoke_native(&operation, input, native_timeout, update_current_tab)
                            .await
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
            },
        );
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
