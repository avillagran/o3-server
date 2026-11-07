//! Ordered worker scope completion tracks actual writer admission, not CDP
//! replies. The callback owner retains its real RunContext until this fence.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::watch;

#[derive(Default)]
struct State {
    sealed: bool,
    next: u64,
    pending: BTreeSet<u64>,
}
pub(super) struct Frames {
    state: Mutex<State>,
    drained: watch::Sender<bool>,
}
pub(super) struct Frame {
    owner: Weak<Frames>,
    id: u64,
    settled: std::sync::atomic::AtomicBool,
}
impl Frames {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Default::default(),
            drained: watch::channel(false).0,
        })
    }
    /// Call synchronously while consuming the ordered worker connection, before
    /// spawning the command task. A task first polled after seal still owns its
    /// earlier reservation; a later wire request cannot reserve a new one.
    pub fn reserve(self: &Arc<Self>) -> Option<Arc<Frame>> {
        let mut state = self.state.lock().unwrap();
        if state.sealed {
            return None;
        }
        state.next = state.next.checked_add(1)?;
        let id = state.next;
        state.pending.insert(id);
        Some(Arc::new(Frame {
            owner: Arc::downgrade(self),
            id,
            settled: false.into(),
        }))
    }
    pub fn seal(&self) -> watch::Receiver<bool> {
        let mut state = self.state.lock().unwrap();
        state.sealed = true;
        if state.pending.is_empty() {
            self.drained.send_replace(true);
        }
        self.drained.subscribe()
    }
    fn settled(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        state.pending.remove(&id);
        if state.sealed && state.pending.is_empty() {
            self.drained.send_replace(true);
        }
    }
}
impl Frame {
    /// Called only after the real Control gate and target reservation accepted
    /// writer begin, or when that frame is definitively refused. This does not
    /// create authority or promote a retired callback to its successor.
    pub fn settled(&self) {
        if !self.settled.swap(true, std::sync::atomic::Ordering::AcqRel) {
            if let Some(owner) = self.owner.upgrade() {
                owner.settled(self.id);
            }
        }
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        // Last owner disappearing before writer begin is a refused frame, not
        // a successful write or a known remote CDP outcome.
        self.settled();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_waits_for_prior_actual_admission_but_not_its_remote_reply() {
        let frames = Frames::new();
        let first = frames.reserve().unwrap();
        let second = frames.reserve().unwrap();
        let drained = frames.seal();
        assert!(!*drained.borrow());
        assert!(frames.reserve().is_none());
        first.settled();
        assert!(!*drained.borrow());
        second.settled();
        assert!(*drained.borrow());
        // Live frame owners and their remote replies may outlive this marker.
        assert_eq!(Arc::strong_count(&first), 1);
        first.settled();
        assert!(*frames.seal().borrow());
    }
    #[test]
    fn queued_task_and_permit_clones_keep_marker_pending_until_last_refusal() {
        let frames = Frames::new();
        let frame = frames.reserve().unwrap();
        let permit = frame.clone();
        let drained = frames.seal();
        drop(frame);
        assert!(!*drained.borrow());
        drop(permit);
        assert!(*drained.borrow());
    }
    #[test]
    fn empty_scope_closes_immediately_and_never_reopens() {
        let frames = Frames::new();
        assert!(*frames.seal().borrow());
        assert!(frames.reserve().is_none());
        assert!(*frames.seal().borrow());
    }
}
