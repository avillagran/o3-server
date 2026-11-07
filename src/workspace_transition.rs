//! Workspace metadata changes detach native sessions before admitting another turn.
//! Based on nightly4151 Orchestrator/ProviderSessionManager (MIT).
use crate::{
    deletion::visit,
    events::{append, get, put},
    store::{now_ms, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS workspace_detaches(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,payload TEXT NOT NULL,completed_at INTEGER,released_at INTEGER,attempts INTEGER NOT NULL DEFAULT 0,retry_at INTEGER NOT NULL DEFAULT 0,last_error TEXT); CREATE INDEX IF NOT EXISTS workspace_detaches_thread ON workspace_detaches(thread_id) WHERE completed_at IS NULL; CREATE INDEX IF NOT EXISTS workspace_detaches_pending ON workspace_detaches(retry_at) WHERE released_at IS NULL;").map_err(|e| e.to_string())
}
pub(crate) fn pending(db: &Connection, tid: &str) -> Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM workspace_detaches WHERE thread_id=? AND completed_at IS NULL)", [tid], |r|r.get(0)).map_err(|e|e.to_string())
}

/// Checks and projections share the command/receipt transaction, including explicit null.
pub(crate) fn plan(db: &Connection, t: &Value, c: &Value, at: &str) -> Result<bool> {
    let tid = t["id"].as_str().ok_or("thread id missing")?;
    for (expected, field) in [
        ("expectedWorktreePath", "worktreePath"),
        ("expectedBranch", "branch"),
    ] {
        if c.get(expected).is_some_and(|v| v != &t[field]) {
            return Err(format!(
                "Thread {tid} {field} changed before the metadata update could be applied."
            ));
        }
    }
    if c["expectedEmpty"] == true {
        let occupied: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=? AND kind IN ('messages','runs'))", [tid], |r|r.get(0)).map_err(|e|e.to_string())?;
        if occupied {
            return Err(format!("Thread {tid} is no longer empty."));
        }
    }
    let Some(path) = c.get("worktreePath") else {
        return Ok(false);
    };
    if !path.is_null() && !path.as_str().is_some_and(|s| !s.trim().is_empty()) {
        return Err("worktreePath must be a nonempty string or null".into());
    }
    if *path == t["worktreePath"] {
        return Ok(false);
    }
    let payload = json!({"threadKey":format!("{}:{}",tid,t["providerGeneration"].as_str().unwrap_or("")),"providerInstanceId":t["providerInstanceId"],"nativeThreadId":crate::projection::native_id(db,t)});
    db.execute(
        "INSERT INTO workspace_detaches(id,thread_id,payload) VALUES(?1,?2,?3)",
        params![c["commandId"].as_str(), tid, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    // Queued messages survive the binding change; only live executions are interrupted.
    visit(db, tid, "runs", |id, mut r| {
        if matches!(
            r["status"].as_str(),
            Some("preparing" | "starting" | "running" | "waiting")
        ) {
            r["status"] = json!("interrupted");
            r["completedAt"] = json!(at);
            r["recoveryPending"] = json!(false);
            r["executionRequested"] = json!(false);
            put(db, "runs", &id, tid, &r)?;
            append(db, tid, "run.updated", r, t)?;
            crate::projection::finish(db, tid, &id, t, at, "interrupted")?;
        }
        Ok(())
    })?;
    for (kind, event) in [
        ("attempts", "attempt.updated"),
        ("nodes", "node.updated"),
        ("providerTurns", "provider-turn.updated"),
        ("subagents", "subagent.updated"),
        ("backgroundTasks", "background-task.updated"),
    ] {
        visit(db, tid, kind, |id, mut v| {
            if matches!(
                v["status"].as_str(),
                Some("pending" | "running" | "waiting" | "starting")
            ) {
                // Preparations belonging to a queued run have not acquired a native session.
                if v["runId"]
                    .as_str()
                    .and_then(|rid| get(db, "runs", rid).ok())
                    .is_some_and(|r| r["status"] == "queued")
                {
                    return Ok(());
                }
                v["status"] = json!("interrupted");
                v["completedAt"] = json!(at);
                v["updatedAt"] = json!(at);
                put(db, kind, &id, tid, &v)?;
                append(db, tid, event, v, t)?;
            }
            Ok(())
        })?;
    }
    visit(db, tid, "runtimeRequests", |id, mut r| {
        if r["status"] == "pending" {
            r["status"] = json!("cancelled");
            r["responseCapability"] = json!({"type":"not_resumable","reason":"Workspace changed."});
            r["resolvedAt"] = json!(at);
            put(db, "runtimeRequests", &id, tid, &r)?;
            append(db, tid, "runtime-request.updated", r, t)?;
        }
        Ok(())
    })?;
    visit(db, tid, "providerSessions", |id, session| {
        if !matches!(session["status"].as_str(), Some("stopped" | "error")) {
            // These projection rows are thread/session bindings; upstream removes
            // the binding while the shared provider process may remain alive.
            db.execute(
                "DELETE FROM records WHERE kind='providerSessions' AND id=? AND thread_id=?",
                params![id, tid],
            )
            .map_err(|e| e.to_string())?;
            let seq = append(
                db,
                tid,
                "provider-session.detached",
                json!({"providerSessionId":id,"detachedAt":at,"reason":"Workspace changed."}),
                t,
            )?;
            db.execute("UPDATE events SET body=json_set(body,'$.driver',?1,'$.providerInstanceId',?2) WHERE sequence=?3",params![session["driver"].as_str(),session["providerInstanceId"].as_str(),seq]).map_err(|e|e.to_string())?;
        }
        Ok(())
    })?;
    visit(db, tid, "messages", |id, mut m| {
        if m["streaming"] == true {
            m["streaming"] = json!(false);
            m["updatedAt"] = json!(at);
            put(db, "messages", &id, tid, &m)?;
            append(db, tid, "message.updated", m.clone(), t)?;
            let item = crate::events::message_item(db, tid, &m)?;
            crate::projection::save_item(db, tid, t, item)?;
        }
        Ok(())
    })?;
    db.execute("UPDATE records SET body=json_set(body,'$.status','cancelled','$.error','Workspace changed.') WHERE kind='controlOutbox' AND thread_id=? AND json_extract(body,'$.status') IN ('pending','handed-off','delivery-unknown')",[tid]).map_err(|e|e.to_string())?;
    Ok(true)
}

/// One worker per State. No per-thread task or timer remains after the outbox drains.
pub(crate) fn start(state: &Arc<State>) {
    if state.workspace_detach_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(state);
    let mut changes = state.changed.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let job = {
                let db = state.db.lock().unwrap();
                db.query_row("SELECT id,thread_id,payload,completed_at,attempts FROM workspace_detaches WHERE released_at IS NULL AND retry_at<=? ORDER BY rowid LIMIT 1",[now_ms()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,u32>(4)?))).optional()
            };
            match job {
                Ok(Some((id, tid, payload, completed, attempts))) => {
                    let work = async {
                        if completed.is_none() {
                            let p: Value =
                                serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                            state.executions.cancel(&tid);
                            state.executions.stopped(&tid).await?;
                            let handled = state.native_mcp.dispose_thread(&tid, false).await?;
                            let provider = state.provider.lock().unwrap().clone();
                            if let Some(provider) = provider.filter(|_| !handled) {
                                provider
                                    .detach(
                                        p["providerInstanceId"].as_str().unwrap_or(""),
                                        p["threadKey"].as_str().unwrap_or(""),
                                        p["nativeThreadId"].as_str(),
                                        false,
                                    )
                                    .await?;
                            }
                            state.streams.commands.finish_thread(&tid);
                            state
                                .db
                                .lock()
                                .unwrap()
                                .execute(
                                    "UPDATE workspace_detaches SET completed_at=? WHERE id=?",
                                    params![now_ms(), id],
                                )
                                .map_err(|e| e.to_string())?;
                        }
                        // The release is journalled separately: a crash after native detach must
                        // still advance the durable queue without unloading a successor session.
                        crate::thread_commands::advance_checked(&state, &tid)?;
                        state.db.lock().unwrap().execute("UPDATE workspace_detaches SET released_at=?,last_error=NULL WHERE id=?",params![now_ms(),id]).map_err(|e|e.to_string())?;
                        Ok::<_, String>(())
                    };
                    let result = tokio::time::timeout(Duration::from_secs(45), work)
                        .await
                        .unwrap_or_else(|_| Err("Workspace detach timed out; will retry.".into()));
                    if let Err(error) = result {
                        let result=state.db.lock().unwrap().execute("UPDATE workspace_detaches SET attempts=attempts+1,retry_at=?1,last_error=?2 WHERE id=?3",params![now_ms()+1000i64.saturating_mul(1i64<<attempts.min(5)),error.chars().take(1000).collect::<String>(),id]);
                        if let Err(error) = result {
                            eprintln!("Unable to journal workspace detach: {error}");
                            return;
                        }
                    }
                    state.notify();
                    continue;
                }
                Err(error) => {
                    eprintln!("Unable to read workspace detach outbox: {error}");
                    return;
                }
                Ok(None) => {}
            }
            let retry: Option<i64> = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT min(retry_at) FROM workspace_detaches WHERE released_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            drop(state);
            match retry {
                Some(at) => {
                    tokio::select! {r=changes.changed()=>if r.is_err(){return;},_=tokio::time::sleep(Duration::from_millis(at.saturating_sub(now_ms()).max(1) as u64))=>{}}
                }
                None => {
                    if changes.changed().await.is_err() {
                        return;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events;
    fn fixture() -> (Arc<State>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "o3-workspace-transition-{}",
            crate::store::random_id().unwrap()
        ));
        let s = Arc::new(State::open(&dir).unwrap());
        events::mutate_project(&s,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":dir})).unwrap();
        events::dispatch(&s,&json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"t","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        (s, dir)
    }
    fn read(s: &State, kind: &str, id: &str) -> Value {
        get(&s.db.lock().unwrap(), kind, id).unwrap()
    }
    fn count(s: &State, sql: &str) -> i64 {
        s.db.lock()
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn preconditions_and_receipts_are_atomic_and_noops_keep_native_binding() {
        let (s, _dir) = fixture();
        let change = json!({"type":"thread.metadata.update","commandId":"bind","threadId":"t","worktreePath":"/worktree","branch":"feature","expectedWorktreePath":null,"expectedBranch":null,"expectedEmpty":true});
        for (field, value) in [
            ("expectedWorktreePath", json!("/raced")),
            ("expectedBranch", json!("raced")),
        ] {
            let mut stale = change.clone();
            stale[field] = value;
            assert!(events::dispatch(&s, &stale)
                .unwrap_err()
                .contains("changed before"));
            assert!(read(&s, "thread", "t")["worktreePath"].is_null());
            assert_eq!(count(&s, "SELECT count(*) FROM workspace_detaches"), 0);
        }
        s.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_workspace BEFORE INSERT ON receipts WHEN NEW.command_id='bind' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(events::dispatch(&s, &change)
            .unwrap_err()
            .contains("injected"));
        assert!(read(&s, "thread", "t")["worktreePath"].is_null());
        assert_eq!(count(&s, "SELECT count(*) FROM workspace_detaches"), 0);
        s.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_workspace")
            .unwrap();
        let result = events::dispatch(&s, &change).unwrap();
        let seq = events::sequence(&s.db.lock().unwrap()).unwrap();
        assert_eq!(events::dispatch(&s, &change).unwrap(), result);
        assert_eq!(events::sequence(&s.db.lock().unwrap()).unwrap(), seq);
        assert_eq!(count(&s, "SELECT count(*) FROM workspace_detaches"), 1);
        let mut race = change.clone();
        race["commandId"] = json!("race");
        race["worktreePath"] = json!("/second");
        assert!(events::dispatch(&s, &race).is_err());
        events::dispatch(&s,&json!({"type":"thread.metadata.update","commandId":"same","threadId":"t","worktreePath":"/worktree","branch":"other","title":"Renamed"})).unwrap();
        assert_eq!(count(&s, "SELECT count(*) FROM workspace_detaches"), 1);
        assert_eq!(read(&s, "thread", "t")["title"], "Renamed");
        // Check expectedEmpty against durable messages/runs, not a prior client projection.
        put(
            &s.db.lock().unwrap(),
            "messages",
            "m",
            "t",
            &json!({"id":"m"}),
        )
        .unwrap();
        let mut occupied = change;
        occupied["commandId"] = json!("occupied");
        occupied["expectedWorktreePath"] = json!("/worktree");
        occupied["expectedBranch"] = json!("other");
        occupied["worktreePath"] = Value::Null;
        assert!(events::dispatch(&s, &occupied)
            .unwrap_err()
            .contains("no longer empty"));
        assert_eq!(read(&s, "thread", "t")["worktreePath"], "/worktree");
        drop(s);
        // Fixture trees are retained for private container teardown.
    }

    struct Peer {
        detaches: std::sync::atomic::AtomicUsize,
        inputs: std::sync::Mutex<Vec<(std::path::PathBuf, Option<String>)>>,
        fail: bool,
    }
    impl crate::provider::Provider for Peer {
        fn detach<'a>(
            &'a self,
            _instance: &'a str,
            _key: &'a str,
            native: Option<&'a str>,
            revoke: bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                assert_eq!(native, Some("native-history"));
                assert!(!revoke);
                self.detaches.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    Err("transient fixture failure".into())
                } else {
                    Ok(())
                }
            })
        }
        fn execute<'a>(
            &'a self,
            input: crate::provider::RunInput,
            output: tokio::sync::mpsc::Sender<crate::provider::ProviderEvent>,
            _control: tokio::sync::mpsc::Receiver<crate::provider::ProviderControl>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.inputs
                    .lock()
                    .unwrap()
                    .push((input.cwd, input.native_session_id));
                output
                    .send(crate::provider::ProviderEvent::Completed)
                    .await
                    .map_err(|e| e.to_string())
            })
        }
    }
    async fn wait(s: &State, sql: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while count(s, sql) != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn failed_detach_and_completed_detach_release_survive_restart_without_losing_queued_work()
    {
        // Two durable crash boundaries: failed native detach; native success before queue release.
        for completed in [false, true] {
            let (s, dir) = fixture();
            let work = dir.join("worktree");
            std::fs::create_dir_all(&work).unwrap();
            let failed = Arc::new(Peer {
                detaches: Default::default(),
                inputs: Default::default(),
                fail: true,
            });
            *s.provider.lock().unwrap() = Some(failed.clone());
            let mut t = read(&s, "thread", "t");
            t["codexNativeThreadId"] = json!("native-history");
            put(&s.db.lock().unwrap(), "thread", "t", "t", &t).unwrap();
            events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"old","threadId":"t","messageId":"old","text":"prior history","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
            events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"continue","attachments":[],"dispatchMode":{"type":"queue_after_active"}})).unwrap();
            events::dispatch(&s,&json!({"type":"thread.metadata.update","commandId":"bind","threadId":"t","expectedWorktreePath":null,"worktreePath":work})).unwrap();
            if completed {
                // Persist the state immediately after the provider detach acknowledgement.
                s.db.lock()
                    .unwrap()
                    .execute("UPDATE workspace_detaches SET completed_at=?", [now_ms()])
                    .unwrap();
            } else {
                start(&s);
                wait(&s,"SELECT count(*) FROM workspace_detaches WHERE attempts=1 AND completed_at IS NULL").await;
                assert_eq!(failed.detaches.load(Ordering::SeqCst), 1);
                assert!(failed.inputs.lock().unwrap().is_empty());
                events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"automatic","threadId":"t","messageId":"automatic","text":"automatic follow-up","attachments":[],"deliveryIntent":"auto"})).unwrap();
                assert_eq!(count(&s,"SELECT count(*) FROM records WHERE kind='runs' AND json_extract(body,'$.status')='queued'"),2);
                assert!(events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"too-soon","threadId":"t","messageId":"too-soon","text":"must not cross barrier","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap_err().contains("detach is pending"));
            }
            let weak = Arc::downgrade(&s);
            drop(s);
            tokio::time::timeout(Duration::from_secs(2), async {
                while weak.strong_count() > 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let s = Arc::new(State::open(&dir).unwrap());
            crate::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
            s.db.lock()
                .unwrap()
                .execute("UPDATE workspace_detaches SET retry_at=0", [])
                .unwrap();
            let peer = Arc::new(Peer {
                detaches: Default::default(),
                inputs: Default::default(),
                fail: false,
            });
            *s.provider.lock().unwrap() = Some(peer.clone());
            start(&s);
            crate::recovery::resume(&s).unwrap();
            wait(
                &s,
                "SELECT count(*) FROM workspace_detaches WHERE released_at IS NOT NULL",
            )
            .await;
            let expected = if completed { 1 } else { 2 };
            wait(&s,&format!("SELECT count(*)={expected} FROM records WHERE kind='runs' AND json_extract(body,'$.status')='completed'")).await;
            assert_eq!(
                peer.detaches.load(Ordering::SeqCst),
                if completed { 0 } else { 1 }
            );
            assert_eq!(
                *peer.inputs.lock().unwrap(),
                vec![(work, Some("native-history".into())); if completed { 1 } else { 2 }]
            );
            assert_eq!(
                count(&s, "SELECT count(*) FROM records WHERE kind='messages'"),
                if completed { 2 } else { 3 }
            );
            assert_eq!(count(&s,"SELECT count(*) FROM records WHERE kind='runs' AND json_extract(body,'$.status')='interrupted'"),1);
            drop(s);
            tokio::task::yield_now().await;
            // Fixture trees are retained for private container teardown.
        }
    }
}
