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
            "SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",
            [tid],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"id":id,"threadId":tid,"runId":rid,"nodeId":rid,"providerThreadId":get(db,"runs",rid).ok().map(|r|r["providerThreadId"].clone()),"providerTurnId":rid,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"running","title":null,"startedAt":at,"completedAt":null,"updatedAt":at}),
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
        let root = get(db, "runs", v["runId"].as_str().unwrap_or(""))
            .ok()
            .map(|r| r["rootNodeId"].clone())
            .filter(|v| !v.is_null())
            .unwrap_or_else(|| v["runId"].clone());
        let node = json!({"id":id,"threadId":tid,"runId":v["runId"],"parentNodeId":v["parentItemId"].as_str().map(|s|json!(s)).unwrap_or_else(||root.clone()),"rootNodeId":root,"kind":kind,"status":v["status"],"countsForRun":true,"providerThreadId":t["activeProviderThreadId"],"providerTurnId":v["providerTurnId"],"nativeItemRef":v["nativeItemRef"],"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":v["startedAt"],"completedAt":v["completedAt"]});
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
            let native = f["session_id"].as_str().ok_or("session id missing")?;
            crate::native_recovery::bound(db, run, native)?;
            let owned = t["activeProviderThreadId"]
                .as_str()
                .unwrap_or(native)
                .to_owned();
            let id = owned.as_str();
            t["activeProviderThreadId"] = json!(id);
            run["providerThreadId"] = json!(id);
            run["rootNodeId"] = json!(rid);
            let aid = format!("attempt-{rid}");
            run["activeAttemptId"] = json!(aid);
            let attempt = json!({"id":aid,"runId":rid,"attemptOrdinal":1,"providerInstanceId":t["providerInstanceId"],"providerThreadId":id,"reason":"initial","rootNodeId":rid,"nativeThreadId":native,"providerTurnId":rid,"status":"running","startedAt":at,"completedAt":null});
            put(db, "attempts", &aid, tid, &attempt)?;
            append(db, tid, "attempt.updated", attempt, t)?;
            put(db, "thread", tid, tid, t)?;
            append(db, tid, "thread.metadata-updated", t.clone(), t)?;
            let capabilities: Value =
                serde_json::from_str(include_str!("claude-capabilities.json"))
                    .map_err(|e| e.to_string())?;
            let cwd = t["worktreePath"].as_str().unwrap_or(".");
            let session = json!({"id":id,"driver":"claudeAgent","providerInstanceId":t["providerInstanceId"],"status":"running","cwd":cwd,"model":run["modelSelection"]["model"],"capabilities":capabilities,"createdAt":at,"updatedAt":at,"lastError":null});
            put(db, "providerSessions", id, tid, &session)?;
            append(db, tid, "provider-session.updated", session, t)?;
            let previous = get(db, "providerThreads", id).unwrap_or(Value::Null);
            let p = json!({"id":id,"driver":"claudeAgent","providerInstanceId":t["providerInstanceId"],"providerSessionId":id,"appThreadId":tid,"ownerNodeId":null,"nativeThreadRef":reference(&json!(native)),"nativeConversationHeadRef":null,"contextUsage":if previous["nativeThreadRef"]["nativeId"]==native{previous["contextUsage"].clone()}else{Value::Null},"goal":if previous["nativeThreadRef"]["nativeId"]==native{previous["goal"].clone()}else{Value::Null},"nativeMetadata":if previous["nativeThreadRef"]["nativeId"]==native{previous["nativeMetadata"].clone()}else{Value::Null},"pendingBackgroundTasks":if previous["nativeThreadRef"]["nativeId"]==native{previous.get("pendingBackgroundTasks").cloned().unwrap_or(json!([]))}else{json!([])},"status":"active","firstRunOrdinal":previous.get("firstRunOrdinal").filter(|ordinal|ordinal.is_number()).unwrap_or(&run["ordinal"]),"lastRunOrdinal":run["ordinal"],"handoffIds":previous.get("handoffIds").cloned().unwrap_or(json!([])),"forkedFrom":previous["forkedFrom"],"createdAt":previous.get("createdAt").unwrap_or(&json!(at)),"updatedAt":at});
            put(db, "providerThreads", id, tid, &p)?;
            append(db, tid, "provider-thread.updated", p, t)?;
            let turn = json!({"id":rid,"providerThreadId":id,"nodeId":rid,"runAttemptId":aid,"nativeTurnRef":null,"ordinal":run["ordinal"],"status":"running","startedAt":at,"completedAt":null});
            put(db, "providerTurns", rid, tid, &turn)?;
            append(db, tid, "provider-turn.updated", turn, t)?;
            let node = json!({"id":rid,"threadId":tid,"runId":rid,"parentNodeId":null,"rootNodeId":rid,"kind":"root_turn","status":"running","countsForRun":true,"providerThreadId":id,"providerTurnId":rid,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":at,"completedAt":null});
            put(db, "nodes", rid, tid, &node)?;
            append(db, tid, "node.updated", node, t)?;
        }
        "proposed_plan" => {
            let id = f["id"].as_str().ok_or("plan id missing")?;
            let plan = json!({"id":id,"threadId":tid,"runId":rid,"nodeId":rid,"status":"active","kind":"proposed_plan","markdown":f["markdown"]});
            put(db, "plans", id, tid, &plan)?;
            append(db, tid, "plan.updated", plan, t)?;
            let mut i = item(db, tid, rid, &format!("item-{id}"), at)?;
            i["type"] = json!("proposed_plan");
            i["planId"] = json!(id);
            i["markdown"] = f["markdown"].clone();
            i["streaming"] = json!(false);
            i["status"] = json!("completed");
            i["completedAt"] = json!(at);
            save_item(db, tid, t, i)?;
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
            let owned:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='backgroundTasks' AND thread_id=? AND json_extract(body,'$.itemId')=? AND json_extract(body,'$.status')='running')",rusqlite::params![tid,id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if owned {
                i["status"] = json!("running");
                i["completedAt"] = Value::Null;
            }
            if i["type"] == "subagent" {
                let sub = json!({"id":id,"threadId":tid,"runId":rid,"parentNodeId":rid,"origin":"provider_native","createdBy":"agent","driver":"claudeAgent","providerInstanceId":run["providerInstanceId"],"providerThreadId":t["activeProviderThreadId"],"childThreadId":null,"nativeTaskRef":reference(native),"prompt":i["prompt"],"title":i["title"],"model":null,"status":i["status"],"result":i["result"],"startedAt":i["startedAt"],"completedAt":i["completedAt"],"updatedAt":at});
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
            let outcome = match frame["status"].as_str() {
                Some("completed") => "completed",
                Some("stopped" | "cancelled" | "killed") => "cancelled",
                _ => "failed",
            };
            if frame["subtype"] == "background_tasks_changed" {
                for task in frame["tasks"].as_array().into_iter().flatten() {
                    crate::background::update(
                        db,
                        tid,
                        run,
                        t,
                        at,
                        task["task_id"].as_str().ok_or("task id missing")?,
                        if task["task_type"] == "local_bash" {
                            "command"
                        } else {
                            "background_task"
                        },
                        false,
                        "running",
                        None,
                    )?;
                }
            }
            let task_id = frame["task_id"].as_str().unwrap_or("");
            let id = get(db, "claudeTasks", task_id)
                .ok()
                .and_then(|v| v["itemId"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    format!("tool-{rid}-{}", frame["tool_use_id"].as_str().unwrap_or(""))
                });
            if matches!(
                frame["subtype"].as_str(),
                Some("task_started" | "task_notification")
            ) {
                crate::background::update(
                    db,
                    tid,
                    run,
                    t,
                    at,
                    task_id,
                    if frame["task_type"] == "local_bash" {
                        "command"
                    } else {
                        "subagent"
                    },
                    frame["subtype"] == "task_notification",
                    outcome,
                    Some(&id),
                )?;
            }
            if frame["subtype"] == "task_started" {
                if let Ok(mut i) = get(db, "turnItems", &id) {
                    i["status"] = json!("running");
                    i["completedAt"] = Value::Null;
                    save_item(db, tid, t, i)?;
                }
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
                    sub["status"] = json!(outcome);
                    sub["result"] = frame["summary"].clone();
                    sub["completedAt"] = json!(at);
                    sub["updatedAt"] = json!(at);
                    put(db, "subagents", &id, tid, &sub)?;
                    append(db, tid, "subagent.updated", sub, t)?;
                }
                if let Ok(mut i) = get(db, "turnItems", &id) {
                    i["status"] = json!(outcome);
                    i["result"] = frame["summary"].clone();
                    i["completedAt"] = json!(at);
                    i["updatedAt"] = json!(at);
                    save_item(db, tid, t, i)?;
                }
            }
        }
        "permission" => {
            let id = f["request_id"].as_str().ok_or("request id missing")?;
            let user = f["request"]["tool_name"] == "AskUserQuestion";
            let node_id = format!("node-{id}");
            let node = json!({"id":node_id,"threadId":tid,"runId":rid,"parentNodeId":rid,"rootNodeId":rid,"kind":if user{"user_input_request"}else{"approval_request"},"status":"waiting","countsForRun":false,"providerThreadId":t["activeProviderThreadId"],"providerTurnId":rid,"nativeItemRef":reference(&f["native_id"]),"runtimeRequestId":id,"checkpointScopeId":null,"startedAt":at,"completedAt":null});
            put(db, "nodes", &node_id, tid, &node)?;
            append(db, tid, "node.updated", node, t)?;
            let mut req = json!({"id":id,"nodeId":node_id,"providerTurnId":rid,"nativeRequestRef":reference(&f["native_id"]),"kind":if user {"user_input"}else{"permission"},"status":"pending","responseCapability":{"type":"live","providerSessionId":t["activeProviderThreadId"],"supportsFeedback":true},"createdAt":at,"resolvedAt":null});
            if let Some(key) = crate::approval_policy::key(f, true) {
                req["approvalPolicyKey"] = key;
            }
            put(db, "runtimeRequests", id, tid, &req)?;
            append(db, tid, "runtime-request.updated", req, t)?;
            let mut i = item(db, tid, rid, &format!("approval-{id}"), at)?;
            i["type"] = json!(if user {
                "user_input_request"
            } else {
                "approval_request"
            });
            if user {
                i["questions"] = questions(&f["request"]["input"]);
            }
            i["nodeId"] = json!(node_id);
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
        "permission_cancelled" => {
            let id = f["request_id"].as_str().ok_or("request id missing")?;
            let Some(mut request) = get(db, "runtimeRequests", id)
                .ok()
                .filter(|r| r["status"] == "pending" && r["providerTurnId"] == rid)
            else {
                return Ok(());
            };
            request["status"] = json!("cancelled");
            request["resolvedAt"] = json!(at);
            put(db, "runtimeRequests", id, tid, &request)?;
            append(db, tid, "runtime-request.updated", request.clone(), t)?;
            if let Some(node_id) = request["nodeId"].as_str() {
                if let Ok(mut node) = get(db, "nodes", node_id) {
                    if node["runId"] == rid && node["id"] != rid {
                        node["status"] = json!("cancelled");
                        node["completedAt"] = json!(at);
                        put(db, "nodes", node_id, tid, &node)?;
                        append(db, tid, "node.updated", node, t)?;
                    }
                }
            }
            if let Ok(mut card) = get(db, "turnItems", &format!("approval-{id}")) {
                card["status"] = json!("cancelled");
                card["completedAt"] = json!(at);
                card["updatedAt"] = json!(at);
                save_item(db, tid, t, card)?;
            }
            let pending: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE kind='runtimeRequests' AND thread_id=? AND json_extract(body,'$.providerTurnId')=? AND json_extract(body,'$.status')='pending')",
                [tid, rid], |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if !pending && run["status"] == "waiting" {
                run["status"] = json!("running");
            }
        }
        "rate_limit" => {
            let info = &f["info"];
            if info["status"] == "rejected" {
                run["errorClass"] = json!("usage_limit");
            }
            t["usageLimitResetAt"] = if info["status"] == "rejected" {
                info["resetsAt"]
                    .as_i64()
                    .and_then(|s| s.checked_mul(1000))
                    .filter(|s| time::OffsetDateTime::from_unix_timestamp(*s / 1000).is_ok())
                    .map(crate::store::iso)
                    .map(Value::String)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            put(db, "thread", tid, tid, t)?;
            append(db, tid, "thread.metadata-updated", t.clone(), t)?;
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
                turn["tokenUsage"] = json!({"usedTokens":input+cached+output+created,"inputTokens":input+created,"cacheCreationTokens":created,"cachedInputTokens":cached,"outputTokens":output,"updatedAt":at});
                if f["cost"].is_number() {
                    turn["tokenUsage"]["cost"] = f["cost"].clone();
                }
                if let Some(window) = f["modelUsage"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values())
                    .filter_map(|m| m["contextWindow"].as_u64())
                    .max()
                {
                    turn["tokenUsage"]["maxTokens"] = json!(window);
                }
                if let Some(id) = t["activeProviderThreadId"].as_str() {
                    if let Ok(mut pt) = get(db, "providerThreads", id) {
                        pt["contextUsage"] = turn["tokenUsage"].clone();
                        put(db, "providerThreads", id, tid, &pt)?;
                        append(db, tid, "provider-thread.updated", pt, t)?;
                    }
                }
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
    let aid = format!("attempt-{rid}");
    let ended = |v: &Value| {
        matches!(
            v["status"].as_str(),
            Some("completed" | "failed" | "interrupted" | "cancelled")
        )
    };
    if let Some(mut a) = get(db, "attempts", &aid).ok().filter(|a| !ended(a)) {
        a["status"] = json!(status);
        a["completedAt"] = json!(at);
        put(db, "attempts", &aid, tid, &a)?;
        append(db, tid, "attempt.updated", a, t)?;
    }
    for (kind, event) in [
        ("nodes", "node.updated"),
        ("providerTurns", "provider-turn.updated"),
    ] {
        if let Some(mut v) = get(db, kind, rid).ok().filter(|v| !ended(v)) {
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

fn questions(input: &Value) -> Value {
    json!(input["questions"].as_array().into_iter().flatten().enumerate().filter_map(|(index,q)| {
        let text = q["question"].as_str()?.trim();
        if text.is_empty() { return None; }
        let options:Vec<Value> = q["options"].as_array().into_iter().flatten().filter_map(|o| {
            let label = o["label"].as_str()?.trim();
            if label.is_empty() { return None; }
            let mut option=json!({"label":label,"description":o["description"].as_str().filter(|d| !d.trim().is_empty()).unwrap_or(label)});
            if let Some(preview)=o["preview"].as_str() { option["preview"]=json!(preview); }
            Some(option)
        }).collect();
        Some(json!({"id":text,"question":text,"header":q["header"].as_str().filter(|h| !h.trim().is_empty()).map(str::to_owned).unwrap_or(format!("Question {}",index+1)),"options":options,"multiSelect":q["multiSelect"]==true}))
    }).collect::<Vec<_>>())
}
