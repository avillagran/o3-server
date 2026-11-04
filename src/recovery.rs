//! Never leave dead in-process executions advertised as live after a restart.
use crate::{
    events,
    store::{iso, now_ms},
};
use rusqlite::Connection;
use serde_json::{json, Value};
pub fn recover(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    crate::deletion::recover(&tx)?;
    use rusqlite::OptionalExtension;
    let mut cursor = 0i64;
    loop {
        let row=tx.query_row("SELECT rowid,kind,id,thread_id,body FROM records WHERE rowid>? AND kind IN ('runs','runtimeRequests','providerSessions','controlOutbox','thread') ORDER BY rowid LIMIT 1",[cursor],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional().map_err(|e|e.to_string())?;
        let Some((rowid, kind, id, tid, body)) = row else {
            break;
        };
        cursor = rowid;
        let mut v: Value = if kind == "runs" {
            events::get(&tx, &kind, &id)?
        } else {
            serde_json::from_str(&body).map_err(|e| e.to_string())?
        };
        let event = match kind.as_str() {
            "thread" if !v["titleRegeneration"].is_null() => {
                v["titleRegeneration"] = Value::Null;
                "thread.metadata-updated"
            }
            "runs"
                if matches!(
                    v["status"].as_str(),
                    Some("running" | "starting" | "waiting")
                ) || v["status"] == "preparing" && v["executionRequested"] == true =>
            {
                v["status"] = json!("failed");
                v["completedAt"] = json!(iso(now_ms()));
                v["error"] = json!("server restarted during execution");
                if let Ok(t) = events::get(&tx, "thread", &tid) {
                    let at = v["completedAt"].as_str().unwrap();
                    crate::projection::finish(&tx, &tid, &id, &t, at, "failed")?;
                    let mut child_cursor = 0i64;
                    loop {
                        let row=tx.query_row("SELECT rowid,kind,id,body FROM records WHERE thread_id=? AND rowid>? AND kind IN ('nodes','attempts','subagents') AND (json_extract(body,'$.runId')=? OR json_extract(body,'$.parentNodeId')=?) AND json_extract(body,'$.status') IN ('running','starting','waiting','pending') ORDER BY rowid LIMIT 1",rusqlite::params![tid,child_cursor,id,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional().map_err(|e|e.to_string())?;
                        let Some((cursor, kind, child, body)) = row else {
                            break;
                        };
                        child_cursor = cursor;
                        let mut child_value: Value =
                            serde_json::from_str(&body).map_err(|e| e.to_string())?;
                        child_value["status"] = json!("failed");
                        child_value["completedAt"] = json!(at);
                        child_value["updatedAt"] = json!(at);
                        events::put(&tx, &kind, &child, &tid, &child_value)?;
                        events::append(
                            &tx,
                            &tid,
                            match kind.as_str() {
                                "nodes" => "node.updated",
                                "attempts" => "attempt.updated",
                                _ => "subagent.updated",
                            },
                            child_value,
                            &t,
                        )?;
                    }
                    let mut q=tx.prepare("SELECT id,body FROM records WHERE kind='messages' AND thread_id=? AND json_extract(body,'$.runId')=? AND json_extract(body,'$.streaming')=1").map_err(|e|e.to_string())?;
                    let rows = q
                        .query_map([&tid, &id], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                        })
                        .map_err(|e| e.to_string())?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    drop(q);
                    for (mid, body) in rows {
                        let mut m: Value =
                            serde_json::from_str(&body).map_err(|e| e.to_string())?;
                        m["streaming"] = json!(false);
                        m["updatedAt"] = json!(at);
                        events::put(&tx, "messages", &mid, &tid, &m)?;
                        events::append(&tx, &tid, "message.updated", m.clone(), &t)?;
                        let item = events::message_item(&tx, &tid, &m)?;
                        events::append(&tx, &tid, "turn-item.updated", item, &t)?;
                    }
                }
                if let Ok(t) = events::get(&tx, "thread", &tid) {
                    if matches!(
                        t["providerInstanceId"].as_str(),
                        Some("codex" | "claudeAgent" | "claude" | "claude-code")
                    ) && events::get(&tx, "turnItems", &format!("{id}-interrupt-request"))
                        .is_err()
                        && crate::projection::native_id(&tx, &t).is_some_and(|id| !id.is_empty())
                    {
                        v["status"] = json!("starting");
                        v["completedAt"] = Value::Null;
                        v["error"] = Value::Null;
                        v["recoveryPending"] = json!(true);
                        v["recoveryReason"] = json!("server restarted; continuing native session in a new turn; prior tools may have executed; old approvals/questions cancelled");
                    }
                }
                "run.updated"
            }
            "runtimeRequests" if v["status"] == "pending" => {
                v["status"] = json!("cancelled");
                v["resolvedAt"] = json!(iso(now_ms()));
                v["resolutionReason"] = json!("server-restarted; native approval channel closed; resend the message to request approval again");
                "runtime-request.updated"
            }
            "providerSessions"
                if matches!(
                    v["status"].as_str(),
                    Some("running" | "starting" | "ready" | "connected")
                ) =>
            {
                v["status"] = json!("disconnected");
                "provider-session.updated"
            }
            "controlOutbox" if matches!(v["status"].as_str(), Some("pending" | "handed-off")) => {
                v["status"] = json!("delivery-unknown");
                v["error"] = json!("server restarted; native execution acknowledgement unavailable; not replayed into another turn");
                events::put(&tx, &kind, &id, &tid, &v)?;
                if let Ok(mut t) = events::get(&tx, "thread", &tid) {
                    t["lastControlDeliveryError"] =
                        json!({"commandId":id,"status":"delivery-unknown","message":v["error"]});
                    events::put(&tx, "thread", &tid, &tid, &t)?;
                    events::append(&tx, &tid, "thread.metadata-updated", t.clone(), &t)?;
                }
                continue;
            }
            _ => continue,
        };
        events::put(&tx, &kind, &id, &tid, &v)?;
        if let Ok(t) = events::get(&tx, "thread", &tid) {
            events::append(&tx, &tid, event, v, &t)?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Start only executions selected by durable recovery, after provider registration.
pub fn resume(state: &std::sync::Arc<crate::store::State>) -> Result<(), String> {
    let rows = {
        let db = state.db.lock().unwrap();
        let mut q = db.prepare("SELECT thread_id,id FROM records WHERE kind='runs' AND id IN (SELECT id FROM run_internal WHERE json_extract(body,'$.recoveryPending')=1) AND json_extract(body,'$.status')='starting'").map_err(|e|e.to_string())?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let enabled = crate::settings::read(state)?["continueThreadsAfterServerUpdate"] != false;
    for (tid, rid) in rows {
        if !enabled {
            let mut db = state.db.lock().unwrap();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let mut run = events::get(&tx, "runs", &rid)?;
            run["status"] = json!("cancelled");
            run["recoveryPending"] = json!(false);
            run["completedAt"] = json!(iso(now_ms()));
            run.as_object_mut().unwrap().remove("error");
            events::put(&tx, "runs", &rid, &tid, &run)?;
            if let Ok(t) = events::get(&tx, "thread", &tid) {
                crate::projection::finish(
                    &tx,
                    &tid,
                    &rid,
                    &t,
                    run["completedAt"].as_str().unwrap(),
                    "cancelled",
                )?;
                events::append(&tx, &tid, "run.updated", run, &t)?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            continue;
        }
        crate::provider::start(state.clone(), tid, rid,
            "The server restarted during your previous turn. Continue the interrupted task using this native session's history. Inspect current state before acting: previous tools may already have executed. Do not repeat completed work. Any old approval or question was cancelled; request fresh authorization or ask again where necessary.".into());
    }
    Ok(())
}

#[cfg(test)]
mod title_tests {
    use super::*;
    #[test]
    fn restart_clears_busy_title_without_changing_title_or_resurrecting_request() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let state = crate::store::State::open(&dir).unwrap();
        {
            let mut db = state.db.lock().unwrap();
            let t =
                json!({"id":"t","title":"Renamed","titleRegeneration":{"requestId":"dead-task"}});
            events::put(&db, "thread", "t", "t", &t).unwrap();
            recover(&mut db).unwrap();
            let t = events::get(&db, "thread", "t").unwrap();
            assert!(t["titleRegeneration"].is_null());
            assert_eq!(t["title"], "Renamed");
            let seq = events::sequence(&db).unwrap();
            recover(&mut db).unwrap();
            assert_eq!(events::sequence(&db).unwrap(), seq);
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
