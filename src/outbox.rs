//! Receipt-atomic journal for controls. Native protocols cannot acknowledge exactly-once
//! delivery; an interrupted handoff is retained as delivery-unknown, never replayed
//! blindly into a different native turn (especially permission responses).
use crate::{events, store::State};
use rusqlite::Connection;
use serde_json::{json, Value};

pub fn stage(db: &Connection, command: &Value) -> Result<(), String> {
    let control = matches!(
        command["type"].as_str(),
        Some(
            "run.interrupt"
                | "runtime-request.respond"
                | "thread.user-input.dismiss"
                | "queued-message.promote-to-steer"
                | "subagent.stop"
        )
    ) || command["type"] == "message.dispatch"
        && command["dispatchMode"]["type"] == "steer_active";
    if !control {
        return Ok(());
    }
    let id = command["commandId"].as_str().ok_or("commandId required")?;
    let tid = command["threadId"].as_str().ok_or("threadId required")?;
    events::put(
        db,
        "controlOutbox",
        id,
        tid,
        &json!({"id":id,"threadId":tid,"command":command,"status":"pending","createdAt":crate::store::iso(crate::store::now_ms())}),
    )
}

/// Handoff acknowledges the bounded adapter channel, not provider-native execution.
pub fn handed_off(s: &State, id: &str) -> Result<(), String> {
    let db = s.db.lock().unwrap();
    if let Ok(mut v) = events::get(&db, "controlOutbox", id) {
        // Native failure/recovery may win the race with this post-send journal update.
        if v["status"] != "pending" {
            return Ok(());
        }
        v["status"] = json!("handed-off");
        let tid = v["threadId"]
            .as_str()
            .ok_or("outbox thread missing")?
            .to_owned();
        events::put(&db, "controlOutbox", id, &tid, &v)?;
    }
    Ok(())
}
