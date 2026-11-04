//! ThreadLaunchService: accept a durable deferred run before workspace preparation.
use crate::{
    events::{self, append, get, put},
    store::{iso, now_ms, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
type Result<T> = std::result::Result<T, String>;
#[derive(Default)]
pub(crate) struct Registry {
    started: AtomicBool,
    pub(crate) active: Mutex<HashSet<String>>,
}
struct Flight {
    state: Arc<State>,
    thread: String,
}
impl Drop for Flight {
    fn drop(&mut self) {
        self.state
            .launches
            .active
            .lock()
            .unwrap()
            .remove(&self.thread);
        self.state.notify();
    }
}
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS thread_launches(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,run_id TEXT,request TEXT NOT NULL,snapshot TEXT,phase TEXT NOT NULL DEFAULT 'admitted',cancelled INTEGER NOT NULL DEFAULT 0,uncancellable INTEGER NOT NULL DEFAULT 0,released INTEGER NOT NULL DEFAULT 0,completed INTEGER NOT NULL DEFAULT 0,cleanup_pending INTEGER NOT NULL DEFAULT 0,root TEXT,path TEXT,branch TEXT,owned INTEGER NOT NULL DEFAULT 0,bound INTEGER NOT NULL DEFAULT 0,script_started INTEGER NOT NULL DEFAULT 0,terminal_id TEXT,rename_to TEXT,retry_at INTEGER NOT NULL DEFAULT 0,error TEXT);CREATE TABLE IF NOT EXISTS launch_claims(command_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,project_id TEXT NOT NULL,run_id TEXT);CREATE INDEX IF NOT EXISTS thread_launches_pending ON thread_launches(completed) WHERE completed=0;CREATE INDEX IF NOT EXISTS thread_launches_thread ON thread_launches(thread_id);").map_err(|e|e.to_string())
}
pub(crate) fn enabled(s: &State, pid: &str, instance: &Value) -> Result<()> {
    let id = instance
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Provider instance required")?;
    let settings = crate::settings::read(s)?;
    if settings["providerInstances"][id]["enabled"] == false
        || settings["providerInstances"][id]["config"]["enabled"] == false
        || settings["projectSettingsOverrides"][pid]["providerInstances"][id]["enabled"] == false
    {
        return Err("Provider instance is disabled for this project".into());
    }
    Ok(())
}
pub(crate) fn enqueue(
    db: &Connection,
    tid: &str,
    rid: Option<&str>,
    id: &str,
    plan: &Value,
) -> Result<()> {
    let mut plan = plan.clone();
    let thread = get(db, "thread", tid)?;
    if plan.get("expectedWorktreePath").is_none() {
        plan["expectedWorktreePath"] = thread["worktreePath"].clone();
    }
    if plan.get("expectedBranch").is_none() {
        plan["expectedBranch"] = thread["branch"].clone();
    }
    let snapshot = crate::worktree_setup::begin(db, tid, &plan)?;
    db.execute(
        "INSERT INTO thread_launches(id,thread_id,run_id,request,snapshot) VALUES(?1,?2,?3,?4,?5)",
        params![
            id,
            tid,
            rid,
            plan.to_string(),
            if snapshot.is_null() {
                None
            } else {
                Some(snapshot.to_string())
            }
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn launch(s: &Arc<State>, incoming: &Value) -> Result<Value> {
    let _workspace = s
        .worktree_gate
        .try_read()
        .map_err(|_| "worktree cleanup in progress; retry launch")?;
    let _lock = s.launch_lock.lock().unwrap();
    let (normalized, mut scratch) = crate::managed_folders::normalize(s, incoming)?;
    let p = &normalized;
    let cid = p["commandId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("commandId required")?;
    let pid = p["projectId"].as_str().ok_or("projectId required")?;
    enabled(s, pid, &p["modelSelection"]["instanceId"])?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let project = get(&tx, "project", pid).map_err(|_| "Project was not found")?;
    if !project["deletedAt"].is_null() {
        return Err("Project was deleted".into());
    }
    let claim: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT thread_id,project_id,run_id FROM launch_claims WHERE command_id=?",
            [cid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let receipt: Option<String> = tx
        .query_row(
            "SELECT result FROM receipts WHERE command_id=?",
            [cid],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    // Profiles from before staged launch have a composite launch receipt.
    if claim.is_none() {
        if let Some(receipt) = receipt {
            let mut result: Value = serde_json::from_str(&receipt).map_err(|e| e.to_string())?;
            let tid = result["threadId"]
                .as_str()
                .ok_or("Command ID already belongs to another operation")?;
            let t = get(&tx, "thread", tid)?;
            if t["projectId"] != pid || !t["deletedAt"].is_null() {
                return Err("Launch target is no longer active in this project".into());
            }
            result["projection"] = events::projection_db(&tx, tid)?;
            result["resumed"] = json!(true);
            return Ok(result);
        }
    }
    if s.rollback_pending.load(Ordering::Acquire) {
        return Err("rollback recovery required".into());
    }
    let was_claimed = claim.is_some();
    let tid = claim
        .as_ref()
        .map(|c| c.0.clone())
        .or_else(|| p["threadId"].as_str().map(str::to_owned))
        .unwrap_or(format!("thread-{cid}"));
    if claim.as_ref().is_some_and(|c| c.1 != pid)
        || p["threadId"].as_str().is_some_and(|asked| asked != tid)
    {
        return Err("Launch project or thread identity changed".into());
    }
    let strategy = &p["workspaceStrategy"];
    if !matches!(
        strategy["type"].as_str(),
        Some("root" | "existing_worktree" | "worktree")
    ) {
        return Err("Invalid workspace strategy".into());
    }
    if strategy["type"] == "worktree"
        && !strategy["baseRef"].as_str().is_some_and(|s| !s.is_empty())
    {
        return Err("baseRef required".into());
    }
    if strategy["type"] == "existing_worktree" && !strategy["worktreePath"].is_string() {
        return Err("worktreePath required".into());
    }
    if let Some(m) = p.get("initialMessage") {
        if !m["text"].is_string() || !m["attachments"].is_array() {
            return Err("text and attachments required".into());
        }
        if s.provider.lock().unwrap().is_none() {
            return Err("no provider adapter configured".into());
        }
    }
    let at = iso(now_ms());
    let thread = if was_claimed {
        let t = get(&tx, "thread", &tid)?;
        if t["projectId"] != pid || !t["deletedAt"].is_null() {
            return Err("Launch target is no longer active in this project".into());
        }
        t
    } else if p["reuseExistingThread"] == true {
        if !p["threadId"].is_string() {
            return Err("threadId required for reuse".into());
        }
        let mut t = get(&tx, "thread", &tid)?;
        let occupied:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=? AND kind IN ('runs','messages'))",[&tid],|r|r.get(0)).map_err(|e|e.to_string())?;
        if t["projectId"] != pid
            || occupied
            || !t["archivedAt"].is_null()
            || !t["deletedAt"].is_null()
        {
            return Err("only an empty active thread in the target project can be reused".into());
        }
        t["updatedAt"] = json!(at);
        events::import_native(s, &tx, p, &tid, &at, &mut t)?;
        put(&tx, "thread", &tid, &tid, &t)?;
        append(&tx, &tid, "thread.metadata-updated", t.clone(), &t)?;
        t
    } else {
        let mut create = p.clone();
        create["type"] = json!("thread.create");
        create["threadId"] = json!(tid);
        create["branch"] = strategy["branch"].clone();
        create["worktreePath"] = if strategy["type"] == "existing_worktree" {
            strategy["worktreePath"].clone()
        } else {
            Value::Null
        };
        let mut t = events::new_thread(&tx, &create, &tid, &at)?;
        events::import_native(s, &tx, p, &tid, &at, &mut t)?;
        put(&tx, "thread", &tid, &tid, &t)?;
        append(&tx, &tid, "thread.created", t.clone(), &t)?;
        t
    };
    if !was_claimed {
        tx.execute(
            "INSERT INTO launch_claims(command_id,thread_id,project_id) VALUES(?1,?2,?3)",
            params![cid, tid, pid],
        )
        .map_err(|e| e.to_string())?;
        let sequence: i64 = tx
            .query_row("SELECT coalesce(max(sequence),0) FROM events", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO receipts VALUES(?1,?2)",
            params![cid, json!({"threadId":tid,"sequence":sequence}).to_string()],
        )
        .map_err(|e| e.to_string())?;
        if p.get("initialMessage").is_none() {
            enqueue(&tx, &tid, None, cid, p)?;
        }
    }
    let mut result =
        json!({"threadId":tid,"projection":events::projection_db(&tx,&tid)?,"resumed":was_claimed});
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    scratch.commit();
    s.notify();
    let Some(m) = p.get("initialMessage") else {
        return Ok(result);
    };
    if let Some(rid) = claim.and_then(|c| c.2) {
        let db = s.db.lock().unwrap();
        get(&db, "runs", &rid)?;
        result["projection"] = events::projection_db(&db, &tid)?;
        result["resumed"] = json!(true);
        return Ok(result);
    }
    // The claim is intentionally durable if this second admission fails. A retry
    // recovers its thread id and repeats only the previously unaccepted message.
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut t = get(&tx, "thread", &tid)?;
    if !t["archivedAt"].is_null() || !t["deletedAt"].is_null() {
        return Err("The launch target is archived or deleted".into());
    }
    t["modelSelection"] = p["modelSelection"].clone();
    t["providerInstanceId"] = p["modelSelection"]["instanceId"].clone();
    if p["generateTitle"] == true {
        if let Some(text) = m["text"].as_str().filter(|v| !v.trim().is_empty()) {
            t["title"] = json!(crate::title::fallback(text));
            t["titleRegeneration"] = json!({"requestId":cid,"startedAt":at});
        }
    }
    put(&tx, "thread", &tid, &tid, &t)?;
    let preparation = if was_claimed
        && strategy["type"] == "root"
        && thread["worktreePath"].is_string()
    {
        json!({"type":"existing_worktree","worktreePath":thread["worktreePath"],"branch":strategy["branch"]})
    } else {
        strategy.clone()
    };
    let mut message = json!({"type":"message.dispatch","commandId":format!("{cid}:initial-message"),"threadId":tid,"messageId":m["messageId"].as_str().map(str::to_owned).unwrap_or(format!("message-{cid}")),"text":m["text"],"attachments":m["attachments"],"dispatchMode":{"type":"defer_start","workspaceStrategy":preparation}});
    if let Some(context) = m.get("context") {
        message["context"] = context.clone();
    }
    if p["generateTitle"] == true {
        message["titleSeed"] = p["title"].clone();
    }
    let (accepted, rid, _) = events::stage_message(s, &tx, &message, &mut t, &at, false)?;
    tx.execute(
        "UPDATE launch_claims SET run_id=?1 WHERE command_id=?2",
        params![rid, cid],
    )
    .map_err(|e| e.to_string())?;
    result = json!({"threadId":tid,"projection":events::projection_db(&tx,&tid)?,"resumed":false,"sequence":accepted["sequence"]});
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    if p["generateTitle"] == true {
        if let Some(text) = m["text"].as_str() {
            crate::title::start(s.clone(), tid, cid.into(), text.into());
        }
    }
    Ok(result)
}

pub(crate) fn start(s: &Arc<State>) {
    if s.launches.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(s);
    let mut changed = s.changed.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(s) = weak.upgrade() else {
                return;
            };
            let ids = {
                let db = s.db.lock().unwrap();
                let mut q = match db.prepare(
                    "SELECT id,thread_id FROM thread_launches l WHERE (completed=0 OR cleanup_pending=1) AND retry_at<=?1 AND rowid=(SELECT min(rowid) FROM thread_launches earlier WHERE (earlier.completed=0 OR earlier.cleanup_pending=1) AND earlier.thread_id=l.thread_id) ORDER BY rowid LIMIT 32",
                ) {
                    Ok(q) => q,
                    Err(_) => return,
                };
                let rows = q.query_map([now_ms()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                });
                match rows {
                    Ok(rows) => rows.filter_map(std::result::Result::ok).collect::<Vec<_>>(),
                    Err(_) => return,
                }
            };
            for (id, tid) in ids {
                let mut active = s.launches.active.lock().unwrap();
                if active.len() >= 8 || !active.insert(tid.clone()) {
                    continue;
                }
                drop(active);
                let state = s.clone();
                tokio::spawn(async move {
                    let _flight = Flight {
                        state: state.clone(),
                        thread: tid,
                    };
                    let result = crate::launch_worker::run(&state, &id).await;
                    if let Err(error) = result {
                        if let Err(e) = crate::launch_worker::fail(&state, &id, &error).await {
                            eprintln!("Launch cleanup: {e}");
                            let _ = state.db.lock().unwrap().execute(
                                "UPDATE thread_launches SET retry_at=?1 WHERE id=?2",
                                params![now_ms() + 1000, id],
                            );
                            let weak = Arc::downgrade(&state);
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                if let Some(s) = weak.upgrade() {
                                    s.notify();
                                }
                            });
                        }
                    }
                });
            }
            drop(s);
            if changed.changed().await.is_err() {
                return;
            }
        }
    });
}
pub(crate) fn pending(db: &Connection, rid: &str) -> Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM thread_launches WHERE run_id=? AND ((completed=0 AND released=0) OR cleanup_pending=1))",[rid],|r|r.get(0)).map_err(|e|e.to_string())
}
pub(crate) fn recover(db: &Connection) -> Result<()> {
    db.execute("UPDATE thread_launches SET phase='cleanup',error='Server restarted during workspace preparation; retry explicitly after cleanup.' WHERE completed=0 AND phase!='admitted'",[]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(crate) async fn cancel(s: &State, p: &Value) -> std::result::Result<Value, Value> {
    let tid = crate::p2::str_field(p, "threadId")?;
    let mut changes = s.changed.subscribe();
    let id = {
        let db = s.db.lock().unwrap();
        let id:Option<String>=db.query_row("SELECT id FROM thread_launches WHERE thread_id=? AND completed=0 AND uncancellable=0 AND snapshot IS NOT NULL ORDER BY rowid DESC LIMIT 1",[tid],|r|r.get(0)).optional().map_err(|e|crate::p2::invalid(e.to_string()))?;
        if let Some(id) = &id {
            db.execute("UPDATE thread_launches SET cancelled=1 WHERE id=?", [id])
                .map_err(|e| crate::p2::invalid(e.to_string()))?;
        }
        id
    };
    let Some(id) = id else {
        return Ok(json!({"cancelled":false}));
    };
    s.notify();
    loop {
        let done =
            s.db.lock()
                .unwrap()
                .query_row(
                    "SELECT completed!=0 FROM thread_launches WHERE id=?",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap_or(false);
        if done {
            return Ok(json!({"cancelled":true}));
        }
        changes
            .changed()
            .await
            .map_err(|_| crate::p2::invalid("Setup worker notifications ended"))?;
    }
}
pub(crate) fn retry(
    s: &Arc<State>,
    c: &Value,
    authority: Option<&crate::mcp_access::Authority>,
) -> Result<Value> {
    let cid = c["commandId"].as_str().ok_or("commandId required")?;
    let tid = c["threadId"].as_str().ok_or("threadId required")?;
    let rid = c["runId"].as_str().ok_or("runId required")?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if let Some(a) = authority {
        a.check(&tx, c)?;
    }
    let receipt: Option<String> = tx
        .query_row(
            "SELECT result FROM receipts WHERE command_id=?",
            [cid],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(receipt) = receipt {
        return serde_json::from_str(&receipt).map_err(|e| e.to_string());
    }
    let t = get(&tx, "thread", tid)?;
    let mut r = get(&tx, "runs", rid)?;
    if r["threadId"] != tid
        || r["status"] != "failed"
        || !r["workspacePreparation"].is_object()
        || !t["archivedAt"].is_null()
        || !t["deletedAt"].is_null()
    {
        return Err("Run has no failed workspace preparation to retry".into());
    }
    enabled(
        s,
        t["projectId"].as_str().unwrap_or(""),
        &r["providerInstanceId"],
    )?;
    if pending(&tx, rid)? {
        return Err("Preparation cleanup is still pending".into());
    }
    let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='runs' AND thread_id=?1 AND id!=?2 AND json_extract(body,'$.status') IN ('preparing','starting','running','waiting'))",params![tid,rid],|r|r.get(0)).map_err(|e|e.to_string())?;
    if busy {
        return Err("Another run is active on this thread".into());
    }
    let mut failure = get(
        &tx,
        "turnItems",
        &format!("workspace-preparation-failure:{rid}"),
    )?;
    if failure["status"] != "failed" {
        return Err("Run has no failed workspace preparation to retry".into());
    }
    let at = iso(now_ms());
    failure["status"] = json!("cancelled");
    failure["updatedAt"] = json!(at);
    crate::worktree_setup::save_item(&tx, tid, &t, failure)?;
    let mut item = get(&tx, "turnItems", &format!("workspace-preparation:{rid}"))?;
    let fields = item
        .as_object_mut()
        .ok_or("Invalid workspace preparation item")?;
    fields.remove("output");
    fields.remove("exitCode");
    item["status"] = json!("running");
    item["title"] = json!("Preparing workspace");
    item["completedAt"] = Value::Null;
    item["updatedAt"] = json!(at);
    crate::worktree_setup::save_item(&tx, tid, &t, item)?;
    for (kind, key, event) in [
        ("attempts", "activeAttemptId", "attempt.updated"),
        ("nodes", "rootNodeId", "node.updated"),
    ] {
        let id = r[key].as_str().ok_or("Preparation ownership missing")?;
        let mut v = get(&tx, kind, id)?;
        v["status"] = json!("pending");
        v["completedAt"] = Value::Null;
        put(&tx, kind, id, tid, &v)?;
        append(&tx, tid, event, v, &t)?;
    }
    r["status"] = json!("preparing");
    r["completedAt"] = Value::Null;
    r["error"] = Value::Null;
    r["executionRequested"] = json!(false);
    put(&tx, "runs", rid, tid, &r)?;
    append(&tx, tid, "run.updated", r.clone(), &t)?;
    let body: String = tx
        .query_row(
            "SELECT request FROM thread_launches WHERE run_id=? ORDER BY rowid DESC LIMIT 1",
            [rid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut plan: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if !plan.is_object() {
        return Err("Invalid persisted preparation request".into());
    }
    plan["commandId"] = json!(cid);
    if r["workspacePreparation"]["type"] == "worktree"
        && t["worktreePath"].is_string()
        && t["branch"].is_string()
    {
        plan["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":t["worktreePath"],"branch":t["branch"]});
        plan["reusedWorktree"] = json!({"baseRef":r["workspacePreparation"]["baseRef"]});
    }
    plan["expectedWorktreePath"] = t["worktreePath"].clone();
    plan["expectedBranch"] = t["branch"].clone();
    enqueue(&tx, tid, Some(rid), cid, &plan)?;
    let latest:i64=tx.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),0) FROM records WHERE kind='runs' AND thread_id=? AND json_extract(body,'$.status')!='queued'",[tid],|r|r.get(0)).map_err(|e|e.to_string())?;
    if r["ordinal"] == latest {
        crate::deletion::visit(&tx, tid, "runs", |id, mut queued| {
            if queued["status"] == "queued" && queued["queueHeld"] == true {
                queued["queueHeld"] = json!(false);
                put(&tx, "runs", &id, tid, &queued)?;
                append(&tx, tid, "run.updated", queued, &t)?;
            }
            Ok(())
        })?;
    }
    let sequence: i64 = tx
        .query_row("SELECT coalesce(max(sequence),0) FROM events", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let result = json!({"sequence":sequence});
    tx.execute(
        "INSERT INTO receipts VALUES(?1,?2)",
        params![cid, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
    use std::{future::Future, pin::Pin};
    struct Peer(Mutex<Vec<String>>);
    impl Provider for Peer {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            out: tokio::sync::mpsc::Sender<ProviderEvent>,
            _controls: tokio::sync::mpsc::Receiver<ProviderControl>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.0.lock().unwrap().push(input.text);
                out.send(ProviderEvent::Started)
                    .await
                    .map_err(|e| e.to_string())?;
                out.send(ProviderEvent::Completed)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(())
            })
        }
    }
    fn read(s: &State, kind: &str, id: &str) -> Value {
        get(&s.db.lock().unwrap(), kind, id).unwrap()
    }
    async fn wait(mut f: impl FnMut() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            while !f() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    fn fixture() -> (Arc<State>, std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "o3-launch-recovery-{}",
            crate::store::random_id().unwrap()
        ));
        let s = Arc::new(State::open(&dir).unwrap());
        *s.provider.lock().unwrap() = Some(Arc::new(Peer(Mutex::new(vec![]))));
        events::mutate_project(&s,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":dir})).unwrap();
        let result=launch(&s,&json!({"commandId":"launch","threadId":"t","projectId":"p","title":"t","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"messageId":"m","text":"exact durable request","attachments":[]}})).unwrap();
        let rid = result["projection"]["runs"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        (s, dir, rid)
    }
    #[tokio::test]
    async fn restart_resumes_only_unstarted_preparation_and_explicit_retry_preserves_the_run() {
        for started in [false, true] {
            let (s, dir, rid) = fixture();
            if started {
                s.db.lock()
                    .unwrap()
                    .execute(
                        "UPDATE thread_launches SET phase='bound',bound=1,script_started=1",
                        [],
                    )
                    .unwrap();
            }
            drop(s);
            let s = Arc::new(State::open(&dir).unwrap());
            crate::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
            let peer = Arc::new(Peer(Mutex::new(vec![])));
            *s.provider.lock().unwrap() = Some(peer.clone());
            start(&s);
            wait(|| {
                matches!(
                    read(&s, "runs", &rid)["status"].as_str(),
                    Some("completed" | "failed")
                )
            })
            .await;
            if started {
                assert_eq!(read(&s, "runs", &rid)["status"], "failed");
                assert!(read(&s, "runs", &rid)["error"]
                    .as_str()
                    .unwrap()
                    .contains("retry explicitly"));
                assert!(peer.0.lock().unwrap().is_empty());
                retry(&s,&json!({"type":"prepared-run.retry","commandId":"retry","threadId":"t","runId":rid}),None).unwrap();
                wait(|| read(&s, "runs", &rid)["status"] == "completed").await;
            }
            assert_eq!(*peer.0.lock().unwrap(), vec!["exact durable request"]);
            assert_eq!(
                s.db.lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap(),
                1
            );
            assert_eq!(read(&s, "messages", "m")["text"], "exact durable request");
            wait(|| s.launches.active.lock().unwrap().is_empty()).await;
            drop(s);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn corrupt_persisted_preparation_fails_without_blocking_independent_launch() {
        for corrupt in ["broken json", "[]"] {
            let (s, dir, rid) = fixture();
            s.db.lock()
                .unwrap()
                .execute(
                    "UPDATE thread_launches SET request=?1,snapshot=?1",
                    [corrupt],
                )
                .unwrap();
            drop(s);
            let s = Arc::new(State::open(&dir).unwrap());
            crate::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
            let peer = Arc::new(Peer(Mutex::new(vec![])));
            *s.provider.lock().unwrap() = Some(peer.clone());
            start(&s);
            wait(|| read(&s, "runs", &rid)["status"] == "failed").await;
            assert!(peer.0.lock().unwrap().is_empty());
            assert!(retry(&s, &json!({"type":"prepared-run.retry","commandId":"bad-retry","threadId":"t","runId":rid}), None).is_err());
            let result=launch(&s,&json!({"commandId":"other","threadId":"other","projectId":"p","title":"other","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"messageId":"other","text":"independent request","attachments":[]}})).unwrap();
            let other = result["projection"]["runs"][0]["id"].as_str().unwrap();
            wait(|| read(&s, "runs", other)["status"] == "completed").await;
            assert_eq!(*peer.0.lock().unwrap(), vec!["independent request"]);
            wait(|| s.launches.active.lock().unwrap().is_empty()).await;
            drop(s);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
