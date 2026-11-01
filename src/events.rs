//! SQLite is authoritative. Transactions publish projections, log entries and receipts together.
use crate::store::{iso, now_ms, random_id, State};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, String>;
pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS records(kind TEXT NOT NULL,id TEXT NOT NULL,thread_id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id)); CREATE INDEX IF NOT EXISTS records_thread ON records(thread_id,kind); CREATE TABLE IF NOT EXISTS events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,thread_id TEXT NOT NULL,body TEXT NOT NULL,shell TEXT); CREATE TABLE IF NOT EXISTS receipts(command_id TEXT PRIMARY KEY,result TEXT NOT NULL);").map_err(|e|e.to_string())?;
    crate::threadq::initialize(db)
}
pub(crate) fn put(db: &Connection, kind: &str, id: &str, tid: &str, v: &Value) -> Result<()> {
    if kind == "messages" {
        crate::threadq::index_message(db, tid, v);
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
        new_thread(&tx, c, tid, &at)?
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
                if c["attachments"].as_array().is_none_or(|a| !a.is_empty()) {
                    return Err("steering attachments are unsupported".into());
                }
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
                let m = json!({"id":mid,"threadId":tid,"runId":rid,"nodeId":null,"role":"user","text":text,"attachments":[],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at});
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
                permit.send(crate::provider::ProviderControl::Steer(text.into()));
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
        "run.interrupt" => {
            let rid = c["runId"].as_str().ok_or("runId required")?;
            let mut run = get(&tx, "runs", rid)?;
            if run["threadId"] != tid {
                return Err("run belongs to another thread".into());
            }
            let permit = match s.active.lock().unwrap().get(rid).cloned() {
                Some(sender) => Some(
                    sender
                        .try_reserve_owned()
                        .map_err(|_| "provider control busy")?,
                ),
                None => None,
            };
            crate::projection::finish(&tx, tid, rid, &t, &at, "interrupted")?;
            let mid = format!("assistant-{rid}");
            if let Ok(mut message) = get(&tx, "messages", &mid) {
                message["streaming"] = json!(false);
                message["updatedAt"] = json!(at);
                put(&tx, "messages", &mid, tid, &message)?;
                append(&tx, tid, "message.updated", message.clone(), &t)?;
                let item = message_item(&tx, tid, &message)?;
                append(&tx, tid, "turn-item.updated", item, &t)?;
            }
            run["status"] = json!("interrupted");
            run["completedAt"] = json!(at);
            put(&tx, "runs", rid, tid, &run)?;
            let seq = append(&tx, tid, "run.updated", run, &t)?;
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
            if c["decision"] == "acceptAlways" && !request["nativeRequestRef"].is_null() {
                return Err("persistent native approvals are not implemented; choose accept or acceptForSession".into());
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
    let mut item = json!({"id":id,"threadId":tid,"runId":m["runId"],"nodeId":m["nodeId"],"providerThreadId":m["providerThreadId"],"providerTurnId":m["providerTurnId"],"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":null,"startedAt":m["createdAt"],"completedAt":m["updatedAt"],"updatedAt":m["updatedAt"],"type":if m["role"]=="user"{"user_message"}else{"assistant_message"},"messageId":m["id"],"text":m["text"],"streaming":m["streaming"],"attachments":m["attachments"]});
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
    let run_id = random_id()?;
    let mid = c["messageId"].as_str().ok_or("messageId required")?;
    let mut m = json!({"id":mid,"threadId":tid,"runId":run_id,"nodeId":null,"role":"user","text":c["text"],"attachments":c["attachments"],"streaming":false,"createdBy":"user","creationSource":"web","createdAt":at,"updatedAt":at});
    if let Some(context) = c.get("context") {
        m["context"] = context.clone();
    }
    if !c["text"].is_string() || !c["attachments"].is_array() {
        return Err("text and attachments required".into());
    }
    put(tx, "messages", mid, tid, &m)?;
    t["updatedAt"] = json!(at);
    put(tx, "thread", tid, tid, t)?;
    append(tx, tid, "message.updated", m.clone(), t)?;
    let item = message_item(tx, tid, &m)?;
    append(tx, tid, "turn-item.updated", item, t)?;
    let ordinal: i64 = tx
        .query_row(
            "SELECT count(*)+1 FROM records WHERE kind='runs' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut run = json!({"id":run_id,"threadId":tid,"ordinal":ordinal,"providerInstanceId":t["providerInstanceId"],"modelSelection":t["modelSelection"],"providerThreadId":null,"userMessageId":mid,"rootNodeId":null,"activeAttemptId":null,"status":"preparing","requestedAt":at,"startedAt":null,"completedAt":null,"checkpointId":null,"contextHandoffId":null,"executionRequested":c["dispatchMode"]["type"]!="defer_start"});
    if queued {
        run["status"] = json!("queued");
        run["queuePosition"] = json!(crate::thread_commands::next_queue_position(tx, tid)?);
        run["queueHeld"] = json!(false);
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
        json!({"id":tid,"projectId":c["projectId"],"title":c["title"],"providerInstanceId":c["modelSelection"]["instanceId"],"modelSelection":c["modelSelection"],"runtimeMode":c["runtimeMode"],"interactionMode":c["interactionMode"],"branch":c["branch"],"worktreePath":c["worktreePath"],"createdBy":"user","creationSource":c.get("creationSource").cloned().unwrap_or(json!("web")),"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":tid},"forkedFrom":null,"activeProviderThreadId":null,"createdAt":at,"updatedAt":at,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
    })
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
    if !reuse {
        let (path, branch) = crate::workspace::prepare(s, p, &tid)?;
        create["worktreePath"] = path;
        create["branch"] = branch;
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    // Recheck under the same lock as all writes, including concurrent retries.
    if let Some(result) = tx
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
    let at = iso(now_ms());
    let mut thread = if reuse {
        let existing = get(&tx, "thread", &tid)?;
        if existing["projectId"] != p["projectId"] {
            return Err("reuse project does not match thread".into());
        }
        existing
    } else {
        let thread = new_thread(&tx, &create, &tid, &at)?;
        put(&tx, "thread", &tid, &tid, &thread)?;
        append(&tx, &tid, "thread.created", thread.clone(), &thread)?;
        thread
    };
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
    s.notify();
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
