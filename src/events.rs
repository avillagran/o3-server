//! SQLite is authoritative. Transactions publish projections, log entries and receipts together.
use crate::store::{iso, now_ms, random_id, State};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, String>;
pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS records(kind TEXT NOT NULL,id TEXT NOT NULL,thread_id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE INDEX IF NOT EXISTS records_thread ON records(thread_id,kind); CREATE TABLE IF NOT EXISTS events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,thread_id TEXT NOT NULL,body TEXT NOT NULL,shell TEXT); CREATE TABLE IF NOT EXISTS receipts(command_id TEXT PRIMARY KEY,result TEXT NOT NULL);").map_err(|e|e.to_string())?;
    // The partial index finds the latest user without reading assistant bodies.
    db.execute_batch("CREATE INDEX IF NOT EXISTS records_turn_ordinal ON records(thread_id,json_extract(body,'$.ordinal')) WHERE kind='turnItems'; CREATE INDEX IF NOT EXISTS records_user_message ON records(thread_id) WHERE kind='messages' AND json_extract(body,'$.role')='user';").map_err(|e|e.to_string())?;
    initialize_run_internal(db)?;
    crate::threadq::initialize(db)
}
// Durable execution bookkeeping is deliberately separate from wire records.
const INTERNAL_RUN_FIELDS: [&str; 3] = ["executionRequested", "recoveryPending", "recoveryReason"];
fn take_run_internal(v: &mut Value) -> Value {
    let mut internal = json!({});
    for key in INTERNAL_RUN_FIELDS {
        if let Some(value) = v.as_object_mut().and_then(|v| v.remove(key)) {
            internal[key] = value;
        }
    }
    internal
}
fn initialize_run_internal(db: &Connection) -> Result<()> {
    // An immediate transaction must precede every read used to decide a write.
    // Persist cursors with each bounded window so interrupted upgrades resume safely.
    let done: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='run_internal_migrated_v1')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if done {
        return Ok(());
    }
    db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS run_internal(id TEXT PRIMARY KEY,body TEXT NOT NULL); CREATE TABLE IF NOT EXISTS run_internal_migration_cursor(name TEXT PRIMARY KEY,position INTEGER NOT NULL); COMMIT;").map_err(|e| e.to_string())?;
    migrate_windows(db, "records", |db, lo, hi| {
        db.execute("INSERT INTO run_internal SELECT id,json_object('executionRequested',json(body -> '$.executionRequested'),'recoveryPending',json(body -> '$.recoveryPending'),'recoveryReason',json(body -> '$.recoveryReason')) FROM records WHERE rowid>?1 AND rowid<=?2 AND kind='runs' AND (json_type(body,'$.executionRequested') IS NOT NULL OR json_type(body,'$.recoveryPending') IS NOT NULL OR json_type(body,'$.recoveryReason') IS NOT NULL) ON CONFLICT(id) DO UPDATE SET body=json_patch(run_internal.body,excluded.body)", params![lo, hi]).map_err(|e| e.to_string())?;
        db.execute("UPDATE records SET body=json_remove(body,'$.executionRequested','$.recoveryPending','$.recoveryReason') WHERE rowid>?1 AND rowid<=?2 AND kind='runs' AND (json_type(body,'$.executionRequested') IS NOT NULL OR json_type(body,'$.recoveryPending') IS NOT NULL OR json_type(body,'$.recoveryReason') IS NOT NULL)", params![lo, hi]).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    migrate_windows(db, "events", |db, lo, hi| {
        db.execute("UPDATE events SET body=json_remove(body,'$.payload.executionRequested','$.payload.recoveryPending','$.payload.recoveryReason') WHERE rowid>?1 AND rowid<=?2 AND json_extract(body,'$.type') IN ('run.created','run.updated') AND (json_type(body,'$.payload.executionRequested') IS NOT NULL OR json_type(body,'$.payload.recoveryPending') IS NOT NULL OR json_type(body,'$.payload.recoveryReason') IS NOT NULL)", params![lo, hi]).map_err(|e| e.to_string())?;
        db.execute("UPDATE events SET body=json_set(body,'$.type','run-attempt.updated') WHERE rowid>?1 AND rowid<=?2 AND json_extract(body,'$.type')='attempt.updated'", params![lo, hi]).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    migrate_windows(db, "receipts", |db, lo, hi| {
        db.execute("UPDATE receipts SET result=json_set(result,'$.projection.runs',json(coalesce((SELECT json_group_array(json(json_remove(value,'$.executionRequested','$.recoveryPending','$.recoveryReason'))) FROM json_each(result,'$.projection.runs')),'[]'))) WHERE rowid>?1 AND rowid<=?2 AND json_type(result,'$.projection.runs')='array'", params![lo, hi]).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS run_internal_migrated_v1(done INTEGER); DROP TABLE IF EXISTS run_internal_migration_cursor; COMMIT;").map_err(|e| e.to_string())
}
fn migrate_windows(
    db: &Connection,
    table: &str,
    apply: impl Fn(&Connection, i64, i64) -> Result<()>,
) -> Result<()> {
    loop {
        #[cfg(test)]
        let started = std::time::Instant::now();
        db.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let step: Result<bool> = (|| {
            // Another opener may already have finished the migration.
            let done: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='run_internal_migrated_v1')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
            if done {
                return Ok(false);
            }
            let lo: i64 = db.query_row("SELECT coalesce((SELECT position FROM run_internal_migration_cursor WHERE name=?),0)", [table], |r| r.get(0)).map_err(|e| e.to_string())?;
            // Bound ordinary batches by bytes as well as rows. A single large
            // receipt is indivisible, so it gets its own transaction.
            let column = if table == "receipts" {
                "result"
            } else {
                "body"
            };
            let mut q = db.prepare(&format!("SELECT rowid,length(CAST({column} AS BLOB)) FROM {table} WHERE rowid>? ORDER BY rowid LIMIT 128")).map_err(|e| e.to_string())?;
            let mut rows = q.query([lo]).map_err(|e| e.to_string())?;
            let mut hi = None;
            let mut bytes = 0i64;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let size: i64 = row.get(1).map_err(|e| e.to_string())?;
                if hi.is_some() && bytes + size > 2 * 1024 * 1024 {
                    break;
                }
                hi = Some(row.get::<_, i64>(0).map_err(|e| e.to_string())?);
                bytes += size;
            }
            drop(rows);
            drop(q);
            let Some(hi) = hi else {
                return Ok(false);
            };
            apply(db, lo, hi)?;
            db.execute("INSERT INTO run_internal_migration_cursor VALUES(?,?) ON CONFLICT(name) DO UPDATE SET position=excluded.position", params![table,hi]).map_err(|e| e.to_string())?;
            Ok(true)
        })();
        db.execute_batch(if step.is_ok() { "COMMIT" } else { "ROLLBACK" })
            .map_err(|e| e.to_string())?;
        #[cfg(test)]
        MIGRATION_WINDOWS.with(|windows| windows.borrow_mut().push(started.elapsed()));
        if !step? {
            return Ok(());
        }
    }
}
#[cfg(test)]
thread_local! { static MIGRATION_WINDOWS: std::cell::RefCell<Vec<std::time::Duration>> = const { std::cell::RefCell::new(Vec::new()) }; }

pub(crate) fn put(db: &Connection, kind: &str, id: &str, tid: &str, v: &Value) -> Result<()> {
    let public = if kind == "runs" {
        let mut public = v.clone();
        let internal = take_run_internal(&mut public);
        db.execute("INSERT INTO run_internal VALUES(?,?) ON CONFLICT(id) DO UPDATE SET body=json_patch(run_internal.body,excluded.body)", params![id,internal.to_string()]).map_err(|e|e.to_string())?;
        Some(public)
    } else {
        None
    };
    let v = public.as_ref().unwrap_or(v);
    if kind == "messages" {
        crate::threadq::index_message(db, tid, v)?;
    }
    db.execute(
        "INSERT INTO records VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",
        params![kind, id, tid, v.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn get(db: &Connection, kind: &str, id: &str) -> Result<Value> {
    let s: String = db
        .query_row(
            "SELECT body FROM records WHERE kind=? AND id=?",
            params![kind, id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    if kind == "runs" {
        let internal: Option<String> = db
            .query_row("SELECT body FROM run_internal WHERE id=?", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(internal) = internal {
            let internal: Value = serde_json::from_str(&internal).map_err(|e| e.to_string())?;
            v.as_object_mut()
                .ok_or("invalid run")?
                .extend(internal.as_object().ok_or("invalid internal run")?.clone());
        }
    }
    Ok(v)
}
fn list(db: &Connection, kind: &str, tid: Option<&str>, limit: usize) -> Result<Vec<Value>> {
    // A nullable OR hides the (thread_id,kind,rowid) index from SQLite and
    // sorts every historical body into a temporary B-tree for each shell row.
    let sql = if tid.is_some() {
        "SELECT body FROM records WHERE kind=?1 AND thread_id=?2 ORDER BY rowid DESC LIMIT ?3"
    } else {
        "SELECT body FROM records WHERE kind=?1 AND ?2 IS NULL ORDER BY rowid DESC LIMIT ?3"
    };
    let mut q = db.prepare(sql).map_err(|e| e.to_string())?;
    let rows = q
        .query_map(params![kind, tid, limit], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut a = Vec::new();
    for row in rows {
        a.push(serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?)
    }
    a.reverse();
    Ok(a)
}
pub fn sequence(db: &Connection) -> Result<i64> {
    db.query_row("SELECT coalesce(max(sequence),0) FROM events", [], |r| {
        r.get(0)
    })
    .map_err(|e| e.to_string())
}
pub(crate) fn shell_thread(db: &Connection, t: &Value) -> Result<Value> {
    let tid = t["id"].as_str().ok_or("thread id missing")?;
    let mut t = t.clone();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM records WHERE kind='messages' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let m = list(db, "messages", Some(tid), 1)?.pop();
    t["status"] = json!("idle");
    t["latestRunId"] = Value::Null;
    t["activeRunId"] = Value::Null;
    t["pendingRuntimeRequest"] = Value::Null;
    t["latestVisibleMessage"] = m
        .map(|m| json!({"id":m["id"],"role":m["role"],"text":m["text"],"updatedAt":m["updatedAt"]}))
        .unwrap_or(Value::Null);
    let latest_user: Option<String> = db.query_row("SELECT json_extract(body,'$.createdAt') FROM records WHERE kind='messages' AND thread_id=? AND json_extract(body,'$.role')='user' ORDER BY rowid DESC LIMIT 1",[tid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    t["latestUserMessageAt"] = json!(latest_user);
    t["latestUserAuthoredMessageAt"] = json!(latest_user);
    for field in [
        "latestRunRequestedAt",
        "latestRunStartedAt",
        "latestRunCompletedAt",
        "activityRunStartedAt",
        "activityRunStatus",
        "lastError",
    ] {
        t[field] = Value::Null;
    }
    for key in [
        "snoozedAt",
        "snoozedUntil",
        "unsettledAt",
        "titleRegeneration",
        "usageLimitResetAt",
    ] {
        if t.get(key).is_none() {
            t[key] = Value::Null;
        }
    }
    let mut q=db.prepare("SELECT body FROM records r WHERE kind IN ('subagents','backgroundTasks') AND thread_id=? AND (kind='backgroundTasks' OR NOT EXISTS(SELECT 1 FROM records b WHERE b.kind='backgroundTasks' AND b.thread_id=r.thread_id AND json_extract(b.body,'$.itemId')=r.id)) AND coalesce(json_extract(body,'$.status'),'pending') NOT IN ('completed','cancelled','failed','interrupted') ORDER BY rowid LIMIT 1000").map_err(|e|e.to_string())?;
    let tasks = q
        .query_map([tid], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|body| body.map(|body| { let v:Value=serde_json::from_str(&body).unwrap_or(Value::Null); json!({"kind":v["kind"].as_str().unwrap_or("subagent"),"taskId":v["taskId"].as_str().or(v["id"].as_str()),"awaited":v["awaited"]==true}) }))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    t["pendingBackgroundTasks"] = json!(tasks);
    t["latestRunStopRequested"] = json!(false);
    if let Some(run) = list(db, "runs", Some(tid), 1)?.pop() {
        t["latestRunId"] = run["id"].clone();
        t["latestRunStopRequested"] = json!(db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE kind='turnItems' AND id=?)",
                params![format!(
                    "{}-interrupt-request",
                    run["id"].as_str().unwrap_or("")
                )],
                |r| r.get::<_, bool>(0)
            )
            .map_err(|e| e.to_string())?);
        t["latestRunRequestedAt"] = run["requestedAt"].clone();
        t["latestRunStartedAt"] = run["startedAt"].clone();
        t["latestRunCompletedAt"] = run["completedAt"].clone();
        t["activityRunStatus"] = run["status"].clone();
        t["activityRunStartedAt"] = run["startedAt"].clone();
        t["lastError"] = run["error"].clone();
        t["lastErrorClass"] = run["errorClass"].clone();
        t["status"] = run["status"].clone();
        if matches!(
            run["status"].as_str(),
            Some("preparing" | "starting" | "running" | "waiting")
        ) {
            t["activeRunId"] = run["id"].clone();
        }
    }
    if let Some(request) = list(db, "runtimeRequests", Some(tid), 1)?
        .pop()
        .filter(|r| r["status"] == "pending")
    {
        t["pendingRuntimeRequest"] =
            json!({"id":request["id"],"kind":request["kind"],"createdAt":request["createdAt"]});
    }
    t["hasActionableProposedPlan"]=json!(db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='plans' AND thread_id=? AND json_extract(body,'$.status')='active')",[tid],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?);
    t["itemCount"] = json!(count);
    t["visibleItemCount"] = json!(count);
    Ok(t)
}
pub fn shell(s: &State) -> Result<Value> {
    let db = s.reader.lock().unwrap();
    shell_db(&db)
}
pub(crate) fn shell_db(db: &Connection) -> Result<Value> {
    let mut active = Vec::new();
    let mut archive = Vec::new();
    for t in list(db, "thread", None, 10000)? {
        if !t["deletedAt"].is_null() {
            continue;
        }
        let t = shell_thread(db, &t)?;
        if t["archivedAt"].is_null() {
            active.push(t)
        } else {
            archive.push(t)
        }
    }
    Ok(
        json!({"schemaVersion":2,"snapshotSequence":sequence(db)?,"projects":list(db,"project",None,10000)?.into_iter().filter(|p|p["deletedAt"].is_null()).collect::<Vec<_>>(),"threads":active,"archivedThreads":archive}),
    )
}
pub fn projection(s: &State, tid: &str) -> Result<Value> {
    let db = s.reader.lock().unwrap();
    projection_db(&db, tid)
}
pub(crate) fn projection_db(db: &Connection, tid: &str) -> Result<Value> {
    let thread = get(db, "thread", tid)?;
    let mut p = json!({"updatedAt":thread["updatedAt"],"thread":thread});
    for kind in [
        "runs",
        "attempts",
        "nodes",
        "subagents",
        "providerSessions",
        "providerThreads",
        "providerTurns",
        "runtimeRequests",
        "messages",
        "plans",
        "turnItems",
        "checkpointScopes",
        "checkpoints",
        "contextHandoffs",
        "contextTransfers",
        "visibleTurnItems",
    ] {
        p[kind] = json!(list(db, kind, Some(tid), 100)?);
    }
    Ok(p)
}
pub(crate) fn latest_local_ordinal(db: &Connection, tid: &str) -> Result<Option<i64>> {
    db.query_row("SELECT max(json_extract(body,'$.ordinal')) FROM records WHERE kind='turnItems' AND thread_id=?", [tid], |r| r.get(0)).map_err(|e| e.to_string())
}
pub fn bounded(s: &State, tid: &str) -> Result<Value> {
    let db = s.reader.lock().unwrap();
    let p = projection_db(&db, tid)?;
    let (cursor, more) = history_boundary(&db, tid)?;
    Ok(
        json!({"snapshotSequence":sequence(&db)?,"projection":p,"historyCursor":cursor,"hasMoreHistory":more,"latestLocalTurnOrdinal":latest_local_ordinal(&db, tid)?}),
    )
}
fn history_boundary(db: &Connection, tid: &str) -> Result<(Option<String>, bool)> {
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM records WHERE kind='visibleTurnItems' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let anchor:Option<i64>=db.query_row("SELECT rowid FROM records WHERE kind='visibleTurnItems' AND thread_id=? ORDER BY rowid DESC LIMIT 1 OFFSET 99",[tid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    Ok((
        if count > 100 {
            anchor.map(|n| n.to_string())
        } else {
            None
        },
        count > 100,
    ))
}
pub fn history(s: &State, tid: &str, cursor: &str) -> Result<Value> {
    let anchor: i64 = cursor.parse().map_err(|_| "invalid history cursor")?;
    let db = s.reader.lock().unwrap();
    get(&db, "thread", tid)?;
    let (lower, more) = crate::snapshot::boundary(&db, tid, anchor)?;
    let mut q=db.prepare("SELECT body FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid<? AND rowid>=? ORDER BY rowid DESC").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![tid, anchor, lower], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for row in rows {
        items.push(
            serde_json::from_str::<Value>(&row.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
        );
    }
    let next = Some(lower.to_string());
    items.reverse();
    Ok(
        json!({"snapshotSequence":sequence(&db)?,"items":items,"nextCursor":if more{next}else{None},"hasMoreHistory":more}),
    )
}

pub fn next(s: &State, after: i64, tid: Option<&str>, shell: bool) -> Result<Option<(i64, Value)>> {
    let db = s.reader.lock().unwrap();
    let column = if shell { "shell" } else { "body" };
    let sql=format!("SELECT sequence,{column} FROM events WHERE sequence>? AND (?2 IS NULL OR thread_id=?2) AND {column} IS NOT NULL ORDER BY sequence LIMIT 1");
    let row: Option<(i64, String)> = db
        .query_row(&sql, params![after, tid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    row.map(|(n, b)| {
        serde_json::from_str(&b)
            .map(|v| (n, v))
            .map_err(|e| e.to_string())
    })
    .transpose()
}
pub(crate) fn append(
    db: &Connection,
    tid: &str,
    kind: &str,
    mut payload: Value,
    thread: &Value,
) -> Result<i64> {
    // Older internal producers used an alias the real client reducer ignores.
    let kind = if kind == "attempt.updated" {
        "run-attempt.updated"
    } else {
        kind
    };
    if matches!(kind, "run.created" | "run.updated") {
        take_run_internal(&mut payload);
    }
    let mut event = json!({"id":random_id()?,"threadId":tid,"occurredAt":iso(now_ms()),"type":kind,"payload":payload});
    for key in ["runId", "nodeId"] {
        if let Some(v) = payload.get(key).filter(|v| !v.is_null()) {
            event[key] = v.clone();
        }
    }
    if let Some(v) = thread.get("providerInstanceId") {
        event["providerInstanceId"] = v.clone();
    }
    db.execute(
        "INSERT INTO events(thread_id,body) VALUES(?,?)",
        params![tid, event.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let seq = db.last_insert_rowid();
    let location = if thread["archivedAt"].is_null() {
        "active"
    } else {
        "archive"
    };
    let shell = if !thread["deletedAt"].is_null() {
        json!({"kind":"thread.removed","sequence":seq,"location":location,"threadId":tid})
    } else {
        json!({"kind":"thread.updated","sequence":seq,"location":location,"thread":shell_thread(db,thread)?})
    };
    db.execute(
        "UPDATE events SET shell=? WHERE sequence=?",
        params![shell.to_string(), seq],
    )
    .map_err(|e| e.to_string())?;
    Ok(seq)
}
pub fn dispatch(s: &std::sync::Arc<State>, c: &Value) -> Result<Value> {
    let _worktree = s
        .worktree_gate
        .try_read()
        .map_err(|_| "worktree cleanup in progress; retry command")?;
    let cid = c["commandId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("commandId required")?;
    if c["type"]
        .as_str()
        .is_some_and(crate::thread_commands::handles)
    {
        return crate::thread_commands::dispatch(s, c);
    }
    if c["type"] == "thread.stop" {
        let target = {
            let db = s.db.lock().unwrap();
            list(&db, "runs", c["threadId"].as_str(), 10000)?
                .into_iter()
                .rev()
                .find(|r| {
                    matches!(
                        r["status"].as_str(),
                        Some("preparing" | "starting" | "running" | "waiting")
                    )
                })
        };
        if let Some(run) = target {
            let mut interrupt = c.clone();
            interrupt["type"] = json!("run.interrupt");
            interrupt["runId"] = run["id"].clone();
            interrupt["holdQueue"] = json!(true);
            if let Ok(result) = dispatch(s, &interrupt) {
                return Ok(result);
            }
        }
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if let Some(r) = tx
        .query_row(
            "SELECT result FROM receipts WHERE command_id=?",
            [cid],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return serde_json::from_str(&r).map_err(|e| e.to_string());
    }
    let kind = c["type"].as_str().ok_or("type required")?;
    let tid = c["threadId"].as_str().ok_or("threadId required")?;
    let at = iso(now_ms());
    let mut t = if kind == "thread.create" {
        new_thread(&tx, c, tid, &at)?
    } else {
        get(&tx, "thread", tid)?
    };
    let event = match kind {
        "thread.create" => {
            import_native(s, &tx, c, tid, &at, &mut t)?;
            "thread.created"
        }
        "thread.archive" => {
            t["archivedAt"] = json!(at);
            "thread.archived"
        }
        "thread.unarchive" => {
            t["archivedAt"] = Value::Null;
            "thread.unarchived"
        }
        "thread.delete" => {
            t["deletedAt"] = json!(at);
            "thread.deleted"
        }
        "thread.pin" => {
            t["pinnedAt"] = json!(at);
            "thread.pinned"
        }
        "thread.unpin" => {
            t["pinnedAt"] = Value::Null;
            "thread.unpinned"
        }
        "thread.settle" => {
            t["settledAt"] = json!(at);
            t["settledOverride"] = json!("settled");
            "thread.settled"
        }
        "thread.unsettle" => {
            t["settledAt"] = Value::Null;
            t["settledOverride"] = json!("active");
            "thread.unsettled"
        }
        "thread.visit" => {
            t["lastVisitedAt"] = json!(at);
            "thread.visited"
        }
        "thread.metadata.update" => {
            if c["regenerateTitle"] == true {
                let text: Option<String> = tx.query_row("SELECT json_extract(body,'$.text') FROM records WHERE kind='messages' AND thread_id=? AND json_extract(body,'$.role')='user' ORDER BY rowid LIMIT 1",[tid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
                let title = crate::title::fallback(text.as_deref().unwrap_or(""));
                if title.is_empty() {
                    return Err("send a message before generating a title".into());
                }
                t["title"] = json!(title);
                t["titleRegeneration"] = json!({"requestId":cid,"startedAt":at});
            }
            if c.get("title").is_some() {
                t["titleRegeneration"] = Value::Null;
            }
            for k in ["title", "branch", "worktreePath", "limitRecovery"] {
                if let Some(v) = c.get(k) {
                    t[k] = v.clone()
                }
            }
            "thread.metadata-updated"
        }
        "thread.title.regeneration.complete" => {
            if t["titleRegeneration"]["requestId"] == c["requestId"] && c["requestId"].is_string() {
                if let Some(title) = c["title"].as_str().filter(|s| !s.trim().is_empty()) {
                    t["title"] = json!(crate::title::fallback(title));
                }
                t["titleRegeneration"] = Value::Null;
            }
            "thread.metadata-updated"
        }
        "thread.model-selection.set"
        | "thread.runtime-mode.set"
        | "thread.interaction-mode.set" => {
            let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='runs' AND thread_id=? AND json_extract(body,'$.status') IN ('preparing','starting','running','waiting'))",[tid],|r|r.get(0)).map_err(|e|e.to_string())?;
            if active {
                return Err("change model or mode after the active run finishes".into());
            }
            let key = match kind {
                "thread.model-selection.set" => "modelSelection",
                "thread.runtime-mode.set" => "runtimeMode",
                _ => "interactionMode",
            };
            let value = c.get(key).ok_or("mode or model selection required")?;
            if key == "modelSelection" {
                if !value["instanceId"].is_string()
                    || !value["model"].as_str().is_some_and(|m| !m.is_empty())
                {
                    return Err("invalid model selection".into());
                }
                t["providerInstanceId"] = value["instanceId"].clone();
            } else if key == "runtimeMode"
                && !matches!(
                    value.as_str(),
                    Some("approval-required" | "auto-accept-edits" | "full-access")
                )
            {
                return Err("unsupported runtime mode".into());
            } else if key == "interactionMode"
                && !matches!(value.as_str(), Some("default" | "plan"))
            {
                return Err("unsupported interaction mode".into());
            }
            if t[key] != *value && matches!(key, "runtimeMode" | "modelSelection") {
                tx.execute(
                    "DELETE FROM records WHERE kind='approvalPolicies' AND thread_id=?",
                    [tid],
                )
                .map_err(|e| e.to_string())?;
            }
            t[key] = value.clone();
            match kind {
                "thread.model-selection.set" => "thread.model-selection-updated",
                "thread.runtime-mode.set" => "thread.runtime-mode-updated",
                _ => "thread.interaction-mode-updated",
            }
        }
        "message.dispatch" => {
            if c["dispatchMode"]["type"] == "steer_active" {
                let rid = c["dispatchMode"]["targetRunId"]
                    .as_str()
                    .ok_or("targetRunId required")?;
                let run = get(&tx, "runs", rid)?;
                if run["threadId"] != tid
                    || !matches!(run["status"].as_str(), Some("running" | "waiting"))
                {
                    return Err("target run is not active in this thread".into());
                }
                let text = c["text"].as_str().ok_or("text required")?;
                if !c["attachments"].is_array() {
                    return Err("attachments required".into());
                }
                let content = crate::provider_input::materialize(
                    &crate::assets::attachments_dir(s),
                    text,
                    &c["attachments"],
                    c.get("context"),
                    crate::provider_input::driver(s, &t["providerInstanceId"])? != "codex",
                )?;
                let sender = s
                    .active
                    .lock()
                    .unwrap()
                    .get(rid)
                    .cloned()
                    .ok_or("provider control unavailable")?;
                let permit = sender
                    .try_reserve_owned()
                    .map_err(|_| "provider control busy")?;
                let mid = c["messageId"].as_str().ok_or("messageId required")?;
                let m = json!({"id":mid,"threadId":tid,"runId":rid,"nodeId":rid,"role":"user","text":text,"attachments":c["attachments"],"context":c["context"],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at});
                put(&tx, "messages", mid, tid, &m)?;
                append(&tx, tid, "message.updated", m.clone(), &t)?;
                let mut item = message_item(&tx, tid, &m)?;
                item["inputIntent"] = json!("steer");
                put(&tx, "turnItems", item["id"].as_str().unwrap(), tid, &item)?;
                put(
                    &tx,
                    "visibleTurnItems",
                    item["id"].as_str().unwrap(),
                    tid,
                    &json!({"position":item["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":item["id"],"item":item}),
                )?;
                let seq = append(&tx, tid, "turn-item.updated", item, &t)?;
                let result = json!({"sequence":seq});
                crate::outbox::stage(&tx, c)?;
                tx.execute(
                    "INSERT INTO receipts VALUES(?,?)",
                    params![cid, result.to_string()],
                )
                .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
                drop(db);
                permit.send(crate::provider::ProviderControl::SteerContent(content));
                crate::outbox::handed_off(s, cid)?;
                s.notify();
                return Ok(result);
            }
            let (result, run_id, queued) = stage_message(s, &tx, c, &mut t, &at)?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            s.notify();
            if c["dispatchMode"]["type"] != "defer_start" && !queued {
                crate::provider::start(
                    s.clone(),
                    tid.to_owned(),
                    run_id,
                    c["text"].as_str().unwrap_or("").to_owned(),
                );
            }
            return Ok(result);
        }
        "thread.stop" => {
            if t["deletedAt"].is_null() {
                if let Some(run) = list(&tx, "runs", Some(tid), 10000)?
                    .into_iter()
                    .rev()
                    .find(|r| r["status"] != "queued")
                {
                    if run["rootNodeId"].is_string() && run["providerThreadId"].is_string() {
                        interrupt_request(&tx, tid, &run, &t, &at, c)?;
                    }
                }
                crate::thread_commands::hold_queue(&tx, tid, &t)?;
            }
            "thread.metadata-updated"
        }
        "run.interrupt" => {
            let rid = c["runId"].as_str().ok_or("runId required")?;
            let mut run =
                get(&tx, "runs", rid).map_err(|_| format!("Run {rid} is not interruptible."))?;
            if run["threadId"] != tid {
                return Err(format!("Run {rid} is not interruptible."));
            }
            let root = get(&tx, "nodes", run["rootNodeId"].as_str().unwrap_or("")).ok();
            let owner = get(
                &tx,
                "providerThreads",
                run["providerThreadId"].as_str().unwrap_or(""),
            )
            .ok();
            if root.is_none()
                || owner.is_none()
                || matches!(
                    run["status"].as_str(),
                    Some("completed" | "failed" | "cancelled" | "interrupted" | "queued")
                )
            {
                return Err(format!("Run {rid} is not interruptible."));
            }
            let native_started = root
                .as_ref()
                .is_some_and(|n| n["providerTurnId"].is_string());
            if !native_started
                && get(
                    &tx,
                    "attempts",
                    run["activeAttemptId"].as_str().unwrap_or(""),
                )
                .is_err()
            {
                return Err(format!("Run {rid} has no active attempt to interrupt."));
            }
            // Reserve even during initialization so a pre-start cancellation
            // reaches the adapter, but never send before the receipt commits.
            let permit = match s.active.lock().unwrap().get(rid).cloned() {
                Some(sender) => Some(
                    sender
                        .try_reserve_owned()
                        .map_err(|_| "provider control busy")?,
                ),
                None => None,
            };
            interrupt_request(&tx, tid, &run, &t, &at, c)?;
            if !native_started || permit.is_none() {
                crate::projection::finish(&tx, tid, rid, &t, &at, "interrupted")?;
                interrupt_result(
                    &tx,
                    tid,
                    &run,
                    &t,
                    &at,
                    "Run interrupted before provider start",
                    false,
                )?;
                run["status"] = json!("interrupted");
                run["completedAt"] = json!(at);
                put(&tx, "runs", rid, tid, &run)?;
                append(&tx, tid, "run.updated", run, &t)?;
            }
            let seq = sequence(&tx)?;
            if c["holdQueue"] == true {
                crate::thread_commands::hold_queue(&tx, tid, &t)?;
            }
            let result = json!({"sequence":seq});
            crate::outbox::stage(&tx, c)?;
            tx.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            if let Some(permit) = permit {
                permit.send(crate::provider::ProviderControl::Interrupt);
                crate::outbox::handed_off(s, cid)?;
                if native_started {
                    crate::provider::interrupt_deadline(s.clone(), tid.to_owned(), rid.to_owned());
                }
            }
            s.notify();
            return Ok(result);
        }
        "runtime-request.respond" | "thread.user-input.dismiss" => {
            let id = c["requestId"].as_str().ok_or("requestId required")?;
            let mut request = get(&tx, "runtimeRequests", id)?;
            let node_id = request["nodeId"]
                .as_str()
                .ok_or("request node missing")?
                .to_owned();
            let rid = get(&tx, "nodes", &node_id)
                .ok()
                .and_then(|n| n["runId"].as_str().map(str::to_owned))
                .unwrap_or_else(|| node_id.to_owned());
            if get(&tx, "runs", &rid)?["threadId"] != tid {
                return Err("request belongs to another thread".into());
            }
            if request["status"] != "pending" {
                return Err("runtime request is not pending".into());
            }
            if c["decision"] == "acceptAlways" && request["approvalPolicyKey"].is_null() {
                return Err("this request has no persistable operation scope".into());
            }
            if matches!(
                c["decision"].as_str(),
                Some("acceptAlways" | "acceptForSession")
            ) && !request["approvalPolicyKey"].is_null()
            {
                let key = request["approvalPolicyKey"].clone();
                let id = format!("approval-policy-{}", random_id()?);
                put(
                    &tx,
                    "approvalPolicies",
                    &id,
                    tid,
                    &json!({"id":id,"key":key,"providerInstanceId":t["providerInstanceId"],"worktreePath":t["worktreePath"],"projectId":t["projectId"],"createdAt":at}),
                )?;
            }
            let sender = s.active.lock().unwrap().get(&rid).cloned();
            let permit = match sender {
                Some(sender) => Some(
                    sender
                        .try_reserve_owned()
                        .map_err(|_| "provider control busy")?,
                ),
                None if kind == "thread.user-input.dismiss" => None,
                None => return Err("provider request is no longer live".into()),
            };
            request["status"] = json!(if kind == "thread.user-input.dismiss" {
                "cancelled"
            } else {
                "resolved"
            });
            request["resolvedAt"] = json!(at);
            for k in ["decision", "feedback", "answers"] {
                if let Some(v) = c.get(k) {
                    request[k] = v.clone()
                }
            }
            if let Ok(mut node) = get(&tx, "nodes", &node_id) {
                node["status"] = json!("completed");
                node["completedAt"] = json!(at);
                put(&tx, "nodes", &node_id, tid, &node)?;
                append(&tx, tid, "node.updated", node, &t)?;
                let item_id = format!("item-{id}");
                if let Ok(mut item) = get(&tx, "turnItems", &item_id) {
                    item["status"] = json!("completed");
                    item["completedAt"] = json!(at);
                    item["updatedAt"] = json!(at);
                    put(&tx, "turnItems", &item_id, tid, &item)?;
                    put(
                        &tx,
                        "visibleTurnItems",
                        &item_id,
                        tid,
                        &json!({"position":item["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":item_id,"item":item}),
                    )?;
                    append(&tx, tid, "turn-item.updated", item, &t)?;
                }
            }
            put(&tx, "runtimeRequests", id, tid, &request)?;
            if let Ok(mut item) = get(&tx, "turnItems", &format!("approval-{id}")) {
                item["status"] = json!("completed");
                item["completedAt"] = json!(at);
                item["updatedAt"] = json!(at);
                crate::projection::save_item(&tx, tid, &t, item)?;
            }
            if let Ok(mut run) = get(&tx, "runs", &rid) {
                if run["status"] == "waiting" {
                    run["status"] = json!("running");
                }
                put(&tx, "runs", &rid, tid, &run)?;
                append(&tx, tid, "run.updated", run, &t)?;
            }
            let seq = append(&tx, tid, "runtime-request.updated", request, &t)?;
            let result = json!({"sequence":seq});
            crate::outbox::stage(&tx, c)?;
            tx.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            if let Some(permit) = permit {
                permit.send(crate::provider::ProviderControl::Respond {
                    request_id: id.into(),
                    decision: if kind == "thread.user-input.dismiss" {
                        json!("cancel")
                    } else {
                        c["decision"].clone()
                    },
                    feedback: c["feedback"].as_str().map(str::to_owned),
                    answers: c.get("answers").cloned(),
                });
                crate::outbox::handed_off(s, cid)?;
            }
            s.notify();
            return Ok(result);
        }
        _ => return Err(format!("command {kind} is not implemented")),
    };
    t["updatedAt"] = json!(at);
    put(&tx, "thread", tid, tid, &t)?;
    let seq = append(&tx, tid, event, t.clone(), &t)?;
    let result = json!({"sequence":seq});
    crate::outbox::stage(&tx, c)?;
    tx.execute(
        "INSERT INTO receipts VALUES(?,?)",
        params![cid, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    if c["regenerateTitle"] == true {
        let text = {
            let db = s.db.lock().unwrap();
            db.query_row("SELECT json_extract(body,'$.text') FROM records WHERE kind='messages' AND thread_id=? AND json_extract(body,'$.role')='user' ORDER BY rowid LIMIT 1",[tid],|r|r.get::<_,String>(0)).unwrap_or_default()
        };
        crate::title::start(s.clone(), tid.into(), cid.into(), text);
    }
    Ok(result)
}

fn interrupt_request(
    db: &Connection,
    tid: &str,
    run: &Value,
    t: &Value,
    at: &str,
    c: &Value,
) -> Result<()> {
    let rid = run["id"].as_str().ok_or("run id required")?;
    let id = format!("{rid}-interrupt-request");
    if get(db, "turnItems", &id).is_ok() {
        return Ok(());
    }
    interrupt_signal(
        db,
        tid,
        run,
        t,
        at,
        false,
        c["reason"].as_str().unwrap_or("Interrupt requested"),
        c["type"] != "thread.stop",
    )
}
pub(crate) fn interrupt_result(
    db: &Connection,
    tid: &str,
    run: &Value,
    t: &Value,
    at: &str,
    message: &str,
    native: bool,
) -> Result<()> {
    interrupt_signal(db, tid, run, t, at, true, message, native)
}
#[allow(clippy::too_many_arguments)]
fn interrupt_signal(
    db: &Connection,
    tid: &str,
    run: &Value,
    t: &Value,
    at: &str,
    result: bool,
    message: &str,
    native: bool,
) -> Result<()> {
    let rid = run["id"].as_str().ok_or("run id required")?;
    let suffix = if result { "result" } else { "request" };
    let id = format!("{rid}-interrupt-{suffix}");
    if get(db, "turnItems", &id).is_ok() {
        return Ok(());
    }
    let ordinal: i64 = db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",[tid],|r|r.get(0)).map_err(|e|e.to_string())?;
    let turn = if native && !(result && run["codexProviderTurnId"].is_string()) {
        get(db, "nodes", run["rootNodeId"].as_str().unwrap_or(""))
            .ok()
            .map(|n| n["providerTurnId"].clone())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let item = json!({"id":id,"threadId":tid,"runId":rid,"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":turn,"nativeItemRef":null,"parentItemId":if result {json!(format!("{rid}-interrupt-request"))}else{Value::Null},"ordinal":ordinal,"status":if result {"interrupted"}else{"completed"},"title":if result {"Interrupted"}else{"Interrupt requested"},"startedAt":at,"completedAt":at,"updatedAt":at,"type":format!("run_interrupt_{suffix}"),"message":message});
    crate::projection::save_item(db, tid, t, item)
}
#[cfg(test)]
fn interrupt_items(
    db: &Connection,
    tid: &str,
    run: &Value,
    t: &Value,
    at: &str,
    c: &Value,
    native: bool,
) -> Result<()> {
    interrupt_request(db, tid, run, t, at, c)?;
    interrupt_result(
        db,
        tid,
        run,
        t,
        at,
        "Run interrupted before provider start",
        native,
    )
}

pub(crate) fn prepare_run_ownership(
    tx: &Connection,
    tid: &str,
    t: &mut Value,
    run: &mut Value,
    at: &str,
) -> Result<()> {
    let run_id = run["id"].as_str().ok_or("run id missing")?.to_owned();
    let ordinal = run["ordinal"].clone();
    if t["activeProviderThreadId"]
        .as_str()
        .is_none_or(|id| get(tx, "providerThreads", id).is_err())
    {
        let native = crate::projection::native_id(tx, t);
        let id = if let Some(id) = t["activeProviderThreadId"].as_str() {
            id.to_owned()
        } else {
            format!("provider-thread-{}", crate::store::random_id()?)
        };
        t["activeProviderThreadId"] = json!(id);
        let driver = if t["providerInstanceId"] == "codex" {
            "codex"
        } else {
            "claudeAgent"
        };
        let p = json!({"id":id,"appThreadId":tid,"driver":driver,"providerInstanceId":t["providerInstanceId"],"providerSessionId":id,"ownerNodeId":null,"nativeThreadRef":native.map(|native|json!({"driver":driver,"nativeId":native,"strength":"strong"})),"nativeConversationHeadRef":null,"status":"not_loaded","firstRunOrdinal":ordinal,"lastRunOrdinal":ordinal,"handoffIds":[],"forkedFrom":null,"contextUsage":null,"goal":null,"nativeMetadata":null,"pendingBackgroundTasks":[],"createdAt":at,"updatedAt":at});
        put(tx, "providerThreads", &id, tid, &p)?;
        put(tx, "thread", tid, tid, t)?;
        append(tx, tid, "provider-thread.updated", p, t)?;
        append(tx, tid, "thread.metadata-updated", t.clone(), t)?;
    }
    let aid = format!("attempt-{run_id}");
    run["activeAttemptId"] = json!(aid);
    run["rootNodeId"] = json!(run_id);
    run["providerThreadId"] = t["activeProviderThreadId"].clone();
    let attempt = json!({"id":aid,"runId":run_id,"attemptOrdinal":1,"providerInstanceId":t["providerInstanceId"],"providerThreadId":run["providerThreadId"],"reason":"initial","rootNodeId":run_id,"providerTurnId":null,"status":"pending","startedAt":null,"completedAt":null});
    put(tx, "attempts", &aid, tid, &attempt)?;
    append(tx, tid, "attempt.updated", attempt, t)?;
    let node = json!({"id":run_id,"threadId":tid,"runId":run_id,"parentNodeId":null,"rootNodeId":run_id,"kind":"root_turn","status":"pending","countsForRun":true,"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":null,"completedAt":null});
    put(tx, "nodes", &run_id, tid, &node)?;
    append(tx, tid, "node.updated", node, t)?;
    Ok(())
}

pub(crate) fn message_item(db: &Connection, tid: &str, m: &Value) -> Result<Value> {
    let id = format!("item-{}", m["id"].as_str().ok_or("message id required")?);
    let ordinal: i64 = db
        .query_row(
            "SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut item = json!({"id":id,"threadId":tid,"runId":m["runId"],"nodeId":m["nodeId"],"providerThreadId":m["providerThreadId"],"providerTurnId":m["providerTurnId"],"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":null,"startedAt":m["createdAt"],"completedAt":m["updatedAt"],"updatedAt":m["updatedAt"],"type":if m["role"]=="user"{"user_message"}else{"assistant_message"},"messageId":m["id"],"text":m["text"],"streaming":m["streaming"],"attachments":m["attachments"]});
    if m["role"] == "user" {
        item.as_object_mut().unwrap().remove("streaming");
        item["createdBy"] = json!("user");
        item["creationSource"] = json!("web");
        item["inputIntent"] = m.get("inputIntent").cloned().unwrap_or(json!("turn_start"));
    }
    if let Ok(old) = get(db, "turnItems", &id) {
        item["ordinal"] = old["ordinal"].clone();
        if let Some(intent) = old.get("inputIntent") {
            item["inputIntent"] = intent.clone();
        }
    }
    put(db, "turnItems", &id, tid, &item)?;
    put(
        db,
        "visibleTurnItems",
        &id,
        tid,
        &json!({"position":item["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":item}),
    )?;
    Ok(item)
}

fn stage_message(
    s: &std::sync::Arc<State>,
    tx: &Connection,
    c: &Value,
    t: &mut Value,
    at: &str,
) -> Result<(Value, String, bool)> {
    let tid = c["threadId"].as_str().ok_or("threadId required")?;
    let cid = c["commandId"].as_str().ok_or("commandId required")?;
    if !matches!(
        c["dispatchMode"]["type"].as_str(),
        Some("defer_start" | "start_immediately" | "queue_after_active" | "steer_active")
    ) {
        return Err("dispatch mode is not implemented".into());
    }
    let mode = c["dispatchMode"]["type"].as_str();
    if s.active.lock().unwrap().len() >= 32 {
        return Err("active run limit reached".into());
    }
    let mut active_run: Option<String> = tx.query_row(
        "SELECT id FROM records WHERE kind='runs' AND thread_id=? AND json_extract(body,'$.status') IN ('preparing','running','waiting','starting') ORDER BY rowid DESC LIMIT 1",
        [tid], |r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if active_run.is_none() {
        let active = s.active.lock().unwrap();
        active_run = active
            .keys()
            .find(|rid| get(tx, "runs", rid).is_ok_and(|r| r["threadId"] == tid))
            .cloned();
    }
    if c["dispatchMode"]["type"] == "start_immediately" && active_run.is_some() {
        return Err(
            "thread already has an active run; use queue_after_active or steer_active".into(),
        );
    }

    let queued = c["dispatchMode"]["type"] == "queue_after_active" && active_run.is_some();
    if mode != Some("defer_start") && s.provider.lock().unwrap().is_none() {
        return Err("no provider adapter configured; only defer_start is available".into());
    }
    if let Some(mode) = c.get("interactionMode") {
        if !matches!(mode.as_str(), Some("default" | "plan")) {
            return Err("unsupported interaction mode".into());
        }
        if active_run.is_some() {
            return Err("cannot change mode with active or queued work".into());
        }
        t["interactionMode"] = mode.clone();
        append(tx, tid, "thread.interaction-mode-updated", t.clone(), t)?;
    }
    if let Some(reference) = c.get("sourcePlanRef") {
        let source = reference["threadId"]
            .as_str()
            .ok_or("source plan thread missing")?;
        let id = reference["planId"]
            .as_str()
            .ok_or("source plan id missing")?;
        let mut plan = get(tx, "plans", id)?;
        let source_thread = get(tx, "thread", source)?;
        if plan["threadId"] != source
            || plan["status"] != "active"
            || source_thread["projectId"] != t["projectId"]
            || t["interactionMode"] != "default"
        {
            return Err("source plan is not eligible for implementation".into());
        }
        plan["status"] = json!("completed");
        put(tx, "plans", id, source, &plan)?;
        append(tx, source, "plan.updated", plan, &source_thread)?;
    }
    let run_id = random_id()?;
    let mid = c["messageId"].as_str().ok_or("messageId required")?;
    let mut m = json!({"id":mid,"threadId":tid,"runId":run_id,"nodeId":run_id,"role":"user","text":c["text"],"attachments":c["attachments"],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at});
    if let Some(reference) = c.get("sourcePlanRef") {
        m["sourcePlanRef"] = reference.clone();
    }
    if let Some(context) = c.get("context") {
        m["context"] = context.clone();
    }
    if !c["text"].is_string() || !c["attachments"].is_array() {
        return Err("text and attachments required".into());
    }
    put(tx, "messages", mid, tid, &m)?;
    t["updatedAt"] = json!(at);
    if t["lastVisitedAt"].is_null() {
        t["lastVisitedAt"] = json!(iso(now_ms().saturating_sub(1)));
        append(tx, tid, "thread.metadata-updated", t.clone(), t)?;
    }
    put(tx, "thread", tid, tid, t)?;
    append(tx, tid, "message.updated", m.clone(), t)?;
    let ordinal: i64 = tx
        .query_row(
            "SELECT count(*)+1 FROM records WHERE kind='runs' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut run = json!({"id":run_id,"threadId":tid,"ordinal":ordinal,"providerInstanceId":t["providerInstanceId"],"modelSelection":t["modelSelection"],"providerThreadId":null,"userMessageId":mid,"rootNodeId":null,"activeAttemptId":null,"status":"preparing","queuePosition":null,"requestedAt":at,"startedAt":null,"completedAt":null,"checkpointId":null,"contextHandoffId":null,"executionRequested":c["dispatchMode"]["type"]!="defer_start"});
    if queued {
        run["status"] = json!("queued");
        run["queuePosition"] = json!(crate::thread_commands::next_queue_position(tx, tid)?);
        run["queueHeld"] = json!(false);
    }
    if !queued {
        prepare_run_ownership(tx, tid, t, &mut run, at)?;
    }
    let mut item_message = m.clone();
    item_message["providerThreadId"] = run["providerThreadId"].clone();
    if queued {
        item_message["inputIntent"] = json!("queued_turn");
    }
    let item = message_item(tx, tid, &item_message)?;
    append(tx, tid, "turn-item.updated", item.clone(), t)?;
    if mode == Some("defer_start") {
        let mut preparation = item;
        let id = format!("workspace-preparation:{run_id}");
        let object = preparation.as_object_mut().unwrap();
        for field in [
            "messageId",
            "text",
            "attachments",
            "inputIntent",
            "createdBy",
            "creationSource",
            "streaming",
        ] {
            object.remove(field);
        }
        preparation["id"] = json!(id);
        preparation["ordinal"] = json!(preparation["ordinal"].as_i64().unwrap_or(0) + 1);
        preparation["type"] = json!("command_execution");
        preparation["status"] = json!("running");
        preparation["input"] = json!("Preparing workspace");
        preparation["title"] = json!("Preparing workspace");
        preparation["completedAt"] = Value::Null;
        put(tx, "turnItems", &id, tid, &preparation)?;
        put(
            tx,
            "visibleTurnItems",
            &id,
            tid,
            &json!({"position":preparation["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":preparation}),
        )?;
        append(tx, tid, "turn-item.updated", preparation, t)?;
    }
    put(tx, "runs", &run_id, tid, &run)?;
    let seq = append(tx, tid, "run.created", run, t)?;
    let result = json!({"sequence":seq});
    crate::outbox::stage(tx, c)?;
    tx.execute(
        "INSERT INTO receipts VALUES(?,?)",
        params![cid, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok((result, run_id, queued))
}

fn import_native(
    s: &State,
    db: &Connection,
    c: &Value,
    tid: &str,
    at: &str,
    thread: &mut Value,
) -> Result<()> {
    if let Some(imported) = c.get("importedNativeThread") {
        let native = imported["ref"]["nativeId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("nativeId required")?;
        let driver = imported["ref"]["driver"]
            .as_str()
            .ok_or("native driver required")?;
        let expected = crate::provider_input::driver(s, &thread["providerInstanceId"])?;
        if driver != expected || imported["ref"]["strength"] != "strong" {
            return Err("import requires matching strong native ref".into());
        }
        let id = format!("imported-{tid}");
        let record = json!({"id":id,"appThreadId":tid,"driver":driver,"providerInstanceId":thread["providerInstanceId"],"nativeThreadRef":imported["ref"],"status":"not_loaded","nativeMetadata":imported["metadata"],"pendingBackgroundTasks":[],"createdAt":at,"updatedAt":at});
        thread["activeProviderThreadId"] = json!(id);
        if driver == "codex" {
            thread["codexNativeThreadId"] = json!(native);
        }
        put(db, "providerThreads", &id, tid, &record)?;
        append(db, tid, "provider-thread.updated", record, thread)?;
    }
    Ok(())
}

fn new_thread(db: &Connection, c: &Value, tid: &str, at: &str) -> Result<Value> {
    Ok({
        for key in [
            "projectId",
            "title",
            "modelSelection",
            "runtimeMode",
            "interactionMode",
        ] {
            if c.get(key).is_none() {
                return Err(format!("{key} required"));
            }
        }
        if get(db, "thread", tid).is_ok() {
            return Err("thread already exists".into());
        }
        json!({"id":tid,"projectId":c["projectId"],"title":c["title"],"providerInstanceId":c["modelSelection"]["instanceId"],"modelSelection":c["modelSelection"],"runtimeMode":c["runtimeMode"],"interactionMode":c["interactionMode"],"branch":c["branch"],"worktreePath":c["worktreePath"],"createdBy":"user","creationSource":c.get("creationSource").cloned().unwrap_or(json!("web")),"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":tid},"forkedFrom":null,"activeProviderThreadId":null,"createdAt":at,"updatedAt":at,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"snoozedAt":null,"snoozedUntil":null,"pullRequests":[],"deletedAt":null})
    })
}

pub fn launch(s: &std::sync::Arc<State>, p: &Value) -> Result<Value> {
    let _worktree = s
        .worktree_gate
        .try_read()
        .map_err(|_| "worktree cleanup in progress; retry launch")?;
    let _launch = s.launch_lock.lock().unwrap();
    let cid = p["commandId"].as_str().ok_or("commandId required")?;
    if let Some(result) =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT result FROM receipts WHERE command_id=?",
                [cid],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
    {
        return serde_json::from_str(&result).map_err(|e| e.to_string());
    }
    let tid = p["threadId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(format!("thread-{cid}"));
    let mut create = p.clone();
    create["type"] = json!("thread.create");
    create["threadId"] = json!(tid);
    create["commandId"] = json!(format!("{cid}:create"));
    if p.get("initialMessage").is_some() && s.provider.lock().unwrap().is_none() {
        return Err("no provider adapter configured".into());
    }
    let reuse = p["reuseExistingThread"] == true;
    if reuse && p["threadId"].as_str().is_none() {
        return Err("threadId required for reuse".into());
    }
    // Validate the message before workspace preparation can create filesystem effects.
    if let Some(m) = p.get("initialMessage") {
        if !m["text"].is_string() || !m["attachments"].is_array() {
            return Err("text and attachments required".into());
        }
    }
    let mut prepared = crate::workspace::prepare(s, p, &tid)?;
    create["worktreePath"] = prepared.path.clone();
    create["branch"] = prepared.branch.clone();
    let result = (|| -> Result<_> {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let at = iso(now_ms());
        let mut thread = if reuse {
            let existing = get(&tx, "thread", &tid)?;
            if existing["projectId"] != p["projectId"] {
                return Err("reuse project does not match thread".into());
            }
            let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=? AND kind IN ('messages','runs'))", [&tid], |r| r.get(0)).map_err(|e| e.to_string())?;
            if occupied || !existing["archivedAt"].is_null() || !existing["deletedAt"].is_null() {
                return Err("only an empty active thread can be reused".into());
            }
            let mut existing = existing;
            existing["ownedWorktree"] = json!(
                existing["ownedWorktree"] == true && existing["worktreePath"] == prepared.path
            );
            existing["branch"] = prepared.branch.clone();
            existing["worktreePath"] = prepared.path.clone();
            existing["updatedAt"] = json!(at);
            put(&tx, "thread", &tid, &tid, &existing)?;
            append(&tx, &tid, "thread.updated", existing.clone(), &existing)?;
            existing
        } else {
            let thread = new_thread(&tx, &create, &tid, &at)?;
            put(&tx, "thread", &tid, &tid, &thread)?;
            append(&tx, &tid, "thread.created", thread.clone(), &thread)?;
            thread
        };
        if prepared.is_owned() {
            thread["ownedWorktree"] = json!(true);
        }
        if p.get("initialMessage").is_some() {
            thread["modelSelection"] = p["modelSelection"].clone();
            thread["providerInstanceId"] = p["modelSelection"]["instanceId"].clone();
        }
        if p["generateTitle"] == true {
            if let Some(text) = p["initialMessage"]["text"].as_str() {
                let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !title.is_empty() {
                    thread["title"] = json!(title.chars().take(80).collect::<String>());
                    thread["titleRegeneration"] = json!({"requestId":cid,"startedAt":at});
                }
            }
        }
        import_native(s, &tx, p, &tid, &at, &mut thread)?;
        put(&tx, "thread", &tid, &tid, &thread)?;
        append(
            &tx,
            &tid,
            "thread.metadata-updated",
            thread.clone(),
            &thread,
        )?;
        let mut start = None;
        if let Some(m) = p.get("initialMessage") {
            let message = json!({"type":"message.dispatch","commandId":format!("{cid}:initial"),"threadId":tid,"messageId":m["messageId"].as_str().map(str::to_owned).unwrap_or(format!("message-{cid}")),"text":m["text"],"context":m["context"],"attachments":m["attachments"],"dispatchMode":{"type":"start_immediately"}});
            let (_, rid, _) = stage_message(s, &tx, &message, &mut thread, &at)?;
            start = Some((rid, m["text"].as_str().unwrap().to_owned()));
        }
        let result = json!({"threadId":tid,"projection":projection_db(&tx,&tid)?,"resumed":reuse});
        tx.execute(
            "INSERT INTO receipts VALUES(?,?)",
            params![cid, result.to_string()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(db);
        Ok((result, start))
    })();
    let (result, start) = match result {
        Ok(value) => {
            prepared.commit();
            value
        }
        Err(error) => {
            return match prepared.rollback() {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!("{error}; worktree cleanup failed: {cleanup}")),
            }
        }
    };
    s.notify();
    if p["generateTitle"] == true {
        if let Some(text) = p["initialMessage"]["text"].as_str() {
            crate::title::start(s.clone(), tid.clone(), cid.into(), text.into());
        }
    }
    if let Some((rid, text)) = start {
        crate::provider::start(s.clone(), tid, rid, text);
    }
    Ok(result)
}

pub fn mutate_project(s: &State, c: &Value) -> Result<Value> {
    let cid = c["commandId"].as_str().ok_or("commandId required")?;
    let id = c["projectId"].as_str().ok_or("projectId required")?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if let Some(r) = tx
        .query_row(
            "SELECT result FROM receipts WHERE command_id=?",
            [cid],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return serde_json::from_str(&r).map_err(|e| e.to_string());
    }
    let at = iso(now_ms());
    let mut p = match c["type"].as_str() {
        Some("project.create") => {
            if !c["workspaceRoot"].as_str().is_some_and(|s| !s.is_empty())
                || !c["title"].as_str().is_some_and(|s| !s.is_empty())
            {
                return Err("workspaceRoot and title required".into());
            }
            if get(&tx, "project", id).is_ok() {
                return Err("project exists".into());
            }
            json!({"id":id,"title":c["title"],"workspaceRoot":c["workspaceRoot"],"defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"repositoryIdentity":null,"archivedAt":null,"pinnedAt":null,"scripts":[],"createdAt":at,"updatedAt":at,"deletedAt":null})
        }
        Some("project.update" | "project.delete") => get(&tx, "project", id)?,
        _ => return Err("invalid project mutation".into()),
    };
    for key in [
        "title",
        "workspaceRoot",
        "defaultModelSelection",
        "defaultThreadEnvMode",
        "autoPull",
        "faviconPath",
        "projectIcon",
        "scripts",
        "repositoryIdentity",
    ] {
        if let Some(v) = c.get(key) {
            p[key] = v.clone();
        }
    }
    p["updatedAt"] = json!(at);
    if c["type"] == "project.delete" {
        p["deletedAt"] = json!(at);
    }
    if c["type"] == "project.delete" && c["force"] == true {
        for mut t in list(&tx, "thread", None, i64::MAX as usize)? {
            if t["projectId"] == id && t["deletedAt"].is_null() {
                t["deletedAt"] = json!(at);
                t["updatedAt"] = json!(at);
                let tid = t["id"].as_str().ok_or("thread id missing")?.to_owned();
                put(&tx, "thread", &tid, &tid, &t)?;
                append(&tx, &tid, "thread.deleted", t.clone(), &t)?;
            }
        }
    }
    put(&tx, "project", id, "", &p)?;
    let body = json!({"id":random_id()?,"type":c["type"],"payload":p,"occurredAt":at});
    tx.execute(
        "INSERT INTO events(thread_id,body) VALUES('',?)",
        [body.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let seq = tx.last_insert_rowid();
    let shell = if c["type"] == "project.delete" {
        json!({"kind":"project.removed","sequence":seq,"projectId":id})
    } else {
        json!({"kind":"project.updated","sequence":seq,"project":p})
    };
    tx.execute(
        "UPDATE events SET shell=? WHERE sequence=?",
        params![shell.to_string(), seq],
    )
    .map_err(|e| e.to_string())?;
    crate::outbox::stage(&tx, c)?;
    tx.execute(
        "INSERT INTO receipts VALUES(?,?)",
        params![cid, p.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(p)
}

#[cfg(test)]
mod internal_run_tests {
    use super::*;
    #[test]
    fn ordinal_allocation_uses_expression_index() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let plan: String = db.query_row("EXPLAIN QUERY PLAN SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?", ["t"], |r| r.get(3)).unwrap();
        assert!(plan.contains("records_turn_ordinal"), "{plan}");
    }
    #[test]
    fn sparse_imported_ordinals_leave_room_for_messages_and_interrupts() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        put(
            &db,
            "turnItems",
            "imported",
            "t",
            &json!({"id":"imported","ordinal":900}),
        )
        .unwrap();
        let m = json!({"id":"m","role":"user"});
        assert_eq!(message_item(&db, "t", &m).unwrap()["ordinal"], 901);
        interrupt_items(
            &db,
            "t",
            &json!({"id":"r"}),
            &json!({"id":"t"}),
            "now",
            &json!({}),
            false,
        )
        .unwrap();
        assert_eq!(
            get(&db, "turnItems", "r-interrupt-request").unwrap()["ordinal"],
            902
        );
        assert_eq!(
            get(&db, "turnItems", "r-interrupt-result").unwrap()["ordinal"],
            903
        );
    }
    #[test]
    fn migration_avoids_deterministic_wal_snapshot_upgrade_and_skips_history() {
        let dir = std::env::temp_dir().join(random_id().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        let a = Connection::open(&path).unwrap();
        a.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        initialize(&a).unwrap();
        let b = Connection::open(&path).unwrap();
        a.execute_batch("SAVEPOINT legacy; CREATE TABLE IF NOT EXISTS run_internal(id TEXT PRIMARY KEY,body TEXT NOT NULL)").unwrap();
        b.execute("INSERT INTO receipts VALUES('concurrent','{}')", [])
            .unwrap();
        let err = a.execute("UPDATE events SET body=body", []).unwrap_err();
        assert_eq!(
            err.sqlite_error().unwrap().extended_code,
            rusqlite::ffi::SQLITE_BUSY_SNAPSHOT
        );
        a.execute_batch("ROLLBACK TO legacy; RELEASE legacy")
            .unwrap();
        // A migrated opener never touches historical rows, even while another writer owns WAL.
        b.execute_batch("BEGIN IMMEDIATE").unwrap();
        initialize_run_internal(&a).unwrap();
        b.execute_batch("ROLLBACK").unwrap();
        // Legacy migration takes the write lock before reading: a concurrent commit
        // cannot invalidate its snapshot. It also preserves active internal state.
        a.execute_batch("DROP TABLE run_internal_migrated_v1")
            .unwrap();
        initialize_run_internal(&a).unwrap();
        drop((a, b));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn legacy_run_bookkeeping_migrates_without_entering_any_public_record() {
        let mut db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let run = json!({"id":"r","threadId":"t","status":"preparing","executionRequested":true,"recoveryPending":true,"recoveryReason":"restart"});
        let thread = json!({"id":"t","providerInstanceId":"fixture"});
        put(&db, "thread", "t", "t", &thread).unwrap();
        db.execute(
            "INSERT INTO records VALUES('runs','r','t',?)",
            [run.to_string()],
        )
        .unwrap();
        let event = json!({"type":"run.created","payload":run});
        db.execute(
            "INSERT INTO events(thread_id,body) VALUES('t',?)",
            [event.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO receipts VALUES('launch',?)",
            [json!({"projection":{"runs":[run]}}).to_string()],
        )
        .unwrap();
        db.execute_batch("DROP TABLE run_internal_migrated_v1")
            .unwrap();
        for _ in 0..2 {
            initialize(&db).unwrap();
        }
        assert_eq!(get(&db, "runs", "r").unwrap()["executionRequested"], true);
        // An existing preparing execution still fails visibly after restart.
        crate::recovery::recover(&mut db).unwrap();
        assert_eq!(get(&db, "runs", "r").unwrap()["status"], "failed");
        let mut updated = get(&db, "runs", "r").unwrap();
        updated["recoveryPending"] = json!(false);
        put(&db, "runs", "r", "t", &updated).unwrap();
        append(&db, "t", "run.updated", updated, &thread).unwrap();
        initialize(&db).unwrap();
        assert_eq!(get(&db, "runs", "r").unwrap()["recoveryPending"], false);
        let seq = append(
            &db,
            "t",
            "attempt.updated",
            json!({"id":"a","status":"completed"}),
            &thread,
        )
        .unwrap();
        let body: String = db
            .query_row("SELECT body FROM events WHERE sequence=?", [seq], |r| {
                r.get(0)
            })
            .unwrap();
        let event: Value = serde_json::from_str(&body).unwrap();
        let mut mirror = json!({"thread":{"id":"t"},"attempts":[]});
        assert!(t3_protocol::projection::apply(&mut mirror, &event));
        assert_eq!(mirror["attempts"][0]["status"], "completed");
        let projection = projection_db(&db, "t").unwrap();
        let receipt: String = db
            .query_row("SELECT result FROM receipts", [], |r| r.get(0))
            .unwrap();
        let mut q = db
            .prepare("SELECT body FROM records WHERE kind='runs' UNION ALL SELECT body FROM events")
            .unwrap();
        let bodies = q
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(std::result::Result::unwrap)
            .chain([projection.to_string(), receipt]);
        for body in bodies {
            for field in INTERNAL_RUN_FIELDS {
                assert!(!body.contains(field), "leaked {field}");
            }
        }
    }
}

#[cfg(test)]
mod migration_benchmark {
    use super::*;
    #[test]
    #[ignore = "synthetic 400-thread large-profile writer lock measurement"]
    fn large_profile_writer_windows() {
        let root = std::path::Path::new("/var/tmp/o3-build/migration-profile");
        std::fs::create_dir_all(root).unwrap();
        let path = root.join(format!("{}.sqlite", random_id().unwrap()));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .unwrap();
        initialize(&db).unwrap();
        db.execute_batch("DROP TABLE run_internal_migrated_v1; BEGIN IMMEDIATE;")
            .unwrap();
        let history = "x".repeat(4096);
        let cached = "x".repeat(1024 * 1024);
        for thread in 0..400 {
            let tid = format!("t-{thread}");
            db.execute(
                "INSERT INTO records VALUES('runs',?, ?, ?)",
                params![
                    tid,
                    tid,
                    json!({"id":tid,"executionRequested":true}).to_string()
                ],
            )
            .unwrap();
            for _ in 0..100 {
                db.execute("INSERT INTO events(thread_id,body) VALUES(?,?)",params![tid,json!({"type":"run.updated","payload":{"executionRequested":true,"history":history}}).to_string()]).unwrap();
            }
            db.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![
                    tid,
                    json!({"projection":{"runs":[{"executionRequested":true}],"history":cached}})
                        .to_string()
                ],
            )
            .unwrap();
        }
        db.execute_batch("COMMIT").unwrap();
        MIGRATION_WINDOWS.with(|v| v.borrow_mut().clear());
        let start = std::time::Instant::now();
        initialize_run_internal(&db).unwrap();
        let total = start.elapsed();
        MIGRATION_WINDOWS.with(|v| {
            let v = v.borrow();
            eprintln!("migration: threads=400 events=40000 eventPayloadBytes=4096 receipts=400 receiptPayloadBytes=1048576 windows={} maxWriterMs={:.3} totalMs={:.3}",v.len(),v.iter().max().unwrap().as_secs_f64()*1000.,total.as_secs_f64()*1000.);
        });
        assert_eq!(
            db.query_row("SELECT count(*) FROM run_internal", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            400
        );
        let start = std::time::Instant::now();
        initialize_run_internal(&db).unwrap();
        eprintln!(
            "completed migration reopenMs={:.3}",
            start.elapsed().as_secs_f64() * 1000.
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
