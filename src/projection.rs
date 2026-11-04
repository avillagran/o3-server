//! Common transcript operations shared by provider drivers.
pub(crate) use crate::claude_projection::save_item;
pub(crate) fn question_answer(item: &mut serde_json::Value, command: &serde_json::Value) {
    use serde_json::{json, Value};
    if item["type"] != "user_input_request" || command.get("answers").is_none() {
        return;
    }
    let texts = item["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| Some((q["id"].as_str()?.to_owned(), q["question"].clone())))
        .collect::<serde_json::Map<String, Value>>();
    item["questionAnswer"] = json!({"requestId":command["requestId"],"answers":command["answers"],"attachmentsByQuestionId":command.get("attachmentsByQuestionId").cloned().unwrap_or(json!({})),"questionTextById":texts});
}
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
