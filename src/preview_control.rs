//! Immediate browser-control reservations with externally owned asynchronous work.
pub mod runtime;
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};

pub type ActionId = u64;
pub type DependencyId = u64;
pub type CallbackId = u64;
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Generation {
        controller: Option<String>,
        generation: u64,
    },
    Run {
        action: ActionId,
        callback: CallbackId,
        input: Value,
    },
    Complete {
        action: ActionId,
        result: Result<Value, Value>,
    },
}
#[derive(Clone)]
enum Guard {
    Agent { caller: String, generation: u64 },
    Human { viewer: String, generation: u64 },
    System,
    Release,
}
struct Job {
    id: ActionId,
    callback: Option<CallbackId>,
    input: Value,
    guard: Guard,
    deadline: Option<f64>,
    discard_result: bool,
    waiting: BTreeSet<DependencyId>,
    started: bool,
    latest: Option<(String, String)>,
}
/// The driver owns callbacks and polls `drain` after synchronous submissions.
/// Dropping a caller's result receiver never removes queued or running work.
pub struct Kernel {
    agent: Option<String>,
    controller: Option<String>,
    generation: u64,
    closed: bool,
    sequence: ActionId,
    queue: VecDeque<Job>,
    running: Option<Job>,
    observers: BTreeSet<ActionId>,
    latest: Option<ActionId>,
    pending: BTreeSet<DependencyId>,
    handoff: BTreeSet<DependencyId>,
    barrier: BTreeSet<DependencyId>,
    closing: Vec<ActionId>,
    events: Vec<Event>,
}
fn interrupted(message: &str, reason: &str) -> Value {
    json!({"_tag":"BrowserControlInterrupted","name":"BrowserControlInterrupted","message":message,"reason":reason})
}
fn closed() -> Value {
    interrupted("This browser tab is closed.", "closed")
}
fn changed() -> Value {
    interrupted(
        "Browser control changed. Refresh the snapshot before trying again.",
        "interrupted",
    )
}
impl Kernel {
    pub fn new(agent: Option<String>) -> Self {
        Self {
            agent,
            controller: None,
            generation: 0,
            closed: false,
            sequence: 0,
            queue: VecDeque::new(),
            running: None,
            observers: BTreeSet::new(),
            latest: None,
            pending: BTreeSet::new(),
            handoff: BTreeSet::new(),
            barrier: BTreeSet::new(),
            closing: Vec::new(),
            events: Vec::new(),
        }
    }
    pub fn agent_id(&self) -> Option<&str> {
        self.agent.as_deref()
    }
    pub fn controller(&self) -> Option<&str> {
        self.controller.as_deref()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn agent_may_act(&self, caller: &str) -> bool {
        self.agent.as_deref().is_none_or(|agent| agent == caller)
    }
    pub fn agent_can_act_now(&self, caller: &str) -> bool {
        self.agent_may_act(caller) && self.controller.is_none()
    }
    fn id(&mut self) -> ActionId {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }
    fn complete(&mut self, action: ActionId, result: Result<Value, Value>) {
        self.events.push(Event::Complete { action, result });
    }
    fn rejected(&mut self, error: Value) -> ActionId {
        let id = self.id();
        self.complete(id, Err(error));
        id
    }
    fn immediate(&mut self) -> ActionId {
        let id = self.id();
        self.complete(id, Ok(Value::Null));
        id
    }
    fn change_owner(&mut self, owner: Option<String>) {
        self.latest = None;
        self.controller = owner;
        self.generation = self.generation.wrapping_add(1);
        self.events.push(Event::Generation {
            controller: self.controller.clone(),
            generation: self.generation,
        });
    }
    fn enqueue(
        &mut self,
        callback: Option<CallbackId>,
        input: Value,
        guard: Guard,
        deadline: Option<f64>,
        discard_result: bool,
    ) -> ActionId {
        self.latest = None;
        let id = self.id();
        self.queue.push_back(Job {
            id,
            callback,
            input,
            guard,
            deadline,
            discard_result,
            waiting: BTreeSet::new(),
            started: false,
            latest: None,
        });
        id
    }
    pub fn agent(
        &mut self,
        caller: &str,
        callback: CallbackId,
        input: Value,
        deadline: Option<f64>,
    ) -> ActionId {
        if !self.agent_may_act(caller) {
            return self.rejected(interrupted(
                "This tab belongs to another agent.",
                "agentMismatch",
            ));
        }
        if self.controller.is_some() {
            return self.rejected(interrupted("A human controls this tab.", "humanControl"));
        }
        if self.closed {
            return self.rejected(closed());
        }
        self.enqueue(
            Some(callback),
            input,
            Guard::Agent {
                caller: caller.into(),
                generation: self.generation,
            },
            deadline,
            false,
        )
    }
    pub fn human(&mut self, viewer: &str, callback: CallbackId, input: Value) -> ActionId {
        self.human_inner(viewer, Some(callback), input, false)
    }
    fn human_inner(
        &mut self,
        viewer: &str,
        callback: Option<CallbackId>,
        input: Value,
        discard: bool,
    ) -> ActionId {
        if self.closed {
            return self.rejected(closed());
        }
        if self.controller.as_deref() != Some(viewer) {
            return self.rejected(interrupted(
                "You do not control this browser tab.",
                "interrupted",
            ));
        }
        self.enqueue(
            callback,
            input,
            Guard::Human {
                viewer: viewer.into(),
                generation: self.generation,
            },
            None,
            discard,
        )
    }
    pub fn human_latest(
        &mut self,
        viewer: &str,
        key: &str,
        callback: CallbackId,
        input: Value,
        merge: impl FnOnce(&Value, Value) -> Value,
    ) -> ActionId {
        if self.closed || self.controller.as_deref() != Some(viewer) {
            return self.human(viewer, callback, input);
        }
        if let Some(job) = self
            .latest
            .and_then(|id| self.queue.iter_mut().find(|job| job.id == id))
            .filter(|job| {
                job.latest
                    .as_ref()
                    .is_some_and(|(owner, kind)| owner == viewer && kind == key)
            })
        {
            job.input = merge(&job.input, input);
            job.callback = Some(callback);
            return job.id;
        }
        let id = self.human_inner(viewer, Some(callback), input, true);
        self.queue.back_mut().unwrap().latest = Some((viewer.into(), key.into()));
        self.latest = Some(id);
        id
    }
    pub fn system(&mut self, callback: CallbackId, input: Value) -> ActionId {
        if self.closed {
            return self.rejected(closed());
        }
        self.enqueue(Some(callback), input, Guard::System, None, false)
    }
    pub fn observe(&mut self, callback: CallbackId, input: Value) -> ActionId {
        if self.closed {
            return self.rejected(closed());
        }
        let action = self.id();
        self.observers.insert(action);
        self.events.push(Event::Run {
            action,
            callback,
            input,
        });
        action
    }
    pub fn take(&mut self, viewer: &str) -> ActionId {
        if self.closed {
            return self.rejected(closed());
        }
        if self
            .controller
            .as_deref()
            .is_some_and(|owner| owner != viewer)
        {
            return self.rejected(interrupted(
                "Another viewer controls this browser tab.",
                "interrupted",
            ));
        }
        if self.controller.as_deref() != Some(viewer) {
            self.change_owner(Some(viewer.into()));
        }
        self.human_inner(viewer, None, Value::Null, true)
    }
    pub fn release(&mut self, viewer: &str, after_drain: Option<CallbackId>) -> ActionId {
        if self.closed {
            return self.rejected(closed());
        }
        if self.controller.as_deref() != Some(viewer) {
            return self.rejected(interrupted(
                "Only the controlling viewer can release this browser tab.",
                "interrupted",
            ));
        }
        self.change_owner(None);
        self.enqueue(after_drain, Value::Null, Guard::Release, None, true)
    }
    pub fn disconnect(&mut self, viewer: &str, after_drain: Option<CallbackId>) -> ActionId {
        if !self.closed && self.controller.as_deref() == Some(viewer) {
            self.release(viewer, after_drain)
        } else {
            self.immediate()
        }
    }
    pub fn close(&mut self) -> ActionId {
        if !self.closed {
            self.closed = true;
            self.change_owner(None);
        }
        let id = self.id();
        self.closing.push(id);
        id
    }
    pub fn track(&mut self, dependency: DependencyId) {
        self.pending.insert(dependency);
    }
    pub fn track_until_handoff(&mut self, dependency: DependencyId) {
        self.handoff.insert(dependency);
    }
    pub fn settle_tracked(&mut self, dependency: DependencyId) {
        self.pending.remove(&dependency);
        self.handoff.remove(&dependency);
        self.barrier.remove(&dependency);
        if let Some(job) = self.running.as_mut() {
            job.waiting.remove(&dependency);
        }
    }
    fn check(&self, job: &Job, now: &mut impl FnMut() -> f64) -> Result<(), Value> {
        if !matches!(job.guard, Guard::Release) && self.closed {
            return Err(closed());
        }
        let allowed = match &job.guard {
            Guard::Agent { caller, generation } => {
                *generation == self.generation && self.agent_can_act_now(caller)
            }
            Guard::Human { viewer, generation } => {
                *generation == self.generation && self.controller.as_deref() == Some(viewer)
            }
            Guard::System | Guard::Release => true,
        };
        if !allowed {
            return Err(changed());
        }
        if job.deadline.is_some_and(|deadline| now() >= deadline) {
            return Err(
                json!({"name":"TimeoutError","message":"This browser request expired before it could start."}),
            );
        }
        Ok(())
    }
    /// Run ready queue transitions without polling or borrowing a caller future.
    pub fn drain(&mut self, now: f64) -> Vec<Event> {
        self.drain_with_clock(|| now)
    }
    pub fn drain_with_clock(&mut self, mut now: impl FnMut() -> f64) -> Vec<Event> {
        loop {
            if !self.barrier.is_empty() {
                break;
            }
            if let Some(job) = self.running.as_mut() {
                if job.started || !job.waiting.is_empty() {
                    break;
                }
                job.started = true;
                if let Some(callback) = job.callback {
                    self.events.push(Event::Run {
                        action: job.id,
                        callback,
                        input: job.input.clone(),
                    });
                    break;
                }
                let id = job.id;
                self.finish_action(id, Ok(Value::Null));
                continue;
            }
            let Some(mut job) = self.queue.pop_front() else {
                let closing = std::mem::take(&mut self.closing);
                for id in closing {
                    self.complete(id, Ok(Value::Null));
                }
                break;
            };
            if self.latest == Some(job.id) {
                self.latest = None;
            }
            if let Err(error) = self.check(&job, &mut now) {
                self.complete(job.id, Err(error));
                self.barrier = self.pending.clone();
                continue;
            }
            if matches!(job.guard, Guard::Release) {
                job.waiting = self.handoff.clone();
            }
            self.running = Some(job);
        }
        std::mem::take(&mut self.events)
    }
    pub fn finish_action(&mut self, action: ActionId, mut result: Result<Value, Value>) {
        if self.observers.remove(&action) {
            self.complete(action, result);
            return;
        }
        if !self
            .running
            .as_ref()
            .is_some_and(|job| job.id == action && job.started)
        {
            return;
        }
        let job = self.running.take().unwrap();
        if job.discard_result && result.is_ok() {
            result = Ok(Value::Null);
        }
        self.complete(action, result);
        self.barrier = self.pending.clone();
    }
}
