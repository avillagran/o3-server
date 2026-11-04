//! Receipt-atomic thread deletion and durable, retryable resource disposal.
//! Adapted from nightly's ThreadDeletion/ResourceCleanupService (MIT).
use crate::{
    events::{append, get, put},
    store::{now_ms, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS deletion_effects(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,payload TEXT NOT NULL,completed_at INTEGER,attempts INTEGER NOT NULL DEFAULT 0,retry_at INTEGER NOT NULL DEFAULT 0,last_error TEXT); CREATE INDEX IF NOT EXISTS deletion_thread ON deletion_effects(thread_id); CREATE INDEX IF NOT EXISTS deletion_pending ON deletion_effects(retry_at) WHERE completed_at IS NULL; CREATE TABLE IF NOT EXISTS deletion_attachments(effect_id TEXT NOT NULL,attachment_id TEXT NOT NULL,PRIMARY KEY(effect_id,attachment_id));").map_err(|e|e.to_string())
}
fn visit(
    db: &Connection,
    tid: &str,
    kind: &str,
    mut f: impl FnMut(String, Value) -> Result<()>,
) -> Result<()> {
    let mut cursor = 0i64;
    loop {
        let rows = {
            let mut q=db.prepare("SELECT rowid,id,body FROM records WHERE thread_id=?1 AND kind=?2 AND rowid>?3 ORDER BY rowid LIMIT 16").map_err(|e|e.to_string())?;
            let rows = q
                .query_map(params![tid, kind, cursor], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if rows.is_empty() {
            break;
        }
        for (row, id, body) in rows {
            cursor = row;
            f(id, serde_json::from_str(&body).map_err(|e| e.to_string())?)?;
        }
    }
    Ok(())
}
fn active(v: &Value) -> bool {
    matches!(
        v["status"].as_str(),
        Some("pending" | "preparing" | "queued" | "starting" | "running" | "waiting")
    )
}
pub(crate) fn plan(db: &Connection, thread: &mut Value, command: &str, at: &str) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?.to_owned();
    let material = serde_json::to_vec(&(command, &tid)).map_err(|e| e.to_string())?;
    let effect = format!("deletion:{:x}", Sha256::digest(material));
    let payload = json!({"threadKey":format!("{}:{}",tid,thread["providerGeneration"].as_str().unwrap_or("")),"providerInstanceId":thread["providerInstanceId"],"nativeThreadId":crate::projection::native_id(db,thread)});
    // Preserve the first tombstone time and cancel work before emitting any new shell summary.
    if thread["deletedAt"].is_null() {
        thread["deletedAt"] = json!(at);
    }
    thread["titleRegeneration"] = Value::Null;
    thread["updatedAt"] = json!(at);
    put(db, "thread", &tid, &tid, thread)?;
    append(db, &tid, "thread.deleted", thread.clone(), thread)?;
    visit(db, &tid, "runs", |id, _| {
        let mut run = get(db, "runs", &id)?;
        if active(&run) {
            run["status"] = json!("cancelled");
            run["queuePosition"] = Value::Null;
            run["completedAt"] = json!(at);
            run["recoveryPending"] = json!(false);
            run["executionRequested"] = json!(false);
            put(db, "runs", &id, &tid, &run)?;
            append(db, &tid, "run.updated", run.clone(), thread)?;
            crate::projection::finish(db, &tid, &id, thread, at, "cancelled")?;
        }
        let tasks:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='subagents' AND thread_id=?1 AND json_extract(body,'$.runId')=?2 AND json_extract(body,'$.origin')='app_owned')",params![tid,id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if run.get("delegatedCompletion").is_some() || tasks {
            run["delegatedCompletion"] = json!({"disposition":"disposed","nextGeneration":run["delegatedCompletion"]["nextGeneration"].as_u64().unwrap_or(1),"delivery":null});
            put(db, "runs", &id, &tid, &run)?;
            append(db, &tid, "run.updated", run, thread)?;
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
        visit(db, &tid, kind, |id, mut v| {
            let mut changed = false;
            if active(&v) {
                v["status"] = json!("cancelled");
                v["completedAt"] = json!(at);
                v["updatedAt"] = json!(at);
                changed = true;
            }
            if kind == "subagents"
                && v["origin"] == "app_owned"
                && !matches!(
                    v["completionDelivery"]["state"].as_str(),
                    Some("acknowledged" | "delivered" | "disposed")
                )
            {
                v["completionDelivery"] = json!({"state":"disposed","observedByRunId":null});
                v["updatedAt"] = json!(at);
                changed = true;
            }
            if changed {
                put(db, kind, &id, &tid, &v)?;
                append(db, &tid, event, v, thread)?;
            }
            Ok(())
        })?;
    }
    visit(db, &tid, "runtimeRequests", |id, mut v| {
        if v["status"] == "pending" {
            v["status"] = json!("cancelled");
            v["responseCapability"] =
                json!({"type":"not_resumable","reason":"The thread was deleted."});
            v["resolvedAt"] = json!(at);
            put(db, "runtimeRequests", &id, &tid, &v)?;
            append(db, &tid, "runtime-request.updated", v, thread)?;
        }
        Ok(())
    })?;
    visit(db, &tid, "providerSessions", |id, mut v| {
        if !matches!(v["status"].as_str(), Some("stopped" | "error")) {
            v["status"] = json!("stopped");
            v["detachedAt"] = json!(at);
            put(db, "providerSessions", &id, &tid, &v)?;
            let seq = append(
                db,
                &tid,
                "provider-session.detached",
                json!({"providerSessionId":id,"detachedAt":at,"reason":"Thread deleted."}),
                thread,
            )?;
            if let Some(driver) = v["driver"].as_str() {
                db.execute(
                    "UPDATE events SET body=json_set(body,'$.driver',?1) WHERE sequence=?2",
                    params![driver, seq],
                )
                .map_err(|e| e.to_string())?;
            }
            if let Some(instance) = v["providerInstanceId"].as_str() {
                db.execute("UPDATE events SET body=json_set(body,'$.providerInstanceId',?1) WHERE sequence=?2",params![instance,seq]).map_err(|e|e.to_string())?;
            }
        }
        Ok(())
    })?;
    visit(db, &tid, "messages", |id, mut v| {
        if v["streaming"] == true {
            v["streaming"] = json!(false);
            v["updatedAt"] = json!(at);
            put(db, "messages", &id, &tid, &v)?;
            append(db, &tid, "message.updated", v.clone(), thread)?;
            let item = crate::events::message_item(db, &tid, &v)?;
            append(db, &tid, "turn-item.updated", item, thread)?;
        }
        Ok(())
    })?;
    db.execute("UPDATE records SET body=json_set(body,'$.status','cancelled','$.error','The thread was deleted.') WHERE kind='controlOutbox' AND thread_id=? AND json_extract(body,'$.status') IN ('pending','handed-off','delivery-unknown')",[&tid]).map_err(|e|e.to_string())?;
    db.execute(
        "INSERT OR IGNORE INTO deletion_effects(id,thread_id,payload) VALUES(?1,?2,?3)",
        params![effect, tid, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    db.execute("INSERT OR IGNORE INTO deletion_attachments SELECT ?1,CASE WHEN a.type='object' THEN coalesce(json_extract(a.value,'$.id'),json_extract(a.value,'$.attachmentId')) ELSE NULL END FROM records r,json_each(r.body,'$.attachments') a WHERE r.thread_id=?2 AND r.kind='messages' AND a.type='object' AND coalesce(json_extract(a.value,'$.id'),json_extract(a.value,'$.attachmentId')) IS NOT NULL",params![effect,tid]).map_err(|e|e.to_string())?;
    Ok(())
}
/// Recover old tombstones whose pre-outbox implementation never cancelled their work.
pub(crate) fn recover(db: &Connection) -> Result<()> {
    let mut cursor = 0i64;
    loop {
        let row=db.query_row("SELECT rowid,id,body FROM records r WHERE kind='thread' AND rowid>?1 AND json_extract(body,'$.deletedAt') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM deletion_effects e WHERE e.thread_id=r.id) ORDER BY rowid LIMIT 1",[cursor],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional().map_err(|e|e.to_string())?;
        let Some((row, id, body)) = row else {
            break;
        };
        cursor = row;
        let mut t: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        plan(
            db,
            &mut t,
            &format!("recovered:{id}"),
            &crate::store::iso(now_ms()),
        )?;
    }
    Ok(())
}
pub(crate) fn signal(state: &State, tid: &str) {
    state.native_mcp.revoke_thread(tid);
    state.executions.cancel(tid);
}
pub(crate) fn start(state: &Arc<State>) {
    if state.deletion_started.swap(true, Ordering::SeqCst) {
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
                db.query_row("SELECT id,thread_id,payload,attempts FROM deletion_effects WHERE completed_at IS NULL AND retry_at<=? ORDER BY rowid LIMIT 1",[now_ms()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,u32>(3)?))).optional()
            };
            match job {
                Ok(Some((id, tid, payload, attempts))) => {
                    let outcome = async {
                        let payload: Value =
                            serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                        dispose(&state, &id, &tid, &payload).await
                    };
                    let result = tokio::time::timeout(Duration::from_secs(45), outcome)
                        .await
                        .unwrap_or_else(|_| Err("Thread disposal timed out; will retry.".into()));
                    let db = state.db.lock().unwrap();
                    let result=match result{
                        Ok(())=>db.execute("UPDATE deletion_effects SET completed_at=?1,last_error=NULL WHERE id=?2",params![now_ms(),id]),
                        Err(error)=>db.execute("UPDATE deletion_effects SET attempts=attempts+1,retry_at=?1,last_error=?2 WHERE id=?3",params![now_ms()+1000i64.saturating_mul(1i64<<attempts.min(5)),error.chars().take(1000).collect::<String>(),id]),
                    };
                    if let Err(error) = result {
                        eprintln!("Unable to journal thread disposal: {error}");
                        return;
                    }
                    continue;
                }
                Err(error) => {
                    eprintln!("Unable to read thread disposal outbox: {error}");
                    return;
                }
                Ok(None) => {}
            }
            let retry: Option<i64> = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT min(retry_at) FROM deletion_effects WHERE completed_at IS NULL",
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
async fn dispose(state: &Arc<State>, id: &str, tid: &str, p: &Value) -> Result<()> {
    if get(&state.db.lock().unwrap(), "thread", tid).is_ok_and(|t| t["deletedAt"].is_null()) {
        return Err("Refusing resource disposal for a live thread.".into());
    }
    signal(state, tid);
    state.executions.stopped(tid).await?;
    let handled = state.native_mcp.dispose_thread(tid).await?;
    let provider = state.provider.lock().unwrap().clone();
    if let Some(provider) = provider.filter(|_| !handled) {
        provider
            .detach(
                p["providerInstanceId"].as_str().unwrap_or(""),
                p["threadKey"].as_str().unwrap_or(""),
                p["nativeThreadId"].as_str(),
            )
            .await?;
    }
    state.streams.commands.finish_thread(tid);
    state.streams.terminals.cleanup(tid).await?;
    crate::preview::cleanup(state, tid).await?;
    {
        let db = state.db.lock().unwrap();
        db.execute("DELETE FROM records WHERE kind='scheduledTasks' AND (json_extract(body,'$.threadId')=?1 OR json_extract(body,'$.createdByThreadId')=?1 OR json_extract(body,'$.anchorThreadId')=?1)",[tid]).map_err(|e|e.to_string())?;
    }
    state.streams.ops.wake_scheduler();
    let mut after = String::new();
    loop {
        let attachment:Option<String>=state.db.lock().unwrap().query_row("SELECT attachment_id FROM deletion_attachments WHERE effect_id=?1 AND attachment_id>?2 ORDER BY attachment_id LIMIT 1",params![id,after],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        let Some(attachment) = attachment else {
            break;
        };
        after = attachment.clone();
        let shared:bool=state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM records m,json_each(m.body,'$.attachments') a JOIN records t ON t.kind='thread' AND t.id=m.thread_id WHERE m.kind='messages' AND json_extract(t.body,'$.deletedAt') IS NULL AND a.type='object' AND coalesce(json_extract(a.value,'$.id'),json_extract(a.value,'$.attachmentId'))=?1)",[&attachment],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !shared {
            crate::assets::cleanup_attachment(state, &attachment)?;
        }
    }
    Ok(())
}
