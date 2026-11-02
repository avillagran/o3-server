//! Common transcript operations shared by provider drivers.
pub(crate) use crate::claude_projection::save_item;
pub(crate) fn finish(
    db: &rusqlite::Connection,
    tid: &str,
    rid: &str,
    thread: &serde_json::Value,
    at: &str,
    status: &str,
) -> Result<(), String> {
    let mut q=db.prepare("SELECT id,body FROM records WHERE kind='subagents' AND thread_id=? AND json_extract(body,'$.runId')=? AND json_extract(body,'$.status') IN ('running','pending','waiting','starting')").map_err(|e|e.to_string())?;
    let rows = q
        .query_map([tid, rid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(q);
    for (id, body) in rows {
        let mut v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        v["status"] = serde_json::json!(status);
        v["completedAt"] = serde_json::json!(at);
        v["updatedAt"] = serde_json::json!(at);
        crate::events::put(db, "subagents", &id, tid, &v)?;
        crate::events::append(db, tid, "subagent.updated", v, thread)?;
    }
    // A terminal run no longer owns a live native response channel.
    let mut q = db.prepare("SELECT r.id,r.body FROM records r JOIN records n ON n.kind='nodes' AND n.id=json_extract(r.body,'$.nodeId') WHERE r.kind='runtimeRequests' AND r.thread_id=? AND json_extract(n.body,'$.runId')=? AND json_extract(r.body,'$.status')='pending'").map_err(|e|e.to_string())?;
    let requests = q
        .query_map([tid, rid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(q);
    for (id, body) in requests {
        let mut request: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;
        request["status"] = serde_json::json!("cancelled");
        request["resolvedAt"] = serde_json::json!(at);
        crate::events::put(db, "runtimeRequests", &id, tid, &request)?;
        crate::events::append(db, tid, "runtime-request.updated", request, thread)?;
    }
    crate::background::finish(db, tid, rid, thread, at, status)?;
    if thread["providerInstanceId"] == "codex" {
        crate::codex_projection::finish(db, tid, rid, thread, &serde_json::json!(status), at)
    } else {
        crate::claude_projection::finish(db, tid, rid, thread, at, status)
    }
}

/// Projection identity is allocated before the native transport starts.
pub(crate) fn native_id(db: &rusqlite::Connection, t: &serde_json::Value) -> Option<String> {
    let key = if t["providerInstanceId"] == "codex" {
        "codexNativeThreadId"
    } else {
        "activeProviderThreadId"
    };
    let id = t[key].as_str()?;
    if key == "codexNativeThreadId" {
        return Some(id.into());
    }
    match crate::events::get(db, "providerThreads", id) {
        Ok(p) => p["nativeThreadRef"]["nativeId"].as_str().map(str::to_owned),
        Err(_) => Some(id.into()), // Imported legacy threads used native IDs directly.
    }
}
