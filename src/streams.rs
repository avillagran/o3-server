//! Subscriptions that read live in-memory sources (terminals, command output) rather than the event log.
use crate::{command_output, store::State, terminal};
use serde_json::Value;
use std::sync::Arc;

pub use terminal::Step;

/// Shared by the live sources; one watch channel wakes every websocket loop that follows them.
pub struct Hub {
    pub changed: terminal::Notifier,
    pub terminals: Arc<terminal::Manager>,
    pub commands: command_output::Hub,
}
impl Hub {
    pub fn new() -> Self {
        let changed: terminal::Notifier = Arc::new(tokio::sync::watch::channel(0).0);
        Self {
            terminals: terminal::Manager::new(changed.clone()),
            commands: command_output::Hub::new(changed.clone()),
            changed,
        }
    }
}
impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

pub enum Live {
    Terminal(terminal::Cursor),
    Command(command_output::Cursor),
}

impl Live {
    pub fn next(&mut self, s: &State) -> Step {
        match self {
            Live::Terminal(c) => c.next(s),
            Live::Command(c) => c.next(s),
        }
    }
    pub fn ack(&mut self) {
        match self {
            Live::Terminal(c) => c.ack(),
            Live::Command(c) => c.ack(),
        }
    }
    /// The subscriber may not be sent another chunk until it acknowledges one.
    pub fn is_full(&self) -> bool {
        match self {
            Live::Terminal(c) => c.is_full(),
            Live::Command(c) => c.is_full(),
        }
    }
}

pub fn is_live_tag(tag: &str) -> bool {
    matches!(
        tag,
        "terminal.attach"
            | "terminal.observe"
            | "subscribeTerminalEvents"
            | "subscribeTerminalMetadata"
            | "orchestration.subscribeCommandOutput"
    )
}

pub async fn open(s: &Arc<State>, tag: &str, p: &Value) -> Result<Live, Value> {
    if tag == "orchestration.subscribeCommandOutput" {
        command_output::open_stream(s, p).map(Live::Command)
    } else {
        terminal::open_stream(s, tag, p).await.map(Live::Terminal)
    }
}
