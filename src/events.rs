//! SQLite is authoritative. Transactions publish projections, log entries and receipts together.
use crate::store::{iso, now_ms, random_id, State};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, String>;
pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS records(kind TEXT NOT NULL,id TEXT NOT NULL,thread_id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE INDEX IF NOT EXISTS records_thread ON records(thread_id,kind); CREATE TABLE IF NOT EXISTS events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,thread_id TEXT NOT NULL,body TEXT NOT NULL,shell TEXT); CREATE TABLE IF NOT EXISTS receipts(command_id TEXT PRIMARY KEY,result TEXT NOT NULL);").map_err(|e|e.to_string())
}
pub(crate) fn put(db: &Connection, kind: &str, id: &str, tid: &str, v: &Value) -> Result<()> {
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
    serde_json::from_str(&s).map_err(|e| e.to_string())
}
fn list(db: &Connection, kind: &str, tid: Option<&str>, limit: usize) -> Result<Vec<Value>> {
    let mut q=db.prepare("SELECT body FROM records WHERE kind=? AND (?2 IS NULL OR thread_id=?2) ORDER BY rowid DESC LIMIT ?3").map_err(|e|e.to_string())?;
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
fn shell_thread(db: &Connection, t: &Value) -> Result<Value> {
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
    t["latestUserMessageAt"] = Value::Null;
    if let Some(run) = list(db, "runs", Some(tid), 1)?.pop() {
        t["latestRunId"] = run["id"].clone();
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
    t["hasActionableProposedPlan"] = json!(false);
    t["itemCount"] = json!(count);
    t["visibleItemCount"] = json!(count);
    Ok(t)
}
pub fn shell(s: &State) -> Result<Value> {
    let db = s.db.lock().unwrap();
    let mut active = Vec::new();
    let mut archive = Vec::new();
    for t in list(&db, "thread", None, 10000)? {
        if !t["deletedAt"].is_null() {
            continue;
        }
        let t = shell_thread(&db, &t)?;
        if t["archivedAt"].is_null() {
            active.push(t)
        } else {
            archive.push(t)
        }
    }
    Ok(
        json!({"schemaVersion":2,"snapshotSequence":sequence(&db)?,"projects":list(&db,"project",None,10000)?.into_iter().filter(|p|p["deletedAt"].is_null()).collect::<Vec<_>>(),"threads":active,"archivedThreads":archive}),
    )
}
pub fn projection(s: &State, tid: &str) -> Result<Value> {
    let db = s.db.lock().unwrap();
    projection_db(&db, tid)
}
fn projection_db(db: &Connection, tid: &str) -> Result<Value> {
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
pub fn bounded(s: &State, tid: &str) -> Result<Value> {
    let db = s.db.lock().unwrap();
    let p = projection_db(&db, tid)?;
    let (cursor, more) = history_boundary(&db, tid)?;
    Ok(
        json!({"snapshotSequence":sequence(&db)?,"projection":p,"historyCursor":cursor,"hasMoreHistory":more,"latestLocalTurnOrdinal":null}),
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
    let db = s.db.lock().unwrap();
    get(&db, "thread", tid)?;
    let mut q=db.prepare("SELECT rowid,body FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid<? ORDER BY rowid DESC LIMIT 101").map_err(|e|e.to_string())?;
    let mut rows = q.query(params![tid, anchor]).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    let mut next = None;
    let mut more = false;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if items.len() == 100 {
            more = true;
            break;
        }
        let id: i64 = row.get(0).map_err(|e| e.to_string())?;
        let body: String = row.get(1).map_err(|e| e.to_string())?;
        items.push(serde_json::from_str::<Value>(&body).map_err(|e| e.to_string())?);
        next = Some(id.to_string());
    }
    items.reverse();
    Ok(
        json!({"snapshotSequence":sequence(&db)?,"items":items,"nextCursor":if more{next}else{None},"hasMoreHistory":more}),
    )
}

pub fn next(s: &State, after: i64, tid: Option<&str>, shell: bool) -> Result<Option<(i64, Value)>> {
    let db = s.db.lock().unwrap();
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
    payload: Value,
    thread: &Value,
) -> Result<i64> {
    let event = json!({"id":random_id()?,"threadId":tid,"occurredAt":iso(now_ms()),"type":kind,"payload":payload});
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
        if get(&tx, "thread", tid).is_ok() {
            return Err("thread already exists".into());
        }
        json!({"id":tid,"projectId":c["projectId"],"title":c["title"],"providerInstanceId":c["modelSelection"]["instanceId"],"modelSelection":c["modelSelection"],"runtimeMode":c["runtimeMode"],"interactionMode":c["interactionMode"],"branch":c["branch"],"worktreePath":c["worktreePath"],"createdBy":"user","creationSource":c.get("creationSource").cloned().unwrap_or(json!("web")),"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":tid},"forkedFrom":null,"activeProviderThreadId":null,"createdAt":at,"updatedAt":at,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
    } else {
        get(&tx, "thread", tid)?
    };
    let event = match kind {
        "thread.create" => "thread.created",
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
            for k in ["title", "branch", "worktreePath", "limitRecovery"] {
                if let Some(v) = c.get(k) {
                    t[k] = v.clone()
                }
            }
            "thread.metadata-updated"
        }
        "message.dispatch" => {
            let mode = c["dispatchMode"]["type"].as_str();
            if !matches!(
                mode,
                Some("defer_start" | "start_immediately" | "queue_after_active")
            ) {
                return Err("dispatch mode is not implemented".into());
            }
            // A queued message waits behind the active run; with none active it starts at once.
            let queued = mode == Some("queue_after_active")
                && crate::thread_commands::has_blocking_run(&tx, tid)?;
            let launch = mode == Some("start_immediately")
                || (mode == Some("queue_after_active") && !queued);
            // Execution is deliberately refused until a provider is configured. Defer-start is durable.
            if launch && s.provider.lock().unwrap().is_none() {
                return Err("no provider adapter configured; only defer_start is available".into());
            }
            if s.active.lock().unwrap().len() >= 32 {
                return Err("active run limit reached".into());
            }
            let run_id = random_id()?;
            let mid = c["messageId"].as_str().ok_or("messageId required")?;
            let mut m = json!({"id":mid,"threadId":tid,"runId":run_id,"nodeId":null,"role":"user","text":c["text"],"attachments":c["attachments"],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at});
            if let Some(context) = c.get("context") {
                m["context"] = context.clone();
            }
            if !c["text"].is_string() || !c["attachments"].is_array() {
                return Err("text and attachments required".into());
            }
            put(&tx, "messages", mid, tid, &m)?;
            t["updatedAt"] = json!(at);
            put(&tx, "thread", tid, tid, &t)?;
            append(&tx, tid, "message.updated", m.clone(), &t)?;
            let item = message_item(&tx, tid, &m)?;
            append(&tx, tid, "turn-item.updated", item, &t)?;
            let ordinal: i64 = tx
                .query_row(
                    "SELECT count(*)+1 FROM records WHERE kind='runs' AND thread_id=?",
                    [tid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let run = json!({"id":run_id,"threadId":tid,"ordinal":ordinal,"providerInstanceId":t["providerInstanceId"],"modelSelection":t["modelSelection"],"providerThreadId":null,"userMessageId":mid,"rootNodeId":null,"activeAttemptId":null,"status":if queued {"queued"} else {"preparing"},"requestedAt":at,"startedAt":null,"completedAt":null,"checkpointId":null,"contextHandoffId":null});
            let mut run = run;
            if queued {
                run["queuePosition"] =
                    json!(crate::thread_commands::next_queue_position(&tx, tid)?);
                run["queueHeld"] = json!(false);
            }
            put(&tx, "runs", &run_id, tid, &run)?;
            let seq = append(&tx, tid, "run.created", run, &t)?;
            let result = json!({"sequence":seq});
            tx.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            s.notify();
            if launch {
                crate::provider::start(
                    s.clone(),
                    tid.to_owned(),
                    run_id,
                    c["text"].as_str().unwrap_or("").to_owned(),
                );
            }
            return Ok(result);
        }
        "run.interrupt" => {
            let rid = c["runId"].as_str().ok_or("runId required")?;
            let mut run = get(&tx, "runs", rid)?;
            if run["threadId"] != tid {
                return Err("run belongs to another thread".into());
            }
            run["status"] = json!("interrupted");
            run["completedAt"] = json!(at);
            put(&tx, "runs", rid, tid, &run)?;
            let seq = append(&tx, tid, "run.updated", run, &t)?;
            if c["holdQueue"] == true {
                crate::thread_commands::hold_queue(&tx, tid, &t)?;
            }
            let result = json!({"sequence":seq});
            tx.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            if let Some(control) = s.active.lock().unwrap().get(rid) {
                let _ = control.try_send(crate::provider::ProviderControl::Interrupt);
            }
            s.notify();
            return Ok(result);
        }
        "runtime-request.respond" | "thread.user-input.dismiss" => {
            let id = c["requestId"].as_str().ok_or("requestId required")?;
            let mut request = get(&tx, "runtimeRequests", id)?;
            let rid = request["nodeId"]
                .as_str()
                .ok_or("request node missing")?
                .to_owned();
            if get(&tx, "runs", &rid)?["threadId"] != tid {
                return Err("request belongs to another thread".into());
            }
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
            put(&tx, "runtimeRequests", id, tid, &request)?;
            let seq = append(&tx, tid, "runtime-request.updated", request, &t)?;
            let result = json!({"sequence":seq});
            tx.execute(
                "INSERT INTO receipts VALUES(?,?)",
                params![cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            drop(db);
            if let Some(control) = s.active.lock().unwrap().get(&rid) {
                let _ = control.try_send(crate::provider::ProviderControl::Respond {
                    request_id: id.into(),
                    decision: c["decision"].clone(),
                    feedback: c["feedback"].as_str().map(str::to_owned),
                    answers: c.get("answers").cloned(),
                });
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
    tx.execute(
        "INSERT INTO receipts VALUES(?,?)",
        params![cid, result.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(result)
}

pub(crate) fn message_item(db: &Connection, tid: &str, m: &Value) -> Result<Value> {
    let id = format!("item-{}", m["id"].as_str().ok_or("message id required")?);
    let ordinal: i64 = db
        .query_row(
            "SELECT count(*) FROM records WHERE kind='turnItems' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut item = json!({"id":id,"threadId":tid,"runId":m["runId"],"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":null,"startedAt":m["createdAt"],"completedAt":m["updatedAt"],"updatedAt":m["updatedAt"],"type":if m["role"]=="user"{"user_message"}else{"assistant_message"},"messageId":m["id"],"text":m["text"],"streaming":m["streaming"],"attachments":m["attachments"]});
    if m["role"] == "user" {
        item["createdBy"] = json!("user");
        item["creationSource"] = json!("web");
        item["inputIntent"] = json!("turn_start");
    }
    if let Ok(old) = get(db, "turnItems", &id) {
        item["ordinal"] = old["ordinal"].clone();
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

pub fn launch(s: &std::sync::Arc<State>, p: &Value) -> Result<Value> {
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
    if p["workspaceStrategy"]["type"] != "root" {
        return Err("only root workspace strategy is implemented".into());
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
    dispatch(s, &create)?;
    if let Some(m) = p.get("initialMessage") {
        let message = json!({"type":"message.dispatch","commandId":format!("{cid}:initial"),"threadId":tid,"messageId":m["messageId"].as_str().map(str::to_owned).unwrap_or(format!("message-{cid}")),"text":m["text"],"attachments":m["attachments"],"dispatchMode":{"type":"start_immediately"}});
        dispatch(s, &message)?;
    }
    let result = json!({"threadId":tid,"projection":projection(s,&tid)?,"resumed":false});
    s.db.lock()
        .unwrap()
        .execute(
            "INSERT INTO receipts VALUES(?,?)",
            params![cid, result.to_string()],
        )
        .map_err(|e| e.to_string())?;
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
            json!({"id":id,"title":c["title"],"workspaceRoot":c["workspaceRoot"],"defaultModelSelection":null,"scripts":[],"createdAt":at,"updatedAt":at,"deletedAt":null})
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
    ] {
        if let Some(v) = c.get(key) {
            p[key] = v.clone();
        }
    }
    p["updatedAt"] = json!(at);
    if c["type"] == "project.delete" {
        p["deletedAt"] = json!(at);
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
