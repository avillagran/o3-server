//! Never leave dead in-process executions advertised as live after a restart.
use crate::{
    events,
    store::{iso, now_ms},
};
use rusqlite::Connection;
use serde_json::{json, Value};
pub fn recover(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    use rusqlite::OptionalExtension;
    let mut cursor = 0i64;
    loop {
        let row=tx.query_row("SELECT rowid,kind,id,thread_id,body FROM records WHERE rowid>? AND kind IN ('runs','runtimeRequests','providerSessions') ORDER BY rowid LIMIT 1",[cursor],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).optional().map_err(|e|e.to_string())?;
        let Some((rowid, kind, id, tid, body)) = row else {
            break;
        };
        cursor = rowid;
        let mut v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let event = match kind.as_str() {
            "runs"
                if matches!(
                    v["status"].as_str(),
                    Some("running" | "starting" | "waiting")
                ) || v["status"] == "preparing" && v["executionRequested"] == true =>
            {
                v["status"] = json!("failed");
                v["completedAt"] = json!(iso(now_ms()));
                v["error"] = json!("server restarted during execution");
                "run.updated"
            }
            "runtimeRequests" if v["status"] == "pending" => {
                v["status"] = json!("cancelled");
                v["resolvedAt"] = json!(iso(now_ms()));
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
            _ => continue,
        };
        events::put(&tx, &kind, &id, &tid, &v)?;
        if let Ok(t) = events::get(&tx, "thread", &tid) {
            events::append(&tx, &tid, event, v, &t)?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}
