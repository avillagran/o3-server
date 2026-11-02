//! Dispatch for the long-tail (P2) surfaces: pull requests through `gh`, provider setup, keybindings,
//! agent-session import, preview/device registries and small admin reads. `ext.rs` and `streams.rs`
//! carry one hook each. Nothing here runs while idle: state is created on first use and streams are
//! polled only by sockets that opened them.
use crate::store::State;
use serde_json::{json, Value};
use std::sync::Arc;
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

pub type Reply = Result<Value, Value>;

pub fn invalid(message: impl Into<String>) -> Value {
    json!({"_tag":"RpcInvalidPayload","message":message.into()})
}
pub fn not_implemented(message: impl Into<String>) -> Value {
    json!({"_tag":"RpcNotImplemented","message":message.into()})
}

/// Mutable state of the surfaces below. Every field is empty until a client uses the feature.
#[derive(Default)]
pub struct Runtime {
    pub clones: crate::clone_jobs::Registry,
    pub worktrees: std::sync::Mutex<crate::worktree_progress::Registry>,
    pub device: crate::device::Runtime,
    pub setup: crate::setup::Registry,
    pub preview: crate::preview::Manager,
}

pub fn handles(tag: &str) -> bool {
    crate::pullreq::handles(tag)
        || crate::setup::handles(tag)
        || crate::keybind::handles(tag)
        || crate::sessions::handles(tag)
        || crate::preview::handles(tag)
        || crate::misc::handles(tag)
}

pub async fn call(s: &Arc<State>, tag: &str, p: Value) -> Reply {
    if crate::pullreq::handles(tag) {
        crate::pullreq::call(s, tag, &p).await
    } else if crate::keybind::handles(tag) {
        crate::keybind::call(s, tag, &p)
    } else if crate::sessions::handles(tag) {
        crate::sessions::call(s, tag, &p).await
    } else if crate::preview::handles(tag) {
        crate::preview::call(s, tag, &p)
    } else if crate::misc::handles(tag) {
        crate::misc::call(s, tag, &p).await
    } else {
        crate::setup::call(s, tag, &p).await
    }
}

// ---------------------------------------------------------------------------------------------
// Streams

/// Live subscriptions of this module. All are bounded: one chunk outstanding per subscriber.
pub enum Stream {
    Device(crate::device::Cursor),
    Setup(crate::setup::Cursor),
    Preview(crate::preview::Cursor),
    Servers(crate::preview::Servers),
    Once(Once),
    Worktree(crate::worktree_progress::Cursor),
    Clones(crate::clone_jobs::Cursor),
}

/// A stream that sends one value and then stays open without doing any work.
pub struct Once {
    value: Option<Value>,
    waiting: bool,
}

pub fn is_stream(tag: &str) -> bool {
    matches!(
        tag,
        "provider.auth.subscribe"
            | "provider.install.subscribe"
            | "subscribePreviewEvents"
            | "subscribeDiscoveredLocalServers"
            | "subscribeWorktreeSetup"
            | "subscribeProjectClones"
            | "subscribeDeviceState"
    )
}

pub fn open(s: &Arc<State>, tag: &str, p: &Value) -> Result<Stream, Value> {
    Ok(match tag {
        "subscribePreviewEvents" => Stream::Preview(crate::preview::open_events(s)),
        "subscribeProjectClones" => Stream::Clones(Default::default()),
        "subscribeWorktreeSetup" => Stream::Worktree(crate::worktree_progress::Cursor::new(
            str_field(p, "threadId")?.into(),
        )),
        "subscribeDeviceState" => Stream::Device(crate::device::subscribe(s)),
        "subscribeDiscoveredLocalServers" => {
            Stream::Servers(crate::preview::Servers::new(&p["configuredUrls"]))
        }
        _ => Stream::Setup(crate::setup::open_stream(s, tag, p)?),
    })
}

impl Stream {
    pub fn next(&mut self, s: &State) -> crate::terminal::Step {
        match self {
            Stream::Clones(c) => c.next(s),
            Stream::Worktree(c) => c.next(s),
            Stream::Device(c) => c.poll(),
            Stream::Setup(c) => c.next(s),
            Stream::Preview(c) => c.next(s),
            Stream::Servers(c) => c.next(s),
            Stream::Once(o) => match o.value.take() {
                Some(v) if !o.waiting => {
                    o.waiting = true;
                    crate::terminal::Step::Values(vec![v])
                }
                _ => crate::terminal::Step::Idle,
            },
        }
    }
    pub fn ack(&mut self) {
        match self {
            Stream::Clones(c) => c.ack(),
            Stream::Worktree(c) => c.ack(),
            Stream::Device(c) => c.ack(),
            Stream::Setup(c) => c.ack(),
            Stream::Preview(c) => c.ack(),
            Stream::Servers(c) => c.ack(),
            Stream::Once(o) => o.waiting = false,
        }
    }
    pub fn is_full(&self) -> bool {
        match self {
            Stream::Clones(c) => c.is_full(),
            Stream::Worktree(c) => c.is_full(),
            Stream::Device(c) => c.is_full(),
            Stream::Setup(c) => c.is_full(),
            Stream::Preview(c) => c.is_full(),
            Stream::Servers(c) => c.is_full(),
            Stream::Once(o) => o.waiting,
        }
    }
}

// Small bounded helpers shared by the modules.

/// A time-limited cache with a fixed capacity.
pub struct Ttl<V> {
    ttl: Duration,
    cap: usize,
    map: HashMap<String, (Instant, V)>,
    order: VecDeque<String>,
}
impl<V: Clone> Ttl<V> {
    pub fn new(ttl: Duration, cap: usize) -> Self {
        Self {
            ttl,
            cap,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub fn get(&mut self, key: &str) -> Option<V> {
        match self.map.get(key) {
            Some((at, v)) if at.elapsed() < self.ttl => Some(v.clone()),
            Some(_) => {
                self.map.remove(key);
                self.order.retain(|k| k != key);
                None
            }
            None => None,
        }
    }
    pub fn put(&mut self, key: String, v: V) {
        if self.map.insert(key.clone(), (Instant::now(), v)).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > self.cap {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
    }
}

pub fn str_field<'a>(p: &'a Value, key: &str) -> Result<&'a str, Value> {
    p[key]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| invalid(format!("{key} is required")))
}
