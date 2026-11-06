//! Nightly 4247 fresh-session boundaries. History stays readable but cannot be replayed across a reset.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, String>;

/// Queued or cancelled-before-start resets do not discard the previous context.
pub(crate) fn boundary(db: &Connection, tid: &str, through: i64) -> Result<i64> {
    db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),0) FROM records WHERE kind='runs' AND thread_id=?1 AND json_extract(body,'$.ordinal')<=?2 AND json_extract(body,'$.freshContext')=1 AND json_extract(body,'$.status')!='queued' AND NOT(json_extract(body,'$.status')='cancelled' AND json_extract(body,'$.startedAt') IS NULL AND json_extract(body,'$.restartContinuationOfRunId') IS NULL)",params![tid,through],|r|r.get(0)).map_err(|e|e.to_string())
}

pub(crate) fn validate(command: &serde_json::Value) -> Result<()> {
    if command.get("freshContext").is_some_and(|v| !v.is_boolean()) {
        return Err("freshContext must be a boolean".into());
    }
    Ok(())
}

const BINDING_FIELDS: &[&str] = &[
    "providerGeneration",
    "activeProviderThreadId",
    "codexNativeThreadId",
    "nativeForkSource",
    "nativeForkLastTurnId",
    "nativeForkTransferId",
    "providerHandoff",
    "providerHandoffAttachments",
    "inheritedProviderContext",
    "inheritedProviderAttachments",
];
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS fresh_context_resets(run_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,previous_binding TEXT NOT NULL)").map_err(|e|e.to_string())
}
pub(crate) fn remember(db: &Connection, thread: &Value, run: &Value) -> Result<()> {
    let mut previous = json!({});
    for field in BINDING_FIELDS {
        previous[field] = thread[field].clone();
    }
    db.execute(
        "INSERT OR IGNORE INTO fresh_context_resets VALUES(?,?,?)",
        params![
            run["id"].as_str(),
            thread["id"].as_str(),
            previous.to_string()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
/// A reset cancelled before execution must not turn the next ordinary send into a reset.
pub(crate) fn restore_cancelled(db: &Connection, thread: &mut Value) -> Result<()> {
    let previous:Option<String>=db.query_row("SELECT f.previous_binding FROM fresh_context_resets f JOIN records r ON r.kind='runs' AND r.id=f.run_id WHERE f.thread_id=?1 AND f.run_id=?2 AND json_extract(r.body,'$.status')='cancelled' AND json_extract(r.body,'$.startedAt') IS NULL AND json_extract(r.body,'$.restartContinuationOfRunId') IS NULL",params![thread["id"].as_str(),thread["providerGeneration"].as_str()],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if let Some(previous) = previous {
        let previous: Value = serde_json::from_str(&previous).map_err(|e| e.to_string())?;
        for field in BINDING_FIELDS {
            thread[field] = previous[field].clone();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{self, get, put},
        provider::{Provider, ProviderControl, ProviderEvent, RunInput},
        store::State,
    };
    use serde_json::{json, Value};
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::sync::{mpsc, watch, Notify};
    #[derive(Default)]
    struct Probe {
        log: Mutex<Vec<Value>>,
        release: Notify,
        fail_detach: std::sync::atomic::AtomicBool,
    }
    impl Provider for Probe {
        fn capabilities(&self, _: &str) -> Option<Value> {
            Some(serde_json::from_str(include_str!("claude-capabilities.json")).unwrap())
        }
        fn config(&self) -> Vec<Value> {
            ["a","b"].into_iter().map(|id|json!({"instanceId":id,"driver":"claudeAgent","enabled":true,"installed":true,"status":"ready","models":[{"id":"test"}]})).collect()
        }
        fn execute<'a>(
            &'a self,
            input: RunInput,
            out: mpsc::Sender<ProviderEvent>,
            _control: mpsc::Receiver<ProviderControl>,
            _cancel: watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.log.lock().unwrap().push(json!({"kind":"execute","native":input.native_session_id,"key":input.thread_id,"selection":input.model_selection,"text":input.text}));
                let native = input
                    .native_session_id
                    .unwrap_or_else(|| format!("native-{}", input.run_id));
                out.send(ProviderEvent::Started)
                    .await
                    .map_err(|e| e.to_string())?;
                out.send(ProviderEvent::Native(
                    json!({"type":"session","session_id":native}),
                ))
                .await
                .map_err(|e| e.to_string())?;
                if input.text == "hold" {
                    self.release.notified().await;
                }
                out.send(ProviderEvent::TextDelta(format!("answer-{}", input.text)))
                    .await
                    .map_err(|e| e.to_string())?;
                out.send(ProviderEvent::Completed)
                    .await
                    .map_err(|e| e.to_string())
            })
        }
        fn detach<'a>(
            &'a self,
            _instance: &'a str,
            key: &'a str,
            native: Option<&'a str>,
            _revoke: bool,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(json!({"kind":"detach","key":key,"native":native}));
                if self.fail_detach.load(std::sync::atomic::Ordering::SeqCst) {
                    Err("test detach failure".into())
                } else {
                    Ok(())
                }
            })
        }
    }
    fn state() -> (Arc<State>, Arc<Probe>) {
        let dir = std::env::temp_dir().join(format!(
            "o3-fresh-context-{}",
            crate::store::random_id().unwrap()
        ));
        let state = Arc::new(State::open(&dir).unwrap());
        std::fs::write(state.dir.join("settings.json"),json!({"providerInstances":{"a":{"driver":"claudeAgent","enabled":true},"b":{"driver":"claudeAgent","enabled":true}}}).to_string()).unwrap();
        let probe = Arc::new(Probe::default());
        *state.provider.lock().unwrap() = Some(probe.clone());
        events::dispatch(&state,&json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Test","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"a","model":"test"}})).unwrap();
        (state, probe)
    }
    fn command(id: &str, text: &str, fresh: bool) -> Value {
        json!({"type":"message.dispatch","commandId":id,"threadId":"t","messageId":id,"text":text,"attachments":[],"freshContext":fresh,"dispatchMode":{"type":"queue_after_active"}})
    }
    async fn settled(s: &State, n: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let p = events::projection(s, "t").unwrap();
                let runs = p["runs"].as_array().unwrap();
                assert!(!runs.iter().any(|r| r["status"] == "failed"), "{p}");
                if runs.len() == n
                    && runs.iter().all(|r| r["status"] == "completed")
                    && s.active.lock().unwrap().is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn reset_detaches_before_execution_and_future_switch_cannot_replay_old_history() {
        let (s, p) = state();
        events::dispatch(&s, &command("seed", "OLD-CONTEXT-TOKEN", false)).unwrap();
        settled(&s, 1).await;
        let old = events::projection(&s, "t").unwrap()["runs"][0]["providerThreadId"].clone();
        let reset = command("reset", "NEW-CONTEXT-TOKEN", true);
        events::dispatch(&s, &reset).unwrap();
        settled(&s, 2).await;
        assert_eq!(
            p.log
                .lock()
                .unwrap()
                .iter()
                .filter(|x| x["kind"] == "detach")
                .count(),
            1
        );
        events::dispatch(&s, &reset).unwrap();
        let projection = events::projection(&s, "t").unwrap();
        assert_ne!(projection["runs"][1]["providerThreadId"], old);
        assert_eq!(projection["runs"][1]["freshContext"], true);
        assert!(projection["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "OLD-CONTEXT-TOKEN"));
        assert!(projection["turnItems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["type"] == "user_message"
                && i["messageId"] == "reset"
                && i["freshContext"] == true));
        {
            let log = p.log.lock().unwrap();
            assert_eq!(log[1]["kind"], "detach");
            assert_eq!(log[2]["kind"], "execute");
            assert!(log[2]["native"].is_null());
            assert!(!log[2].to_string().contains("OLD-CONTEXT-TOKEN"));
        }
        let mut next = command("switch", "after-switch", false);
        next["modelSelection"] = json!({"instanceId":"b","model":"test"});
        events::dispatch(&s, &next).unwrap();
        settled(&s, 3).await;
        let mut back = command("back", "after-return", false);
        back["modelSelection"] = json!({"instanceId":"a","model":"test"});
        events::dispatch(&s, &back).unwrap();
        settled(&s, 4).await;
        let log = p.log.lock().unwrap();
        for call in log.iter().filter(|c| {
            c["kind"] == "execute"
                && matches!(c["text"].as_str(), Some("after-switch" | "after-return"))
        }) {
            assert!(!call.to_string().contains("OLD-CONTEXT-TOKEN"), "{call}");
        }
        assert_ne!(
            events::projection(&s, "t").unwrap()["runs"][3]["providerThreadId"],
            old
        );
    }
    #[tokio::test]
    async fn queued_reset_does_not_detach_the_running_turn() {
        let (s, p) = state();
        events::dispatch(&s, &command("hold", "hold", false)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while p.log.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut clear = command("reset", "fresh", true);
        clear["deliveryIntent"] = json!("steer");
        clear["modelSelection"] = json!({"instanceId":"b","model":"test"});
        events::dispatch(&s, &clear).unwrap();
        {
            let db = s.db.lock().unwrap();
            assert_eq!(boundary(&db, "t", i64::MAX).unwrap(), 0);
        }
        assert_eq!(p.log.lock().unwrap().len(), 1);
        p.release.notify_one();
        settled(&s, 2).await;
        let log = p.log.lock().unwrap();
        assert_eq!(log[1]["kind"], "detach");
        assert_eq!(log[2]["kind"], "execute");
        assert!(log[2]["native"].is_null());
    }
    #[test]
    fn boundaries_ignore_queued_and_cancelled_unstarted_resets() {
        let (s, _) = state();
        let db = s.db.lock().unwrap();
        for (id, status, started, ordinal) in [
            ("old", "completed", Some("now"), 1),
            ("queued", "queued", None, 2),
            ("cancelled", "cancelled", None, 3),
        ] {
            put(&db,"runs",id,"t",&json!({"id":id,"threadId":"t","ordinal":ordinal,"freshContext":true,"status":status,"startedAt":started})).unwrap();
        }
        assert_eq!(boundary(&db, "t", i64::MAX).unwrap(), 1);
        assert_eq!(boundary(&db, "t", 0).unwrap(), 0);
        put(&db,"runs","continued","t",&json!({"id":"continued","threadId":"t","ordinal":4,"freshContext":true,"status":"cancelled","startedAt":null,"restartContinuationOfRunId":"source"})).unwrap();
        assert_eq!(boundary(&db, "t", 4).unwrap(), 4);
        assert!(validate(&json!({"freshContext":"true"})).is_err());
        drop(db);
        let mut invalid = command("invalid-intent", "fresh", true);
        invalid["deliveryIntent"] = json!("invalid");
        assert!(events::dispatch(&s, &invalid)
            .unwrap_err()
            .contains("invalid delivery intent"));
    }
    #[tokio::test]
    async fn reset_rejects_background_work_and_pending_transfers_atomically() {
        let (s, p) = state();
        events::dispatch(&s, &command("seed", "old", false)).unwrap();
        settled(&s, 1).await;
        let before = events::projection(&s, "t").unwrap();
        let pt = before["thread"]["activeProviderThreadId"].as_str().unwrap();
        {
            let db = s.db.lock().unwrap();
            let mut record = get(&db, "providerThreads", pt).unwrap();
            record["pendingBackgroundTasks"] =
                json!([{"kind":"command","taskId":"bg","awaited":true}]);
            put(&db, "providerThreads", pt, "t", &record).unwrap();
        }
        assert!(events::dispatch(&s, &command("blocked", "new", true))
            .unwrap_err()
            .contains("background"));
        assert_eq!(events::projection(&s, "t").unwrap()["runs"], before["runs"]);
        assert_eq!(p.log.lock().unwrap().len(), 1);
        {
            let db = s.db.lock().unwrap();
            let mut record = get(&db, "providerThreads", pt).unwrap();
            record["pendingBackgroundTasks"] = json!([]);
            put(&db, "providerThreads", pt, "t", &record).unwrap();
            put(
                &db,
                "contextTransfers",
                "pending",
                "t",
                &json!({"id":"pending","type":"fork","targetThreadId":"t","status":"pending"}),
            )
            .unwrap();
        }
        assert!(events::dispatch(&s, &command("fork-blocked", "new", true))
            .unwrap_err()
            .contains("fork or merge"));
        assert_eq!(events::projection(&s, "t").unwrap()["runs"], before["runs"]);
        assert_eq!(p.log.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn cancelled_deferred_reset_preserves_the_previous_session_for_the_next_send() {
        let (s, p) = state();
        events::dispatch(&s, &command("seed", "old", false)).unwrap();
        settled(&s, 1).await;
        let before = events::projection(&s, "t").unwrap();
        let old = before["runs"][0]["providerThreadId"].clone();
        let mut reset = command("cancel", "unused", true);
        reset["dispatchMode"] = json!({"type":"defer_start"});
        events::dispatch(&s, &reset).unwrap();
        {
            let db = s.db.lock().unwrap();
            let id = events::projection_db(&db, "t").unwrap()["runs"][1]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let mut r = get(&db, "runs", &id).unwrap();
            r["status"] = json!("cancelled");
            put(&db, "runs", &id, "t", &r).unwrap();
        }
        let dir = s.dir.clone();
        drop(s);
        let s = Arc::new(State::open(&dir).unwrap());
        *s.provider.lock().unwrap() = Some(p.clone());
        events::dispatch(&s, &command("next", "keep", false)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v = events::projection(&s, "t").unwrap();
                if v["runs"][2]["status"] == "completed" {
                    break;
                }
                assert_ne!(v["runs"][2]["status"], "failed", "{v}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let after = events::projection(&s, "t").unwrap();
        assert_eq!(after["runs"][2]["providerThreadId"], old);
        let log = p.log.lock().unwrap();
        let last = log.iter().rev().find(|x| x["kind"] == "execute").unwrap();
        assert!(last["native"]
            .as_str()
            .is_some_and(|id| id.starts_with("native-")));
    }
    #[tokio::test]
    async fn detach_failure_blocks_new_execution_and_the_next_send_retries_release() {
        let (s, p) = state();
        events::dispatch(&s, &command("seed", "old", false)).unwrap();
        settled(&s, 1).await;
        p.fail_detach
            .store(true, std::sync::atomic::Ordering::SeqCst);
        events::dispatch(&s, &command("reset", "new", true)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if events::projection(&s, "t").unwrap()["runs"][1]["status"] == "failed"
                    && s.active.lock().unwrap().is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            p.log
                .lock()
                .unwrap()
                .iter()
                .filter(|x| x["kind"] == "execute")
                .count(),
            1
        );
        p.fail_detach
            .store(false, std::sync::atomic::Ordering::SeqCst);
        events::dispatch(&s, &command("retry", "newer", false)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v = events::projection(&s, "t").unwrap();
                if v["runs"][2]["status"] == "completed" {
                    break;
                }
                assert_ne!(v["runs"][2]["status"], "failed", "{v}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let log = p.log.lock().unwrap();
        let last = log.iter().rev().find(|x| x["kind"] == "execute").unwrap();
        assert!(last["native"].is_null());
        assert!(!last.to_string().contains("answer-old"));
    }
    #[tokio::test]
    async fn history_readers_and_native_recovery_do_not_cross_a_fresh_boundary() {
        let (s, _) = state();
        events::dispatch(&s, &command("seed", "OLD-SECRET-TOKEN", false)).unwrap();
        settled(&s, 1).await;
        let mut clear = command("reset", "New request", true);
        clear["dispatchMode"] = json!({"type":"defer_start"});
        events::dispatch(&s, &clear).unwrap();
        let projection = events::projection(&s, "t").unwrap();
        let run = &projection["runs"][1];
        {
            let db = s.db.lock().unwrap();
            let history = crate::thread_commands::handoff(&db, "t", 2).unwrap();
            assert!(!history.contains("OLD-SECRET-TOKEN"));
            assert!(history.contains("New request"));
            let items = crate::context_transfer::fork_items(&db, "t", 2).unwrap();
            assert!(!json!(items).to_string().contains("OLD-SECRET-TOKEN"));
        }
        let recovery = crate::native_recovery::prepare(
            &s,
            "t",
            run["id"].as_str().unwrap(),
            run["activeAttemptId"].as_str(),
            None,
            crate::provider::NativeRecoveryReason::ResumeFailed,
            "New request".into(),
        )
        .unwrap();
        assert_eq!(recovery["handoffs"], json!([]));
        assert!(
            !recovery.to_string().contains("OLD-SECRET-TOKEN"),
            "{recovery}"
        );
        assert!(events::projection(&s, "t").unwrap()["messages"]
            .to_string()
            .contains("OLD-SECRET-TOKEN"));
    }
    #[tokio::test]
    async fn imported_native_binding_is_released_even_without_a_provider_row() {
        let (s, p) = state();
        {
            let db = s.db.lock().unwrap();
            let mut t = get(&db, "thread", "t").unwrap();
            t["activeProviderThreadId"] = json!("imported-native");
            t["providerHandoff"] = json!("OLD-IMPORTED-CONTEXT");
            put(&db, "thread", "t", "t", &t).unwrap();
        }
        let mut reset = command("reset", "fresh target", true);
        reset["modelSelection"] = json!({"instanceId":"b","model":"test"});
        events::dispatch(&s, &reset).unwrap();
        settled(&s, 1).await;
        let log = p.log.lock().unwrap();
        assert_eq!(log[0]["kind"], "detach");
        assert_eq!(log[0]["native"], "imported-native");
        assert_eq!(log[1]["kind"], "execute");
        assert!(log[1]["native"].is_null());
        assert_eq!(log[1]["selection"]["instanceId"], "b");
        assert!(!log[1].to_string().contains("OLD-IMPORTED-CONTEXT"));
    }
}
