//! Bounded record reducers; native frames are discarded after each transaction.
use crate::events::{append, get, put};
use rusqlite::Connection;
use serde_json::{json, Value};
fn reference(id: &Value) -> Value {
    json!({"driver":"claudeAgent","nativeId":id,"strength":"strong"})
}
fn item(db: &Connection, tid: &str, rid: &str, id: &str, at: &str) -> Result<Value, String> {
    if let Ok(v) = get(db, "turnItems", id) {
        return Ok(v);
    }
    let ordinal: i64 = db
        .query_row(
            "SELECT count(*) FROM records WHERE kind='turnItems' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"id":id,"threadId":tid,"runId":rid,"nodeId":rid,"providerThreadId":null,"providerTurnId":rid,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"running","title":null,"startedAt":at,"completedAt":null,"updatedAt":at}),
    )
}
pub(crate) fn save_item(db: &Connection, tid: &str, t: &Value, mut v: Value) -> Result<(), String> {
    let owned_id = v["id"].as_str().ok_or("item id missing")?.to_owned();
    let id = owned_id.as_str();
    let kind = match v["type"].as_str() {
        Some("subagent") => Some("subagent"),
        Some("command_execution" | "dynamic_tool") => Some("tool_call"),
        Some("reasoning") => Some("reasoning"),
        _ => None,
    };
    if let Some(kind) = kind {
        v["nodeId"] = json!(id);
        let node = json!({"id":id,"threadId":tid,"runId":v["runId"],"parentNodeId":v["parentItemId"].as_str().map(|s|json!(s)).unwrap_or_else(||v["runId"].clone()),"rootNodeId":v["runId"],"kind":kind,"status":v["status"],"countsForRun":true,"providerThreadId":t["activeProviderThreadId"],"providerTurnId":v["providerTurnId"],"nativeItemRef":v["nativeItemRef"],"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":v["startedAt"],"completedAt":v["completedAt"]});
        put(db, "nodes", id, tid, &node)?;
        append(db, tid, "node.updated", node, t)?;
    }
    put(db, "turnItems", id, tid, &v)?;
    put(
        db,
        "visibleTurnItems",
        id,
        tid,
        &json!({"position":v["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":v}),
    )?;
    append(db, tid, "turn-item.updated", v, t)?;
    Ok(())
}
pub(crate) fn persist(
    db: &Connection,
    tid: &str,
    rid: &str,
    t: &mut Value,
    run: &mut Value,
    at: &str,
    f: &Value,
) -> Result<(), String> {
    match f["type"].as_str().unwrap_or("") {
        "session" => {
            let id = f["session_id"].as_str().ok_or("session id missing")?;
            t["activeProviderThreadId"] = json!(id);
            run["providerThreadId"] = json!(id);
            run["rootNodeId"] = json!(rid);
            put(db, "thread", tid, tid, t)?;
            append(db, tid, "thread.metadata-updated", t.clone(), t)?;
            let capabilities: Value =
                serde_json::from_str(include_str!("claude-capabilities.json"))
                    .map_err(|e| e.to_string())?;
            let cwd = t["worktreePath"].as_str().unwrap_or(".");
            let session = json!({"id":id,"driver":"claudeAgent","providerInstanceId":t["providerInstanceId"],"status":"running","cwd":cwd,"model":t["modelSelection"]["model"],"capabilities":capabilities,"createdAt":at,"updatedAt":at,"lastError":null});
            put(db, "providerSessions", id, tid, &session)?;
            append(db, tid, "provider-session.updated", session, t)?;
            let p = json!({"id":id,"driver":"claudeAgent","providerInstanceId":t["providerInstanceId"],"providerSessionId":id,"appThreadId":tid,"ownerNodeId":rid,"nativeThreadRef":reference(&json!(id)),"nativeConversationHeadRef":null,"status":"active","firstRunOrdinal":run["ordinal"],"lastRunOrdinal":run["ordinal"],"handoffIds":[],"forkedFrom":null,"createdAt":at,"updatedAt":at});
            put(db, "providerThreads", id, tid, &p)?;
            append(db, tid, "provider-thread.updated", p, t)?;
            let turn = json!({"id":rid,"providerThreadId":id,"nodeId":rid,"runAttemptId":null,"nativeTurnRef":null,"ordinal":run["ordinal"],"status":"running","startedAt":at,"completedAt":null});
            put(db, "providerTurns", rid, tid, &turn)?;
            append(db, tid, "provider-turn.updated", turn, t)?;
            let node = json!({"id":rid,"threadId":tid,"runId":rid,"parentNodeId":null,"rootNodeId":rid,"kind":"root_turn","status":"running","countsForRun":true,"providerThreadId":id,"providerTurnId":rid,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":at,"completedAt":null});
            put(db, "nodes", rid, tid, &node)?;
            append(db, tid, "node.updated", node, t)?;
        }
        "thinking" => {
            let id = format!(
                "thinking-{rid}-{}-{}",
                f["parent_tool_use_id"].as_str().unwrap_or("root"),
                f["index"]
            );
            let mut i = item(db, tid, rid, &id, at)?;
            let mut text = i["text"].as_str().unwrap_or("").to_owned();
            text.push_str(f["text"].as_str().unwrap_or(""));
            if text.len() > 1024 * 1024 {
                return Err("reasoning exceeds 1 MiB".into());
            }
            i["type"] = json!("reasoning");
            i["text"] = json!(text);
            i["streaming"] = json!(true);
            i["updatedAt"] = json!(at);
            if let Some(parent) = f["parent_tool_use_id"].as_str() {
                i["parentItemId"] = json!(format!("tool-{rid}-{parent}"));
            }
            save_item(db, tid, t, i)?;
        }
        "tool" => {
            let b = &f["block"];
            let start = b["type"] == "tool_use";
            let native = if start { &b["id"] } else { &b["tool_use_id"] };
            let id = format!("tool-{rid}-{}", native.as_str().ok_or("tool id missing")?);
            let mut i = item(db, tid, rid, &id, at)?;
            i["nativeItemRef"] = reference(native);
            i["updatedAt"] = json!(at);
            if let Some(parent) = f["parent_tool_use_id"].as_str() {
                i["parentItemId"] = json!(format!("tool-{rid}-{parent}"));
            }
            if start {
                let name = b["name"].as_str().unwrap_or("tool");
                i["title"] = json!(name);
                i["type"] = json!(if matches!(name, "Agent" | "Task") {
                    "subagent"
                } else if name == "Bash" {
                    "command_execution"
                } else {
                    "dynamic_tool"
                });
                i["input"] = if name == "Bash" {
                    b["input"]["command"].clone()
                } else {
                    b["input"].clone()
                };
                i["toolName"] = json!(name);
                if i["type"] == "subagent" {
                    i["subagentId"] = json!(id);
                    i["origin"] = json!("provider_native");
                    i["driver"] = json!("claude");
                    i["providerInstanceId"] = json!("claude");
                    i["childThreadId"] = Value::Null;
                    i["prompt"] = b["input"]["prompt"].as_str().unwrap_or("").into();
                    i["result"] = Value::Null;
                }
            } else {
                if i.get("type").is_none() {
                    i["type"] = json!("dynamic_tool");
                    i["toolName"] = Value::Null;
                    i["input"] = Value::Null;
                }
                let text = if let Some(s) = b["content"].as_str() {
                    s.to_owned()
                } else {
                    b["content"].to_string()
                };
                i["output"] = json!(text);
                i["outputIndicatesFailure"] = json!(b["is_error"] == true);
                i["status"] = json!(if b["is_error"] == true {
                    "failed"
                } else {
                    "completed"
                });
                i["completedAt"] = json!(at);
                if i["type"] == "subagent" {
                    let old = get(db, "subagents", &id).ok();
                    if old.as_ref().is_none_or(|v| v["completedAt"].is_null()) {
                        i["status"] = json!("running");
                        i["completedAt"] = Value::Null;
                    } else {
                        i["status"] = old.as_ref().unwrap()["status"].clone();
                    }
                    i["result"] = json!(text);
                }
            }
            if i["type"] == "subagent" {
                let sub = json!({"id":id,"threadId":tid,"runId":rid,"parentNodeId":rid,"origin":"provider_native","createdBy":"agent","driver":"claudeAgent","providerInstanceId":t["providerInstanceId"],"providerThreadId":t["activeProviderThreadId"],"childThreadId":null,"nativeTaskRef":reference(native),"prompt":i["prompt"],"title":i["title"],"model":null,"status":i["status"],"result":i["result"],"startedAt":i["startedAt"],"completedAt":i["completedAt"],"updatedAt":at});
                put(db, "subagents", &id, tid, &sub)?;
                append(db, tid, "subagent.updated", sub, t)?;
            }
            save_item(db, tid, t, i)?;
        }
        "subagent_text" => {
            let id = format!(
                "tool-{rid}-{}",
                f["parent_tool_use_id"].as_str().unwrap_or("")
            );
            if let Ok(mut i) = get(db, "turnItems", &id) {
                let mut text = i["progress"].as_str().unwrap_or("").to_owned();
                text.push_str(f["text"].as_str().unwrap_or(""));
                if text.len() > 1024 * 1024 {
                    return Err("subagent progress exceeds 1 MiB".into());
                }
                i["progress"] = json!(text);
                i["updatedAt"] = json!(at);
                save_item(db, tid, t, i)?;
            }
        }
        "task" => {
            let frame = &f["frame"];
            let id = format!("tool-{rid}-{}", frame["tool_use_id"].as_str().unwrap_or(""));
            if frame["subtype"] == "task_started" {
                put(
                    db,
                    "claudeTasks",
                    frame["task_id"].as_str().ok_or("task id missing")?,
                    tid,
                    &json!({"itemId":id,"runId":rid}),
                )?;
            }
            if frame["subtype"] == "task_notification" {
                if let Ok(mut sub) = get(db, "subagents", &id) {
                    sub["status"] = frame["status"].clone();
                    sub["result"] = frame["summary"].clone();
                    sub["completedAt"] = json!(at);
                    sub["updatedAt"] = json!(at);
                    put(db, "subagents", &id, tid, &sub)?;
                    append(db, tid, "subagent.updated", sub, t)?;
                }
                if let Ok(mut i) = get(db, "turnItems", &id) {
                    i["status"] = frame["status"].clone();
                    i["result"] = frame["summary"].clone();
                    i["completedAt"] = json!(at);
                    i["updatedAt"] = json!(at);
                    save_item(db, tid, t, i)?;
                }
            }
        }
        "permission" => {
            let id = f["request_id"].as_str().ok_or("request id missing")?;
            let req = json!({"id":id,"nodeId":rid,"providerTurnId":rid,"nativeRequestRef":reference(&f["native_id"]),"kind":"permission","status":"pending","responseCapability":{"type":"live","providerSessionId":t["activeProviderThreadId"],"supportsFeedback":true},"createdAt":at,"resolvedAt":null});
            put(db, "runtimeRequests", id, tid, &req)?;
            append(db, tid, "runtime-request.updated", req, t)?;
            let mut i = item(db, tid, rid, &format!("approval-{id}"), at)?;
            i["type"] = json!("approval_request");
            i["requestId"] = json!(id);
            i["requestKind"] = json!("permission");
            i["prompt"] = json!(format!(
                "{}: {}",
                f["request"]["tool_name"].as_str().unwrap_or("tool"),
                f["request"]["input"]
            ));
            i["status"] = json!("pending");
            save_item(db, tid, t, i)?;
            run["status"] = json!("waiting");
        }
        "usage" => {
            // Store only the current result's accounting, never the CLI transcript.
            put(db, "claudeUsage", rid, tid, f)?;
            if let Ok(mut turn) = get(db, "providerTurns", rid) {
                let u = &f["usage"];
                let input = u["input_tokens"].as_u64().unwrap_or(0);
                let cached = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                let output = u["output_tokens"].as_u64().unwrap_or(0);
                let created = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                turn["tokenUsage"] = json!({"usedTokens":input+cached+output+created,"inputTokens":input+created,"cachedInputTokens":cached,"outputTokens":output,"updatedAt":at});
                put(db, "providerTurns", rid, tid, &turn)?;
                append(db, tid, "provider-turn.updated", turn, t)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn finish(
    db: &Connection,
    tid: &str,
    rid: &str,
    t: &Value,
    at: &str,
    status: &str,
) -> Result<(), String> {
    for (kind, event) in [
        ("nodes", "node.updated"),
        ("providerTurns", "provider-turn.updated"),
    ] {
        if let Ok(mut v) = get(db, kind, rid) {
            v["status"] = json!(status);
            v["completedAt"] = json!(at);
            put(db, kind, rid, tid, &v)?;
            append(db, tid, event, v, t)?;
        }
    }
    let mut q=db.prepare("SELECT body FROM records WHERE kind='turnItems' AND thread_id=? AND json_extract(body,'$.runId')=? AND json_extract(body,'$.status') IN ('running','pending')").map_err(|e|e.to_string())?;
    let rows = q
        .query_map([tid, rid], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let mut v: Value =
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        v["status"] = json!(status);
        v["completedAt"] = json!(at);
        v["updatedAt"] = json!(at);
        if v.get("streaming").is_some() {
            v["streaming"] = json!(false);
        }
        save_item(db, tid, t, v)?;
    }
    Ok(())
}
