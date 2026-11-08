//! Opaque source-producer reservations. An observer returning is not proof that
//! an approved host callback stopped, and no reservation creates a CDP permit.
use super::{caller::Caller, toolkit_callback::CallbackRequest, NativeOperation};
use crate::native_mcp::Context;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};

pub(super) struct Binding {
    pub context: Weak<Context>,
    pub caller: Arc<Caller>,
    pub descriptor: Value,
}
pub(super) struct Invocation {
    pub binding: Arc<Binding>,
    pub tool: NativeOperation,
    pub source_ended: bool,
    pub observer_returned: bool,
    pub callbacks: BTreeSet<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallbackPhase {
    Reserved,
    Running,
    Responded,
}
pub(super) struct Callback {
    pub invocation: String,
    pub request: CallbackRequest,
    pub phase: CallbackPhase,
}
pub(super) struct RunningCallback {
    pub binding: Arc<Binding>,
    pub request: CallbackRequest,
}
#[derive(Default)]
pub(super) struct Ledger {
    pub invocations: BTreeMap<String, Invocation>,
    pub callbacks: BTreeMap<String, Callback>,
    pub bindings: Vec<Arc<Binding>>,
    pub closing: bool,
    pub unknown: bool,
}
impl Ledger {
    /// One immutable credential binding per actual Context allocation. The
    /// cache retains only Weak<Context>; callbacks never keep a process owner
    /// alive by retaining the credential context through an await.
    pub fn register(
        &mut self,
        context: &Arc<Context>,
        tool: NativeOperation,
        receipt: String,
    ) -> Result<Value, Value> {
        if self.closing
            || self.unknown
            || self.invocations.len() >= 64
            || self.invocations.contains_key(&receipt)
        {
            return Err(serde_json::json!({"code":"capability_denied","reason":"toolkit_closed"}));
        }
        let weak = Arc::downgrade(context);
        let binding = if let Some(binding) = self
            .bindings
            .iter()
            .find(|binding| Weak::ptr_eq(&binding.context, &weak))
        {
            // Revalidate the exact credential; never replace it with a successor.
            binding.caller.scope()?;
            binding.clone()
        } else {
            self.bindings.retain(|binding| {
                binding.context.strong_count() != 0 || Arc::strong_count(binding) > 1
            });
            if self.bindings.len() >= 128 {
                return Err(
                    serde_json::json!({"code":"capability_denied","reason":"toolkit_busy"}),
                );
            }
            let caller = Arc::new(Caller::capture(context)?);
            let descriptor = caller.scope()?;
            let binding = Arc::new(Binding {
                context: weak,
                caller,
                descriptor,
            });
            self.bindings.push(binding.clone());
            binding
        };
        let descriptor = binding.descriptor.clone();
        self.invocations.insert(
            receipt,
            Invocation {
                binding,
                tool,
                source_ended: false,
                observer_returned: false,
                callbacks: BTreeSet::new(),
            },
        );
        Ok(descriptor)
    }
    /// Called in wire receive order. Source end forbids new producers; already
    /// reserved callbacks may still begin after their caller stops observing.
    pub fn reserve(
        &mut self,
        invocation: &str,
        receipt: String,
        request: CallbackRequest,
    ) -> Result<(), ()> {
        let scope = self.invocations.get_mut(invocation).ok_or(())?;
        if self.closing
            || self.unknown
            || self.callbacks.len() >= 128
            || scope.source_ended
            || self.callbacks.contains_key(&receipt)
            || scope.callbacks.iter().any(|id| {
                self.callbacks
                    .get(id)
                    .is_some_and(|callback| callback.request.request_id == request.request_id)
            })
        {
            return Err(());
        }
        scope.callbacks.insert(receipt.clone());
        self.callbacks.insert(
            receipt,
            Callback {
                invocation: invocation.into(),
                request,
                phase: CallbackPhase::Reserved,
            },
        );
        Ok(())
    }
    /// Beginning a host callback is distinct from entering Control. Only the
    /// real Control gate can mint the later per-tab write authority.
    pub fn run(&mut self, invocation: &str, receipt: &str) -> Result<RunningCallback, ()> {
        let callback = self.callbacks.get_mut(receipt).ok_or(())?;
        if callback.invocation != invocation || callback.phase != CallbackPhase::Reserved {
            return Err(());
        }
        let scope = self.invocations.get(&callback.invocation).ok_or(())?;
        callback.phase = CallbackPhase::Running;
        Ok(RunningCallback {
            binding: scope.binding.clone(),
            request: callback.request.clone(),
        })
    }
    pub fn responded(&mut self, invocation: &str, receipt: &str) -> Result<(), ()> {
        let callback = self.callbacks.get_mut(receipt).ok_or(())?;
        if callback.invocation != invocation || callback.phase != CallbackPhase::Running {
            return Err(());
        }
        callback.phase = CallbackPhase::Responded;
        Ok(())
    }
    /// Only the real finally/markUsed acknowledgment terminates an executed
    /// callback. A source queue refusal may abandon an unstarted reservation.
    pub fn finish(&mut self, invocation: &str, receipt: &str, abandoned: bool) -> Result<(), ()> {
        let callback = self.callbacks.get(receipt).ok_or(())?;
        if callback.invocation != invocation
            || (abandoned && callback.phase != CallbackPhase::Reserved)
            || (!abandoned && callback.phase != CallbackPhase::Responded)
        {
            return Err(());
        }
        let invocation = callback.invocation.clone();
        self.callbacks.remove(receipt);
        self.invocations
            .get_mut(&invocation)
            .ok_or(())?
            .callbacks
            .remove(receipt);
        self.collect(&invocation);
        Ok(())
    }
    pub fn end_source(&mut self, invocation: &str) -> Result<(), ()> {
        let scope = self.invocations.get_mut(invocation).ok_or(())?;
        if scope.source_ended {
            return Err(());
        }
        scope.source_ended = true;
        self.collect(invocation);
        Ok(())
    }
    pub fn returned(&mut self, invocation: &str) {
        if let Some(scope) = self.invocations.get_mut(invocation) {
            scope.observer_returned = true;
        }
        self.collect(invocation);
    }
    fn collect(&mut self, invocation: &str) {
        if self.invocations.get(invocation).is_some_and(|scope| {
            scope.source_ended && scope.observer_returned && scope.callbacks.is_empty()
        }) {
            self.invocations.remove(invocation);
        }
        self.bindings.retain(|binding| {
            binding.context.strong_count() != 0 || Arc::strong_count(binding) > 1
        });
    }
    pub fn clean(&self) -> bool {
        !self.unknown && self.invocations.is_empty() && self.callbacks.is_empty()
    }
}
