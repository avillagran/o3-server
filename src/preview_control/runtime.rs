//! Owned scheduler for native browser-control callbacks; caller cancellation only drops observation.
use super::{ActionId, CallbackId, DependencyId, Event, Kernel};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{watch, Notify};
type ResultValue = Result<Value, Value>;
type Work = Box<dyn FnOnce(Value) -> Pin<Box<dyn Future<Output = ResultValue> + Send>> + Send>;
type Tracked = Pin<Box<dyn Future<Output = ()> + Send>>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub agent_id: Option<String>,
    pub controller: Option<String>,
    pub generation: u64,
    pub closed: bool,
}
#[derive(Clone)]
pub struct Call {
    action: ActionId,
    result: watch::Receiver<Option<ResultValue>>,
}
impl Call {
    pub fn action_id(&self) -> ActionId {
        self.action
    }
    pub async fn wait(mut self) -> ResultValue {
        loop {
            if let Some(result) = self.result.borrow().clone() {
                return result;
            }
            if self.result.changed().await.is_err() {
                return Err(ended());
            }
        }
    }
}
fn ended() -> Value {
    json!({"_tag":"BrowserControlInterrupted","name":"BrowserControlInterrupted","message":"Browser control worker ended.","reason":"closed"})
}
struct State {
    kernel: Kernel,
    callbacks: BTreeMap<CallbackId, Work>,
    discarded: Vec<Work>,
    action_callbacks: BTreeMap<ActionId, CallbackId>,
    results: BTreeMap<ActionId, watch::Sender<Option<ResultValue>>>,
    sequence: u64,
    dependencies: Vec<(DependencyId, Tracked)>,
    snapshot: watch::Sender<Snapshot>,
    closed: bool,
    shutdown: Option<ActionId>,
    shutdown_complete: bool,
    finished: bool,
}
impl State {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            agent_id: self.kernel.agent_id().map(str::to_owned),
            controller: self.kernel.controller().map(str::to_owned),
            generation: self.kernel.generation(),
            closed: self.closed,
        }
    }
    fn publish(&self) {
        let snapshot = self.snapshot();
        self.snapshot.send_if_modified(|current| {
            if *current == snapshot {
                false
            } else {
                *current = snapshot;
                true
            }
        });
    }
    fn sequence(&mut self) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }
    fn call(&mut self, action: ActionId, callback: Option<CallbackId>) -> Call {
        if let Some(callback) = callback {
            if let Some(previous) = self.action_callbacks.insert(action, callback) {
                if let Some(work) = self.callbacks.remove(&previous) {
                    self.discarded.push(work);
                }
            }
        }
        let sender = self
            .results
            .entry(action)
            .or_insert_with(|| watch::channel(None).0);
        Call {
            action,
            result: sender.subscribe(),
        }
    }
    fn shutdown(&mut self) {
        if self.shutdown.is_none() {
            self.closed = true;
            self.shutdown = Some(self.kernel.close());
            self.publish();
        }
    }
}
struct Shared {
    state: Mutex<State>,
    wake: Notify,
    clock: tokio::time::Instant,
}
struct Owner {
    shared: Arc<Shared>,
    done: watch::Receiver<bool>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown();
        self.shared.wake.notify_one();
    }
}
#[derive(Clone)]
pub struct Control(Arc<Owner>);
enum Submission<'a> {
    Agent(&'a str, Option<tokio::time::Instant>),
    Human(&'a str),
    Latest(&'a str, &'a str),
    Observe,
    System,
}
impl Control {
    pub fn new(agent_id: Option<String>) -> Self {
        let initial = Snapshot {
            agent_id: agent_id.clone(),
            controller: None,
            generation: 0,
            closed: false,
        };
        let (snapshot, _) = watch::channel(initial);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                kernel: Kernel::new(agent_id),
                callbacks: BTreeMap::new(),
                discarded: Vec::new(),
                action_callbacks: BTreeMap::new(),
                results: BTreeMap::new(),
                sequence: 0,
                dependencies: Vec::new(),
                snapshot,
                closed: false,
                shutdown: None,
                shutdown_complete: false,
                finished: false,
            }),
            wake: Notify::new(),
            clock: tokio::time::Instant::now(),
        });
        let (done_tx, done) = watch::channel(false);
        tokio::spawn(run(shared.clone(), done_tx));
        Self(Arc::new(Owner { shared, done }))
    }
    pub fn is_finished(&self) -> bool {
        self.0.shared.state.lock().unwrap().finished
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0.shared.state.lock().unwrap().snapshot()
    }
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.0.shared.state.lock().unwrap().snapshot.subscribe()
    }
    fn submit(
        &self,
        submission: Submission<'_>,
        input: Value,
        work: Work,
        merge: impl FnOnce(&Value, Value) -> Value,
    ) -> Call {
        let mut state = self.0.shared.state.lock().unwrap();
        if state.finished {
            let (sender, result) = watch::channel(Some(Err(ended())));
            drop(sender);
            return Call { action: 0, result };
        }
        let callback = state.sequence();
        state.callbacks.insert(callback, work);
        let action = match submission {
            Submission::Agent(caller, deadline) => state.kernel.agent(
                caller,
                callback,
                input,
                deadline.map(|at| {
                    at.saturating_duration_since(self.0.shared.clock)
                        .as_secs_f64()
                        * 1000.0
                }),
            ),
            Submission::Human(viewer) => state.kernel.human(viewer, callback, input),
            Submission::Latest(viewer, key) => state
                .kernel
                .human_latest(viewer, key, callback, input, merge),
            Submission::Observe => state.kernel.observe(callback, input),
            Submission::System => state.kernel.system(callback, input),
        };
        let call = state.call(action, Some(callback));
        state.publish();
        drop(state);
        self.0.shared.wake.notify_one();
        call
    }
    pub fn agent<F>(
        &self,
        caller: &str,
        deadline: Option<tokio::time::Instant>,
        work: impl FnOnce() -> F + Send + 'static,
    ) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.submit(
            Submission::Agent(caller, deadline),
            Value::Null,
            Box::new(move |_| Box::pin(work())),
            |_, next| next,
        )
    }
    pub fn human<F>(&self, viewer: &str, work: impl FnOnce() -> F + Send + 'static) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.submit(
            Submission::Human(viewer),
            Value::Null,
            Box::new(move |_| Box::pin(work())),
            |_, next| next,
        )
    }
    pub fn human_latest<F>(
        &self,
        viewer: &str,
        key: &str,
        input: Value,
        work: impl FnOnce(Value) -> F + Send + 'static,
        merge: impl FnOnce(&Value, Value) -> Value,
    ) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.submit(
            Submission::Latest(viewer, key),
            input,
            Box::new(move |input| Box::pin(work(input))),
            merge,
        )
    }
    pub fn observe<F>(&self, work: impl FnOnce() -> F + Send + 'static) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.submit(
            Submission::Observe,
            Value::Null,
            Box::new(move |_| Box::pin(work())),
            |_, next| next,
        )
    }
    pub fn system<F>(&self, work: impl FnOnce() -> F + Send + 'static) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.submit(
            Submission::System,
            Value::Null,
            Box::new(move |_| Box::pin(work())),
            |_, next| next,
        )
    }
    fn transition(&self, operation: impl FnOnce(&mut State) -> ActionId) -> Call {
        let mut state = self.0.shared.state.lock().unwrap();
        if state.finished {
            let (sender, result) = watch::channel(Some(Err(ended())));
            drop(sender);
            return Call { action: 0, result };
        }
        let action = operation(&mut state);
        let call = state.call(action, None);
        state.publish();
        drop(state);
        self.0.shared.wake.notify_one();
        call
    }
    pub fn take(&self, viewer: &str) -> Call {
        self.transition(|state| state.kernel.take(viewer))
    }
    pub fn release<F>(&self, viewer: &str, after_drain: impl FnOnce() -> F + Send + 'static) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.transition(|state| {
            let callback = state.sequence();
            state
                .callbacks
                .insert(callback, Box::new(move |_| Box::pin(after_drain())));
            let action = state.kernel.release(viewer, Some(callback));
            state.action_callbacks.insert(action, callback);
            action
        })
    }
    pub fn disconnect<F>(
        &self,
        viewer: &str,
        after_drain: impl FnOnce() -> F + Send + 'static,
    ) -> Call
    where
        F: Future<Output = ResultValue> + Send + 'static,
    {
        self.transition(|state| {
            let callback = state.sequence();
            state
                .callbacks
                .insert(callback, Box::new(move |_| Box::pin(after_drain())));
            let action = state.kernel.disconnect(viewer, Some(callback));
            state.action_callbacks.insert(action, callback);
            action
        })
    }
    pub fn close(&self) -> Call {
        self.transition(|state| {
            state.closed = true;
            state.kernel.close()
        })
    }
    fn tracked(&self, work: Tracked, handoff: Option<bool>) {
        let mut state = self.0.shared.state.lock().unwrap();
        if state.finished {
            // The discarded future may own the final Control and re-enter this mutex.
            drop(state);
            drop(work);
            return;
        }
        let dependency = state.sequence();
        if let Some(handoff) = handoff {
            if handoff {
                state.kernel.track_until_handoff(dependency);
            } else {
                state.kernel.track(dependency);
            }
        }
        state.dependencies.push((dependency, work));
        drop(state);
        self.0.shared.wake.notify_one();
    }
    pub fn track(&self, work: impl Future<Output = ()> + Send + 'static) {
        self.tracked(Box::pin(work), Some(false));
    }
    pub fn track_until_handoff(&self, work: impl Future<Output = ()> + Send + 'static) {
        self.tracked(Box::pin(work), Some(true));
    }
    pub(crate) fn own_finalizer(&self, work: impl Future<Output = ()> + Send + 'static) {
        self.tracked(Box::pin(work), None);
    }
    /// Stop admission and drain all owned work; the page owner must settle its I/O first.
    pub async fn shutdown(&self) -> Result<(), Value> {
        self.0.shared.state.lock().unwrap().shutdown();
        self.0.shared.wake.notify_one();
        let mut done = self.0.done.clone();
        while !*done.borrow() {
            if done.changed().await.is_err() {
                return Err(ended());
            }
        }
        Ok(())
    }
}
enum Finished {
    Action(ActionId, ResultValue),
    Dependency(DependencyId),
}
#[derive(Clone, Copy)]
enum Receipt {
    Action(ActionId),
    Dependency(DependencyId),
}
async fn run(shared: Arc<Shared>, done: watch::Sender<bool>) {
    let mut jobs = tokio::task::JoinSet::new();
    let mut receipts = BTreeMap::new();
    loop {
        let (work, dependencies, discarded, shutdown_complete) = {
            let mut state = shared.state.lock().unwrap();
            let events = state
                .kernel
                .drain_with_clock(|| shared.clock.elapsed().as_secs_f64() * 1000.0);
            let mut work = Vec::new();
            for event in events {
                match event {
                    Event::Generation { .. } => {}
                    Event::Run {
                        action,
                        callback,
                        input,
                    } => {
                        if let Some(callback) = state.callbacks.remove(&callback) {
                            work.push((action, callback, input));
                        } else {
                            state.kernel.finish_action(action, Err(ended()));
                            shared.wake.notify_one();
                        }
                    }
                    Event::Complete { action, result } => {
                        if let Some(callback) = state.action_callbacks.remove(&action) {
                            if let Some(work) = state.callbacks.remove(&callback) {
                                state.discarded.push(work);
                            }
                        }
                        if let Some(sender) = state.results.remove(&action) {
                            sender.send_replace(Some(result));
                        }
                        if state.shutdown == Some(action) {
                            state.shutdown_complete = true;
                        }
                    }
                }
            }
            state.publish();
            (
                work,
                std::mem::take(&mut state.dependencies),
                std::mem::take(&mut state.discarded),
                state.shutdown_complete,
            )
        };
        // Destructors may release the final owner and re-enter this driver's state.
        drop(discarded);
        for (action, callback, input) in work {
            let handle = jobs.spawn(async move { Finished::Action(action, callback(input).await) });
            receipts.insert(handle.id(), Receipt::Action(action));
        }
        for (dependency, work) in dependencies {
            let handle = jobs.spawn(async move {
                work.await;
                Finished::Dependency(dependency)
            });
            receipts.insert(handle.id(), Receipt::Dependency(dependency));
        }
        if shutdown_complete && jobs.is_empty() {
            let mut state = shared.state.lock().unwrap();
            if state.dependencies.is_empty()
                && state.results.is_empty()
                && state.callbacks.is_empty()
            {
                state.finished = true;
                break;
            }
            continue;
        }
        tokio::select! {
            _=shared.wake.notified()=>{},
            result=jobs.join_next_with_id(),if !jobs.is_empty()=>{
                let mut state=shared.state.lock().unwrap();
                match result {
                    Some(Ok((id,Finished::Action(action,result))))=>{receipts.remove(&id);state.kernel.finish_action(action,result);},
                    Some(Ok((id,Finished::Dependency(dependency))))=>{receipts.remove(&id);state.kernel.settle_tracked(dependency);},
                    Some(Err(error))=>{
                        match receipts.remove(&error.id()) {
                            Some(Receipt::Action(action))=>state.kernel.finish_action(action,Err(ended())),
                            Some(Receipt::Dependency(dependency))=>state.kernel.settle_tracked(dependency),
                            None=>{},
                        }
                    },
                    None=>{},
                }
            }
        }
    }
    done.send_replace(true);
}
