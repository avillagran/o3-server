//! Token-scoped foreground activity for preview focus ownership.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

#[derive(Clone, Default)]
pub struct Activity(Arc<Mutex<Ledger>>);
#[derive(Default)]
struct Ledger {
    next: u64,
    threads: HashMap<String, HashSet<u64>>,
    subscribers: Vec<mpsc::UnboundedSender<(String, bool)>>,
}
impl Ledger {
    fn publish(&mut self, thread: &str, active: bool) {
        self.subscribers
            .retain(|tx| tx.send((thread.to_owned(), active)).is_ok());
    }
}
impl Activity {
    pub fn begin(&self, thread: &str) -> TurnGuard {
        let mut ledger = self.0.lock().unwrap();
        ledger.next = ledger
            .next
            .checked_add(1)
            .expect("activity token space exhausted");
        let token = ledger.next;
        let tokens = ledger.threads.entry(thread.to_owned()).or_default();
        let first = tokens.is_empty();
        tokens.insert(token);
        if first {
            ledger.publish(thread, true);
        }
        TurnGuard {
            activity: self.clone(),
            thread: thread.to_owned(),
            token: Some(token),
        }
    }
    pub fn active(&self, thread: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .threads
            .get(thread)
            .is_some_and(|tokens| !tokens.is_empty())
    }
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<(String, bool)> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.0.lock().unwrap().subscribers.push(tx);
        rx
    }
}
pub struct TurnGuard {
    activity: Activity,
    thread: String,
    token: Option<u64>,
}
impl TurnGuard {
    pub fn end(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        let mut ledger = self.activity.0.lock().unwrap();
        if let Some(tokens) = ledger.threads.get_mut(&self.thread) {
            if tokens.remove(&token) && tokens.is_empty() {
                ledger.threads.remove(&self.thread);
                ledger.publish(&self.thread, false);
            }
        }
    }
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_complete_native_activity_traces() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/preview-activity-4288.json"))
                .unwrap();
        for trace in fixture["traces"].as_array().unwrap() {
            let activity = Activity::default();
            let mut changes = activity.subscribe();
            let mut turns = std::collections::HashMap::new();
            for step in trace["steps"].as_array().unwrap() {
                let op = &step["operation"];
                if let Some(thread) = op["begin"].as_str() {
                    turns.insert(op["token"].as_u64().unwrap(), activity.begin(thread));
                } else if let Some(turn) = turns.get_mut(&op["end"].as_u64().unwrap()) {
                    turn.end();
                }
                let mut events = Vec::new();
                while let Ok((thread, active)) = changes.try_recv() {
                    events.push(serde_json::json!({"threadId":thread,"active":active}));
                }
                assert_eq!(serde_json::json!(events), step["events"], "{step}");
                assert_eq!(
                    serde_json::json!({"a":activity.active("a"),"b":activity.active("b")}),
                    step["active"],
                    "{step}"
                );
            }
        }
    }
    #[test]
    fn overlapping_tokens_publish_edges_and_stale_end_preserves_successor() {
        let activity = Activity::default();
        let mut events = activity.subscribe();
        let mut first = activity.begin("a");
        let second = activity.begin("a");
        assert_eq!(events.try_recv().unwrap(), ("a".into(), true));
        first.end();
        first.end();
        assert!(activity.active("a"));
        assert!(events.try_recv().is_err());
        drop(second);
        assert_eq!(events.try_recv().unwrap(), ("a".into(), false));
        let successor = activity.begin("a");
        first.end();
        assert!(activity.active("a"));
        drop(successor);
        assert_eq!(events.try_recv().unwrap(), ("a".into(), true));
        assert_eq!(events.try_recv().unwrap(), ("a".into(), false));
        assert!(!activity.active("a"));
    }
    #[test]
    fn independent_threads_and_dropped_subscriber_do_not_hold_activity() {
        let activity = Activity::default();
        drop(activity.subscribe());
        let a = activity.begin("a");
        let b = activity.begin("b");
        drop(a);
        assert!(!activity.active("a"));
        assert!(activity.active("b"));
        drop(b);
        assert!(!activity.active("b"));
    }
}
