//! Receipt-atomic restart_active: another attempt of the same run (nightly4151, MIT).
use crate::{
    deletion::visit,
    events::{append, get, put},
    provider::ProviderEvent,
    store::{iso, now_ms, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS run_restarts(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,run_id TEXT NOT NULL,old_attempt TEXT NOT NULL,next_attempt TEXT NOT NULL,old_turn TEXT NOT NULL,terminal_at INTEGER,terminal_status TEXT,ready_at INTEGER,admission_at INTEGER,completed_at INTEGER,error TEXT); CREATE INDEX IF NOT EXISTS run_restarts_pending ON run_restarts(run_id) WHERE completed_at IS NULL;").map_err(|e|e.to_string())
}
pub(crate) fn matches(db: &Connection, rid: &str, expected: Option<&str>) -> bool {
    expected.is_none_or(|id| {
        get(db, "runs", rid).is_ok_and(|r| r["activeAttemptId"].as_str() == Some(id))
    })
}
pub(crate) fn pending(db: &Connection, rid: &str) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM run_restarts WHERE run_id=? AND completed_at IS NULL)",
        [rid],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
pub(crate) fn stage(
    db: &Connection,
    c: &Value,
    t: &mut Value,
    at: &str,
    mcp: bool,
) -> Result<String> {
    let tid = c["threadId"].as_str().ok_or("threadId required")?;
    let rid = c["dispatchMode"]["targetRunId"]
        .as_str()
        .ok_or("targetRunId required")?;
    let mut run = get(db, "runs", rid)?;
    if run["threadId"] != tid || run["status"] != "running" || pending(db, rid)? {
        return Err("Target run has no running turn that can be restarted.".into());
    }
    if crate::workspace_transition::pending(db, tid)? || !t["archivedAt"].is_null() {
        return Err("The thread cannot restart while archived or changing workspace.".into());
    }
    if get(db, "turnItems", &format!("{rid}-interrupt-request")).is_ok() {
        return Err("Target run is stopping and cannot be restarted.".into());
    }
    let aid = run["activeAttemptId"]
        .as_str()
        .ok_or("Active attempt missing")?
        .to_owned();
    let oldroot = run["rootNodeId"]
        .as_str()
        .ok_or("Root node missing")?
        .to_owned();
    let turn:Option<String>=db.query_row("SELECT id FROM records WHERE kind='providerTurns' AND thread_id=?1 AND json_extract(body,'$.runAttemptId')=?2 AND json_extract(body,'$.status')='running' ORDER BY rowid DESC LIMIT 1",params![tid,aid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let turn = turn.ok_or("No running provider turn found for restart.")?;
    let pt = get(
        db,
        "providerThreads",
        run["providerThreadId"]
            .as_str()
            .ok_or("Provider thread missing")?,
    )?;
    let session = get(
        db,
        "providerSessions",
        pt["providerSessionId"]
            .as_str()
            .ok_or("Provider session missing")?,
    )?;
    if session["capabilities"]["turns"]["supportsInterrupt"] != true
        || session["capabilities"]["turns"]["supportsSteeringByInterruptRestart"] != true
    {
        return Err("The provider cannot satisfy a required interrupt-and-restart.".into());
    }
    let original =
        get(db, "messages", run["userMessageId"].as_str().unwrap_or("")).unwrap_or_default();
    for msg in [&original, c] {
        let text = msg["text"].as_str().unwrap_or("").trim().to_lowercase();
        if matches!(text.as_str(), "/compact" | "/logout")
            || text == "/goal"
            || text.starts_with("/goal ")
        {
            return Err("Maintenance and goal commands must run as separate turns.".into());
        }
    }
    if !c["text"].is_string() || !c["attachments"].is_array() {
        return Err("text and attachments required".into());
    }
    let selection = c
        .get("modelSelection")
        .unwrap_or(&run["modelSelection"])
        .clone();
    if selection["instanceId"] != run["providerInstanceId"] {
        return Err("Active cross-instance selection handoff is not implemented yet.".into());
    }
    if !selection["model"].as_str().is_some_and(|s| !s.is_empty()) {
        return Err("Invalid model selection".into());
    }
    let ordinal:i64=db.query_row("SELECT coalesce(max(json_extract(body,'$.attemptOrdinal')),0)+1 FROM records WHERE kind='attempts' AND thread_id=?1 AND json_extract(body,'$.runId')=?2",params![tid,rid],|r|r.get(0)).map_err(|e|e.to_string())?;
    let next = format!("attempt-{rid}-{ordinal}");
    let root = format!("{rid}-attempt-{ordinal}");
    let mut old = get(db, "attempts", &aid)?;
    old["status"] = json!("superseded");
    old["completedAt"] = json!(at);
    put(db, "attempts", &aid, tid, &old)?;
    append(db, tid, "attempt.updated", old, t)?;
    if let Ok(mut n) = get(db, "nodes", &oldroot) {
        n["status"] = json!("interrupted");
        n["completedAt"] = json!(at);
        put(db, "nodes", &oldroot, tid, &n)?;
        append(db, tid, "node.updated", n, t)?;
    }
    // Dispose only the old attempt's native work. App-owned delegations keep their threads.
    for (kind, event) in [
        ("turnItems", "turn-item.updated"),
        ("subagents", "subagent.updated"),
        ("backgroundTasks", "background-task.updated"),
    ] {
        visit(db, tid, kind, |id, mut v| {
            if v["runId"] == rid
                && v["origin"] != "app_owned"
                && !get(db, "subagents", &id).is_ok_and(|task| task["origin"] == "app_owned")
                && matches!(
                    v["status"].as_str(),
                    Some("pending" | "running" | "waiting" | "starting")
                )
            {
                v["status"] = json!("interrupted");
                v["completedAt"] = json!(at);
                v["updatedAt"] = json!(at);
                if v.get("streaming").is_some() {
                    v["streaming"] = json!(false);
                }
                if kind == "turnItems" {
                    crate::projection::save_item(db, tid, t, v)?;
                } else {
                    put(db, kind, &id, tid, &v)?;
                    append(db, tid, event, v, t)?;
                }
            }
            Ok(())
        })?;
    }
    visit(db, tid, "messages", |id, mut m| {
        if m["runId"] == rid && m["streaming"] == true {
            m["streaming"] = json!(false);
            m["updatedAt"] = json!(at);
            put(db, "messages", &id, tid, &m)?;
            append(db, tid, "message.updated", m, t)?;
        }
        Ok(())
    })?;
    visit(db, tid, "runtimeRequests", |id, mut r| {
        if r["status"] == "pending"
            && get(db, "nodes", r["nodeId"].as_str().unwrap_or("")).is_ok_and(|n| n["runId"] == rid)
        {
            r["status"] = json!("cancelled");
            r["responseCapability"] =
                json!({"type":"not_resumable","reason":"The run was restarted."});
            r["resolvedAt"] = json!(at);
            put(db, "runtimeRequests", &id, tid, &r)?;
            append(db, tid, "runtime-request.updated", r, t)?;
        }
        Ok(())
    })?;
    run["activeAttemptId"] = json!(next);
    run["rootNodeId"] = json!(root);
    run["userMessageId"] = c["messageId"].clone();
    run["status"] = json!("starting");
    run["codexProviderTurnId"] = Value::Null;
    run["completedAt"] = Value::Null;
    run["error"] = Value::Null;
    run["modelSelection"] = selection.clone();
    run["executionRequested"] = json!(true);
    run["recoveryPending"] = json!(false);
    let changed_selection = t["modelSelection"] != selection;
    t["modelSelection"] = selection;
    t["updatedAt"] = json!(at);
    put(db, "thread", tid, tid, t)?;
    if changed_selection {
        append(db, tid, "thread.model-selection-updated", t.clone(), t)?;
    }
    put(db, "runs", rid, tid, &run)?;
    append(db, tid, "run.updated", run.clone(), t)?;
    let attempt = json!({"id":next,"runId":rid,"attemptOrdinal":ordinal,"rootNodeId":root,"providerInstanceId":run["providerInstanceId"],"providerThreadId":run["providerThreadId"],"providerTurnId":null,"reason":"steering_restart","status":"pending","startedAt":null,"completedAt":null});
    put(db, "attempts", &next, tid, &attempt)?;
    append(db, tid, "attempt.updated", attempt, t)?;
    let node = json!({"id":root,"threadId":tid,"runId":rid,"rootNodeId":root,"parentNodeId":null,"kind":"root_turn","status":"pending","countsForRun":true,"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":null,"completedAt":null});
    put(db, "nodes", &root, tid, &node)?;
    append(db, tid, "node.updated", node, t)?;
    let mid = c["messageId"].as_str().ok_or("messageId required")?;
    let mut m = json!({"id":mid,"threadId":tid,"runId":rid,"nodeId":root,"providerThreadId":run["providerThreadId"],"providerTurnId":null,"role":"user","text":c["text"],"attachments":c["attachments"],"streaming":false,"createdBy":if mcp{"agent"}else{"user"},"creationSource":if mcp{"mcp"}else{"web"},"createdAt":at,"updatedAt":at});
    if let Some(context) = c.get("context") {
        m["context"] = context.clone();
    }
    put(db, "messages", mid, tid, &m)?;
    append(db, tid, "message.updated", m.clone(), t)?;
    m["inputIntent"] = json!("steer");
    let item = crate::events::message_item(db, tid, &m)?;
    append(db, tid, "turn-item.updated", item, t)?;
    db.execute("INSERT INTO run_restarts(id,thread_id,run_id,old_attempt,next_attempt,old_turn) VALUES(?1,?2,?3,?4,?5,?6)",params![c["commandId"].as_str(),tid,rid,aid,next,turn]).map_err(|e|e.to_string())?;
    Ok(rid.into())
}
pub(crate) fn note_terminal(
    db: &Connection,
    rid: &str,
    attempt: &str,
    event: &ProviderEvent,
) -> Result<()> {
    let status = match event {
        ProviderEvent::Interrupted => "interrupted",
        ProviderEvent::Completed => "completed",
        ProviderEvent::Failed(_) => "failed",
        _ => return Ok(()),
    };
    let error = if let ProviderEvent::Failed(message) = event {
        Some(message.chars().take(1000).collect::<String>())
    } else {
        None
    };
    db.execute("UPDATE run_restarts SET terminal_at=?1,terminal_status=?2,error=?5 WHERE run_id=?3 AND old_attempt=?4 AND completed_at IS NULL AND terminal_at IS NULL",params![now_ms(),status,rid,attempt,error]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(crate) fn observed(s: &State, rid: &str, attempt: &str, event: &ProviderEvent) -> Result<()> {
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    note_terminal(&tx, rid, attempt, event)?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(())
}
pub(crate) fn admitted(db: &Connection, rid: &str, attempt: &str) -> Result<()> {
    db.execute("UPDATE run_restarts SET completed_at=?1 WHERE run_id=?2 AND next_attempt=?3 AND completed_at IS NULL",params![now_ms(),rid,attempt]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(crate) fn admission(s: &State, rid: &str, attempt: Option<&str>) -> Result<()> {
    s.db.lock().unwrap().execute("UPDATE run_restarts SET admission_at=?1 WHERE run_id=?2 AND next_attempt=?3 AND completed_at IS NULL",params![now_ms(),rid,attempt]).map_err(|e|e.to_string())?;
    Ok(())
}
fn fail(db: &Connection, id: &str, error: &str) -> Result<()> {
    let (tid, rid, next): (String, String, String) = db
        .query_row(
            "SELECT thread_id,run_id,next_attempt FROM run_restarts WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    let mut r = get(db, "runs", &rid)?;
    if r["activeAttemptId"] == next
        && matches!(
            r["status"].as_str(),
            Some("starting" | "running" | "waiting")
        )
    {
        let t = get(db, "thread", &tid)?;
        let at = iso(now_ms());
        r["status"] = json!("failed");
        r["error"] = json!(error);
        r["completedAt"] = json!(at);
        r["executionRequested"] = json!(false);
        r["recoveryPending"] = json!(false);
        put(db, "runs", &rid, &tid, &r)?;
        append(db, &tid, "run.updated", r, &t)?;
        crate::projection::finish(db, &tid, &rid, &t, &at, "failed")?;
    }
    db.execute(
        "UPDATE run_restarts SET completed_at=?1,error=?2 WHERE id=?3",
        params![now_ms(), error, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn recover(db: &Connection) -> Result<()> {
    let mut after = 0;
    loop {
        let row:Option<(i64,String,Option<i64>,Option<String>)>=db.query_row("SELECT rowid,id,admission_at,terminal_status FROM run_restarts WHERE rowid>? AND completed_at IS NULL ORDER BY rowid LIMIT 1",[after],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
        let Some((row, id, admission, terminal)) = row else {
            break;
        };
        after = row;
        if admission.is_some() {
            fail(db,&id,"Server restarted while replacement admission was unknown; inspect native history before retrying.")?;
        } else if terminal.as_deref() == Some("failed") {
            fail(
                db,
                &id,
                "Previous native turn failed during restart; replacement was not sent.",
            )?;
        } else {
            let (tid, oldturn): (String, String) = db
                .query_row(
                    "SELECT thread_id,old_turn FROM run_restarts WHERE id=?",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| e.to_string())?;
            if let Ok(mut turn) = get(db, "providerTurns", &oldturn) {
                if turn["status"] == "running" {
                    let t = get(db, "thread", &tid)?;
                    turn["status"] = json!("interrupted");
                    turn["completedAt"] = json!(iso(now_ms()));
                    put(db, "providerTurns", &oldturn, &tid, &turn)?;
                    append(db, &tid, "provider-turn.updated", turn, &t)?;
                }
            }
            db.execute("UPDATE run_restarts SET terminal_at=coalesce(terminal_at,?1),terminal_status=coalesce(terminal_status,'interrupted'),ready_at=coalesce(ready_at,?1) WHERE id=?2",params![now_ms(),id]).map_err(|e|e.to_string())?;
        }
    }
    Ok(())
}
pub(crate) fn start(s: &Arc<State>) {
    if s.restart_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(s);
    let mut changes = s.changed.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(s) = weak.upgrade() else {
                return;
            };
            let jobs = {
                let db = s.db.lock().unwrap();
                let mut q=match db.prepare("SELECT id FROM run_restarts WHERE completed_at IS NULL ORDER BY rowid LIMIT 32"){Ok(q)=>q,Err(e)=>{eprintln!("Restart journal: {e}");return;}};
                let rows = q.query_map([], |r| r.get::<_, String>(0));
                match rows {
                    Ok(rows) => rows.collect::<std::result::Result<Vec<_>, _>>(),
                    Err(e) => Err(e),
                }
            };
            let Ok(jobs) = jobs else {
                return;
            };
            for id in jobs {
                if let Err(error) = drive(&s, &id, Duration::from_secs(30)).await {
                    let mut db = s.db.lock().unwrap();
                    let result = (|| -> Result<()> {
                        let tx = db.transaction().map_err(|e| e.to_string())?;
                        fail(&tx, &id, &error)?;
                        tx.commit().map_err(|e| e.to_string())
                    })();
                    if let Err(e) = result {
                        eprintln!("Unable to journal failed restart: {e}");
                        return;
                    }
                    drop(db);
                    s.notify();
                }
            }
            drop(s);
            if changes.changed().await.is_err() {
                return;
            }
        }
    });
}
async fn drive(s: &Arc<State>, id: &str, timeout: Duration) -> Result<()> {
    let (tid, rid, next, ready): (String, String, String, Option<i64>) =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT thread_id,run_id,next_attempt,ready_at FROM run_restarts WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| e.to_string())?;
    if ready.is_none() {
        if tokio::time::timeout(timeout, s.executions.wait_run(&rid))
            .await
            .is_err()
        {
            s.executions.cancel_run(&rid);
            return Err(
                "Native turn did not settle during restart; replacement was not sent.".into(),
            );
        }
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let status: Option<String> = tx
            .query_row(
                "SELECT terminal_status FROM run_restarts WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !matches!(status.as_deref(), Some("interrupted" | "completed")) {
            return Err("Previous native turn ended without a restart acknowledgement; replacement was not sent.".into());
        }
        let oldturn: String = tx
            .query_row("SELECT old_turn FROM run_restarts WHERE id=?", [id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if let Ok(mut turn) = get(&tx, "providerTurns", &oldturn) {
            let t = get(&tx, "thread", &tid)?;
            turn["status"] = json!(status);
            turn["completedAt"] = json!(iso(now_ms()));
            put(&tx, "providerTurns", &oldturn, &tid, &turn)?;
            append(&tx, &tid, "provider-turn.updated", turn, &t)?;
        }
        tx.execute(
            "UPDATE run_restarts SET ready_at=? WHERE id=?",
            params![now_ms(), id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    if s.active.lock().unwrap().contains_key(&rid) {
        return Ok(());
    }
    let text = {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut r = get(&tx, "runs", &rid)?;
        let t = get(&tx, "thread", &tid)?;
        if r["activeAttemptId"] != next
            || r["status"] != "starting"
            || !t["deletedAt"].is_null()
            || !t["archivedAt"].is_null()
        {
            if r["activeAttemptId"] == next && r["status"] == "starting" {
                let at = iso(now_ms());
                r["status"] = json!("cancelled");
                r["completedAt"] = json!(at);
                r["executionRequested"] = json!(false);
                put(&tx, "runs", &rid, &tid, &r)?;
                append(&tx, &tid, "run.updated", r, &t)?;
                crate::projection::finish(&tx, &tid, &rid, &t, &at, "cancelled")?;
            }
            tx.execute(
                "UPDATE run_restarts SET completed_at=? WHERE id=?",
                params![now_ms(), id],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            s.notify();
            return Ok(());
        }
        let text = get(
            &tx,
            "messages",
            r["userMessageId"]
                .as_str()
                .ok_or("Restart message missing")?,
        )?["text"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        tx.commit().map_err(|e| e.to_string())?;
        text
    };
    if s.provider.lock().unwrap().is_none() {
        return Err("No provider adapter available for replacement turn.".into());
    }
    crate::provider::start(s.clone(), tid, rid, text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, ProviderControl, RunInput};
    use std::{future::Future, pin::Pin, sync::Mutex};
    struct Peer(Mutex<Vec<String>>);
    impl Provider for Peer {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            out: tokio::sync::mpsc::Sender<ProviderEvent>,
            mut control: tokio::sync::mpsc::Receiver<ProviderControl>,
            mut cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.0.lock().unwrap().push(input.text.clone());
                out.send(ProviderEvent::Codex(
                    json!({"method":"o3/binding","params":{"threadId":"native","cwd":input.cwd}}),
                ))
                .await
                .unwrap();
                out.send(ProviderEvent::Codex(json!({"method":"turn/started","params":{"threadId":"native","turn":{"id":input.text}}}))).await.unwrap();
                if input.text == "hold" {
                    loop {
                        tokio::select! {_=cancel.changed()=>break,_=control.recv()=>{}}
                    }
                } else {
                    out.send(ProviderEvent::Completed).await.unwrap();
                }
                Ok(())
            })
        }
    }
    fn record(s: &State, kind: &str, id: &str) -> Value {
        get(&s.db.lock().unwrap(), kind, id).unwrap()
    }
    async fn wait(mut f: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !f() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn fixture() -> (Arc<State>, Arc<Peer>, std::path::PathBuf, String) {
        let dir =
            std::env::temp_dir().join(format!("o3-restart-{}", crate::store::random_id().unwrap()));
        let s = Arc::new(State::open(&dir).unwrap());
        let peer = Arc::new(Peer(Mutex::new(Vec::new())));
        *s.provider.lock().unwrap() = Some(peer.clone());
        crate::events::mutate_project(&s,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":dir})).unwrap();
        crate::events::dispatch(&s,&json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"t","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        crate::events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"first","threadId":"t","messageId":"first","text":"hold","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let rid = get(&s.db.lock().unwrap(), "messages", "first").unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_owned();
        wait(|| record(&s, "runs", &rid)["status"] == "running").await;
        crate::events::dispatch(&s,&json!({"type":"message.dispatch","commandId":"restart","threadId":"t","messageId":"next","text":"replacement","attachments":[],"dispatchMode":{"type":"restart_active","targetRunId":rid}})).unwrap();
        (s, peer, dir, rid)
    }
    #[tokio::test]
    async fn restart_recovery_resumes_unsubmitted_replacement_but_reports_ambiguous_admission() {
        for (ambiguous, rejected) in [(false, false), (true, false), (false, true)] {
            let (s, _peer, dir, rid) = fixture().await;
            s.executions.cancel_run(&rid);
            s.executions.wait_run(&rid).await;
            if ambiguous {
                s.db.lock()
                    .unwrap()
                    .execute("UPDATE run_restarts SET ready_at=1,admission_at=1", [])
                    .unwrap();
            }
            if rejected {
                s.db.lock()
                    .unwrap()
                    .execute(
                        "UPDATE run_restarts SET terminal_at=1,terminal_status='failed'",
                        [],
                    )
                    .unwrap();
            }
            drop(s);
            let s = Arc::new(State::open(&dir).unwrap());
            crate::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
            let peer = Arc::new(Peer(Mutex::new(Vec::new())));
            *s.provider.lock().unwrap() = Some(peer.clone());
            start(&s);
            crate::recovery::resume(&s).unwrap();
            if ambiguous || rejected {
                assert_eq!(record(&s, "runs", &rid)["status"], "failed");
                assert!(record(&s, "runs", &rid)["error"]
                    .as_str()
                    .unwrap()
                    .contains(if ambiguous {
                        "admission was unknown"
                    } else {
                        "Previous native turn failed"
                    }));
                assert!(peer.0.lock().unwrap().is_empty());
            } else {
                wait(|| record(&s, "runs", &rid)["status"] == "completed").await;
                assert_eq!(*peer.0.lock().unwrap(), vec!["replacement"]);
                assert_eq!(
                    record(&s, "providerTurns", "codex-turn-hold")["status"],
                    "interrupted"
                );
            }
            assert_eq!(record(&s, "messages", "next")["text"], "replacement");
            assert_eq!(
                s.db.lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap(),
                1
            );
            drop(s);
            tokio::task::yield_now().await;
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn unacknowledged_interrupt_times_out_without_sending_replacement() {
        let (s, peer, dir, rid) = fixture().await;
        let error = drive(&s, "restart", Duration::from_millis(25))
            .await
            .unwrap_err();
        assert!(error.contains("did not settle"));
        {
            let mut db = s.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            fail(&tx, "restart", &error).unwrap();
            tx.commit().unwrap();
        }
        s.executions.wait_run(&rid).await;
        assert_eq!(record(&s, "runs", &rid)["status"], "failed");
        assert_eq!(*peer.0.lock().unwrap(), vec!["hold"]);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
