//! Owner-library native CDP sessions on the existing actor. This is not an MCP
//! endpoint; provider tickets require the separate dynamic caller admission.
use super::*;
use crate::native_page_worker::{cdp::Packet, JavaScriptJson};
use crate::preview_control::runtime::{Control, RunContext};
use native_grant::{Reservation, State as Grants};
use std::sync::{Arc, Mutex as SyncMutex, Weak};

#[derive(Clone)]
pub struct NativePage {
    inner: Arc<Owner>,
}
pub(super) struct Owner {
    target: String,
    borrowed_session: String,
    thread: String,
    tab: String,
    registry: Weak<SyncMutex<events::Registry>>,
    receipts: SyncMutex<BTreeMap<String, Ticket>>,
    actor: mpsc::WeakSender<exact::Request>,
    done: tokio::sync::watch::Receiver<bool>,
    control: Control,
    wake: Arc<tokio::sync::Notify>,
    changed: tokio::sync::Notify,
    grants: SyncMutex<Grants>,
    events: tokio::sync::broadcast::Sender<Packet>,
    close: Mutex<()>,
}
#[derive(Clone)]
pub struct NativeInvocation {
    owner: Arc<Owner>,
    context: RunContext,
    caller: Option<Caller>,
}
/// Fixed broker operation classification. No wire flag chooses read admission.
#[derive(Clone, Copy)]
pub enum NativeOperation {
    Status,
    Open,
    Dialog,
    Navigate,
    Resize,
    SetAppearance,
    Snapshot,
    Act,
    Evaluate,
    WaitFor,
    RecordingStart,
    RecordingStop,
}
impl NativeOperation {
    fn writes(self) -> bool {
        !matches!(self, Self::Status | Self::Snapshot | Self::WaitFor)
    }
}
#[derive(Clone)]
struct Caller {
    state: Weak<crate::store::State>,
    authority: crate::mcp_access::Authority,
    write: bool,
}
impl Caller {
    fn valid(&self, owner: &Owner) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        if self.authority.native_thread() != Some(owner.thread.as_str())
            || self.authority.browser().is_none()
        {
            return false;
        }
        let Some(registry) = owner.registry.upgrade() else {
            return false;
        };
        let same_owner = state
            .streams
            .p2
            .preview
            .browser
            .tabs
            .lock()
            .unwrap()
            .get(&(owner.thread.clone(), owner.tab.clone()))
            .and_then(|tab| tab.events.as_ref())
            .is_some_and(|actual| Arc::ptr_eq(actual, &registry));
        same_owner && self.authority.browser_admission(&state, self.write).is_ok()
    }
}
#[derive(Clone)]
struct Ticket {
    context: RunContext,
    caller: Option<Caller>,
}
/// Opaque receipt for the sealed worker channel. It carries no browser endpoint
/// and cannot be constructed from model JSON or reused on another captured Page.
pub struct NativeReceipt {
    owner: Weak<Owner>,
    value: String,
}
impl NativeReceipt {
    pub fn worker_receipt(&self) -> &str {
        &self.value
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct NativeCloseReceipt {
    pub clean: bool,
    pub pending_commands: usize,
    pub owned_sessions: usize,
    pub pending_attachments: usize,
    pub refused_sessions: usize,
    pub receipt_totals: [u64; 5],
}
struct Tracked {
    owner: Arc<Owner>,
    reservation: Reservation,
}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.owner
            .grants
            .lock()
            .unwrap()
            .cancelled(&self.reservation);
        self.owner.changed.notify_waiters();
    }
}
impl exact::Observer for Tracked {
    fn completed(&self, packet: &Packet) {
        let _ = self
            .owner
            .grants
            .lock()
            .unwrap()
            .completed(&self.reservation, packet);
        self.owner.changed.notify_waiters();
    }
    fn cancelled(&self, unknown: bool) {
        let mut grants = self.owner.grants.lock().unwrap();
        if unknown {
            grants.wire_unknown(&self.reservation);
        } else {
            grants.cancelled(&self.reservation);
        }
        drop(grants);
        self.owner.changed.notify_waiters();
    }
}
struct FrameAuthority {
    tracked: Arc<Tracked>,
    context: Option<RunContext>,
    caller: Option<Caller>,
}
impl write::Authority for FrameAuthority {
    fn valid(&self) -> bool {
        let owner = &self.tracked.owner;
        self.caller
            .as_ref()
            .is_none_or(|caller| caller.valid(owner))
            && !*owner.done.borrow()
            && self
                .context
                .as_ref()
                .is_none_or(|context| owner.control.owns_context(context))
            && owner
                .grants
                .lock()
                .unwrap()
                .may_start(&self.tracked.reservation, self.context.is_none())
    }
    fn begin(&self) -> bool {
        if !self.valid() {
            return false;
        }
        let owner = &self.tracked.owner;
        if self
            .context
            .as_ref()
            .is_some_and(|context| !owner.control.authorize_context_write(context))
        {
            return false;
        }
        let mut grants = owner.grants.lock().unwrap();
        if !grants.may_start(&self.tracked.reservation, self.context.is_none()) {
            return false;
        }
        grants.started(&self.tracked.reservation);
        true
    }
}
fn fault(reason: &str) -> Value {
    json!({"message":"Owned native Page command did not complete", "reason":reason})
}
impl Owner {
    async fn send(
        self: &Arc<Self>,
        admission: Option<Ticket>,
        method: &str,
        params: JavaScriptJson,
        session: Option<String>,
        budget: Duration,
        cleanup: Option<Reservation>,
    ) -> Result<Packet, Value> {
        if budget.is_zero() || budget > Duration::from_secs(121) || *self.done.borrow() {
            return Err(fault("closed_or_deadline"));
        }
        let reservation = match cleanup {
            Some(reservation) => reservation,
            None => self
                .grants
                .lock()
                .unwrap()
                .reserve(method, &params, session.as_deref())
                .map_err(|_| fault("scope"))?,
        };
        let tracked = Arc::new(Tracked {
            owner: self.clone(),
            reservation,
        });
        let authority = Arc::new(FrameAuthority {
            tracked: tracked.clone(),
            context: admission
                .as_ref()
                .map(|admission| admission.context.clone()),
            caller: admission.and_then(|admission| admission.caller),
        });
        let expires = tokio::time::Instant::now() + budget;
        let permit =
            write::Permit::leased(expires, self.done.clone(), authority, self.wake.clone());
        let _cancel = permit.hold();
        let actor = self.actor.upgrade().ok_or_else(|| fault("closed"))?;
        let (reply, result) = oneshot::channel();
        tokio::time::timeout_at(
            expires,
            actor.send(exact::Request {
                method: method.into(),
                params,
                session,
                expires,
                reply,
                permit,
                observer: Some(tracked),
            }),
        )
        .await
        .map_err(|_| fault("deadline"))?
        .map_err(|_| fault("closed"))?;
        tokio::time::timeout_at(expires, result)
            .await
            .map_err(|_| fault("deadline"))?
            .map_err(|_| fault("closed"))?
            .map_err(|_| fault("transport"))
    }
    fn receipt(&self) -> NativeCloseReceipt {
        let grants = self.grants.lock().unwrap();
        NativeCloseReceipt {
            clean: grants.clean(),
            pending_commands: grants.requests_pending(),
            owned_sessions: grants.detach_sessions().len(),
            pending_attachments: grants.attachments_pending(),
            refused_sessions: grants.refused_sessions().len(),
            receipt_totals: grants.receipt_totals(),
        }
    }
}
impl NativePage {
    pub fn control(&self) -> Control {
        self.inner.control.clone()
    }
    /// Owner-library admission from this Page's real scheduler, not a wire epoch.
    pub fn admitted(&self, context: &RunContext) -> Result<NativeInvocation, Value> {
        if !self.inner.control.owns_context(context) {
            return Err(fault("callback_scope"));
        }
        Ok(NativeInvocation {
            owner: self.inner.clone(),
            context: context.clone(),
            caller: None,
        })
    }
    /// Only an opaque context held by the actual provider process can mint this
    /// admission. Scope.caller and capability availability are rechecked for
    /// every frame, independently of ordinary human generation changes.
    pub fn admitted_provider(
        &self,
        context: &RunContext,
        provider: &crate::native_mcp::Context,
        operation: NativeOperation,
    ) -> Result<NativeInvocation, Value> {
        let mut invocation = self.admitted(context)?;
        let (state, authority) = provider.page_authority();
        let caller = Caller {
            state,
            authority,
            write: operation.writes(),
        };
        if !caller.valid(&self.inner) {
            return Err(fault("provider_scope"));
        }
        invocation.caller = Some(caller);
        Ok(invocation)
    }
    /// Resolve only a receipt minted by this owner, preserving its original
    /// callback and caller. There is no current-scope relabeling fallback.
    pub async fn command_receipt(
        &self,
        receipt: &str,
        method: &str,
        params: JavaScriptJson,
        session: Option<String>,
        budget: Duration,
    ) -> Result<Packet, Value> {
        let ticket = self
            .inner
            .receipts
            .lock()
            .unwrap()
            .get(receipt)
            .cloned()
            .ok_or_else(|| fault("receipt"))?;
        NativeInvocation {
            owner: self.inner.clone(),
            context: ticket.context,
            caller: ticket.caller,
        }
        .command(method, params, session, budget)
        .await
    }
    pub fn finish_receipt(&self, receipt: NativeReceipt) -> Result<(), Value> {
        if !receipt
            .owner
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, &self.inner))
        {
            return Err(fault("receipt_owner"));
        }
        self.inner
            .receipts
            .lock()
            .unwrap()
            .remove(&receipt.value)
            .ok_or_else(|| fault("receipt"))?;
        Ok(())
    }
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<Packet> {
        self.inner.events.subscribe()
    }
    pub fn target(&self) -> &str {
        &self.inner.target
    }
    /// Drain real completions before owned detach. The shared browser and the
    /// tab's borrowed legacy session are never cleanup targets of this grant.
    pub async fn close(&self, budget: Duration) -> NativeCloseReceipt {
        let expires = tokio::time::Instant::now() + budget.min(Duration::from_secs(121));
        let Ok(_close) = tokio::time::timeout_at(expires, self.inner.close.lock()).await else {
            return self.inner.receipt();
        };
        self.inner.grants.lock().unwrap().close();
        self.inner.wake.notify_one();
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (pending, sessions) = {
                let grants = self.inner.grants.lock().unwrap();
                (grants.requests_pending(), grants.detach_sessions())
            };
            if pending == 0 {
                let Some(session) = sessions.first() else {
                    return self.inner.receipt();
                };
                let reservation = self.inner.grants.lock().unwrap().cleanup_detach(session);
                let Ok(reservation) = reservation else {
                    break;
                };
                let params =
                    JavaScriptJson::parse(json!({"sessionId":session}).to_string()).unwrap();
                let remaining = expires.saturating_duration_since(tokio::time::Instant::now());
                let wire_session = reservation.session.clone();
                let detached = self
                    .inner
                    .send(
                        None,
                        "Target.detachFromTarget",
                        params,
                        wire_session,
                        remaining,
                        Some(reservation),
                    )
                    .await;
                if !matches!(detached, Ok(packet) if matches!(packet.payload(), Some(crate::native_page_worker::cdp::Payload::Result(_))))
                {
                    break;
                }
            } else if tokio::time::timeout_at(expires, changed).await.is_err() {
                break;
            }
            if tokio::time::Instant::now() >= expires {
                break;
            }
        }
        self.inner.grants.lock().unwrap().mark_unknown();
        self.inner.receipt()
    }
}
impl NativeInvocation {
    pub fn issue_receipt(&self) -> Result<NativeReceipt, Value> {
        if !self.owner.control.owns_context(&self.context)
            || self
                .caller
                .as_ref()
                .is_some_and(|caller| !caller.valid(&self.owner))
        {
            return Err(fault("callback_scope"));
        }
        let mut receipts = self.owner.receipts.lock().unwrap();
        receipts.retain(|_, ticket| self.owner.control.owns_context(&ticket.context));
        if receipts.len() >= 256 {
            return Err(fault("receipt_capacity"));
        }
        let mut bytes = [0u8; 24];
        getrandom::fill(&mut bytes).map_err(|_| fault("entropy"))?;
        let value: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        if receipts.contains_key(&value) {
            return Err(fault("entropy"));
        }
        receipts.insert(
            value.clone(),
            Ticket {
                context: self.context.clone(),
                caller: self.caller.clone(),
            },
        );
        Ok(NativeReceipt {
            owner: Arc::downgrade(&self.owner),
            value,
        })
    }
    pub async fn command(
        &self,
        method: &str,
        params: JavaScriptJson,
        session: Option<String>,
        budget: Duration,
    ) -> Result<Packet, Value> {
        if !self.owner.control.owns_context(&self.context)
            || self
                .caller
                .as_ref()
                .is_some_and(|caller| !caller.valid(&self.owner))
        {
            return Err(fault("callback_scope"));
        }
        self.owner
            .send(
                Some(Ticket {
                    context: self.context.clone(),
                    caller: self.caller.clone(),
                }),
                method,
                params,
                session,
                budget,
                None,
            )
            .await
    }
}
impl crate::preview::Manager {
    /// Trusted owner capture. No public RPC accepts this target or actor handle.
    pub fn capture_native_page(&self, thread: &str, tab: &str) -> Result<NativePage, Value> {
        let page = self.browser.page(thread, tab)?;
        let target = page.tab.target.clone().ok_or_else(|| fault("target"))?;
        let registry = page.tab.events.as_ref().ok_or_else(|| fault("events"))?;
        let registry_weak = Arc::downgrade(registry);
        let mut registry = registry.lock().unwrap();
        if let Some(owner) = registry.native.get(&target) {
            if owner.thread != thread {
                return Err(fault("thread_scope"));
            }
            return Ok(NativePage {
                inner: owner.clone(),
            });
        }
        let owner = Arc::new(Owner {
            target: target.clone(),
            borrowed_session: page
                .tab
                .session
                .clone()
                .ok_or_else(|| fault("borrowed_session"))?,
            thread: thread.into(),
            tab: tab.into(),
            registry: registry_weak,
            receipts: Default::default(),
            actor: page.tab.exact_tx.downgrade(),
            done: page.tab.done.clone(),
            control: page.control(),
            wake: page.tab.exact_wake.clone(),
            changed: Default::default(),
            grants: SyncMutex::new(Grants::captured(target.clone()).map_err(|_| fault("target"))?),
            events: tokio::sync::broadcast::channel(64).0,
            close: Mutex::new(()),
        });
        registry.native.insert(target, owner.clone());
        Ok(NativePage { inner: owner })
    }
}
/// Consume only events for newly owned sessions. All other actor traffic keeps
/// its existing scalar decoder and legacy tab lifecycle consumer.
pub(super) fn route(registry: &events::Shared, bytes: &[u8]) -> bool {
    let Ok(packet) = Packet::decode(bytes, 256 * 1024 * 1024) else {
        return false;
    };
    if packet.id().is_some() {
        return false;
    }
    let owners: Vec<_> = registry.lock().unwrap().native.values().cloned().collect();
    for owner in owners {
        let owned = owner
            .grants
            .lock()
            .unwrap()
            .related_session_event(&packet)
            .unwrap_or(false);
        let borrowed = packet.session() == Some(owner.borrowed_session.as_str());
        let root_related = packet.session().is_none()
            && owner
                .grants
                .lock()
                .unwrap()
                .related_root_event(&packet)
                .unwrap_or(false);
        if owned || root_related {
            let forwarded = owner.grants.lock().unwrap().event(&packet);
            owner.changed.notify_waiters();
            if forwarded == Ok(true) && owner.events.receiver_count() > 0 {
                let _ = owner.events.send(packet.clone());
            }
        }
        // A duplicate application event on the borrowed observer must not make
        // a lone UTF-16 value retire the shared profile. Valid legacy lifecycle
        // events continue to their original decoder. This grants no session.
        if owned || ((borrowed || root_related) && crate::cdp_dialog_json::decode(bytes).is_err()) {
            return true;
        }
    }
    false
}
pub(super) fn stopped(registry: &events::Shared) {
    let owners: Vec<_> = registry.lock().unwrap().native.values().cloned().collect();
    for owner in owners {
        let mut grants = owner.grants.lock().unwrap();
        grants.close();
        if grants.requests_pending() != 0 || !grants.detach_sessions().is_empty() {
            grants.mark_unknown();
        }
        drop(grants);
        owner.changed.notify_waiters();
    }
}
