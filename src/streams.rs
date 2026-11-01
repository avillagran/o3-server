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
    pub ops: crate::ops::Ops,
    pub p2: crate::p2::Runtime,
}
impl Hub {
    pub fn new() -> Self {
        let changed: terminal::Notifier = Arc::new(tokio::sync::watch::channel(0).0);
        Self {
            terminals: terminal::Manager::new(changed.clone()),
            commands: command_output::Hub::new(changed.clone()),
            ops: crate::ops::Ops::new(changed.clone()),
            p2: Default::default(),
            changed,
        }
    }
}
impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

/// `server.getStorageCleanupReport`: the latest report (or null) now, then each new one.
pub struct ReportCursor {
    seen: Option<u64>,
    waiting: bool,
}

pub enum Live {
    Terminal(terminal::Cursor),
    Command(command_output::Cursor),
    Report(ReportCursor),
    P2(crate::p2::Stream),
}

impl Live {
    pub fn next(&mut self, s: &State) -> Step {
        match self {
            Live::Terminal(c) => c.next(s),
            Live::Command(c) => c.next(s),
            Live::P2(c) => c.next(s),
            Live::Report(c) => {
                if c.waiting {
                    return Step::Idle;
                }
                let (version, report) = s.streams.ops.report_version();
                if c.seen == Some(version) {
                    return Step::Idle;
                }
                c.seen = Some(version);
                c.waiting = true;
                Step::Values(vec![report.unwrap_or(Value::Null)])
            }
        }
    }
    pub fn ack(&mut self) {
        match self {
            Live::Terminal(c) => c.ack(),
            Live::Command(c) => c.ack(),
            Live::P2(c) => c.ack(),
            Live::Report(c) => c.waiting = false,
        }
    }
    /// The subscriber may not be sent another chunk until it acknowledges one.
    pub fn is_full(&self) -> bool {
        match self {
            Live::Terminal(c) => c.is_full(),
            Live::Command(c) => c.is_full(),
            Live::P2(c) => c.is_full(),
            Live::Report(c) => c.waiting,
        }
    }
}

pub fn is_live_tag(tag: &str) -> bool {
    crate::p2::is_stream(tag)
        || matches!(
            tag,
            "terminal.attach"
                | "terminal.observe"
                | "subscribeTerminalEvents"
                | "subscribeTerminalMetadata"
                | "orchestration.subscribeCommandOutput"
                | "server.getStorageCleanupReport"
        )
}

pub async fn open(s: &Arc<State>, tag: &str, p: &Value) -> Result<Live, Value> {
    if crate::p2::is_stream(tag) {
        crate::p2::open(s, tag, p).map(Live::P2)
    } else if tag == "server.getStorageCleanupReport" {
        Ok(Live::Report(ReportCursor {
            seen: None,
            waiting: false,
        }))
    } else if tag == "orchestration.subscribeCommandOutput" {
        command_output::open_stream(s, p).map(Live::Command)
    } else {
        terminal::open_stream(s, tag, p).await.map(Live::Terminal)
    }
}
