//! Native Codex frames reduced directly into SQLite; no adapter transcript cache.
use crate::events::{append, get, put};
use rusqlite::Connection;
use serde_json::{json, Value};
fn record(
    db: &Connection,
    tid: &str,
    thread: &Value,
    kind: &str,
    event: &str,
    v: Value,
) -> Result<(), String> {
    let mut v = v;
    if kind == "providerTurns" {
        if let Ok(old) = get(db, kind, v["id"].as_str().unwrap_or("")) {
            if v.get("tokenUsage").is_none() {
                if let Some(usage) = old.get("tokenUsage") {
                    v["tokenUsage"] = usage.clone();
                }
            }
        }
    }
    let id = v["id"].as_str().ok_or("record id missing")?;
    put(db, kind, id, tid, &v)?;
    append(db, tid, event, v, thread)?;
    Ok(())
}
fn item(db: &Connection, tid: &str, thread: &Value, mut v: Value) -> Result<(), String> {
    let id = v["id"].as_str().ok_or("item id missing")?.to_owned();
    v["ordinal"] = if let Ok(old) = get(db, "turnItems", &id) {
        old["ordinal"].clone()
    } else {
        json!(db
            .query_row(
                "SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",
                [tid],
                |r| r.get::<_, i64>(0)
            )
            .map_err(|e| e.to_string())?)
    };
    if v["type"] == "proposed_plan" {
        record(
            db,
            tid,
            thread,
            "plans",
            "plan.updated",
            json!({"id":v["planId"],"threadId":tid,"runId":v["runId"],"nodeId":v["nodeId"],"status":if v["streaming"]==true{"draft"}else{"active"},"kind":"proposed_plan","markdown":v["markdown"]}),
        )?;
    }
    put(
        db,
        "visibleTurnItems",
        &id,
        tid,
        &json!({"position":v["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":v}),
    )?;
    record(db, tid, thread, "turnItems", "turn-item.updated", v)
}
pub(crate) fn persist(
    db: &Connection,
    tid: &str,
    rid: &str,
    thread: &Value,
    run: &mut Value,
    at: &str,
    frame: Value,
) -> Result<(), String> {
    let method = frame["method"].as_str().unwrap_or("");
    let p = &frame["params"];
    let native_thread = p["threadId"].as_str().unwrap_or("");
    let native_turn = p["turnId"]
        .as_str()
        .or_else(|| p["turn"]["id"].as_str())
        .unwrap_or("");
    let root = run["rootNodeId"].as_str().unwrap_or(rid).to_owned();
    let aid = run["activeAttemptId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("attempt-{rid}"));
    let previous_attempt = get(db, "attempts", &aid).unwrap_or_default();
    let ordinal = previous_attempt["attemptOrdinal"].as_i64().unwrap_or(1);
    let reason = previous_attempt["reason"].as_str().unwrap_or("initial");
    let session_id = format!("codex-session-{tid}");
    let pt = thread["activeProviderThreadId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("codex-thread-{native_thread}"));
    let pv = format!("codex-turn-{native_turn}");
    let native_item = p["itemId"]
        .as_str()
        .or_else(|| p["item"]["id"].as_str())
        .unwrap_or(native_turn);
    let id = format!("codex-item-{root}-{native_item}");
    let mut base = json!({"id":id,"threadId":tid,"runId":rid,"nodeId":root,"providerThreadId":pt,"providerTurnId":pv,"nativeItemRef":{"driver":"codex","nativeId":native_item,"strength":"strong"},"parentItemId":null,"ordinal":db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",[tid],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?,"status":"running","title":null,"startedAt":at,"completedAt":null,"updatedAt":at});
    match method {
        "o3/binding" => {
            crate::native_recovery::bound(db, run, native_thread)?;
            let mut t = thread.clone();
            t["codexNativeThreadId"] = json!(native_thread);
            t["activeProviderThreadId"] = json!(pt);
            put(db, "thread", tid, tid, &t)?;
            run["providerThreadId"] = json!(pt);
            let previous = get(db, "providerThreads", &pt).unwrap_or(Value::Null);
            record(
                db,
                tid,
                &t,
                "providerThreads",
                "provider-thread.updated",
                json!({"id":pt,"appThreadId":tid,"driver":"codex","ownerNodeId":null,"nativeConversationHeadRef":null,"firstRunOrdinal":previous.get("firstRunOrdinal").filter(|ordinal|ordinal.is_number()).unwrap_or(&run["ordinal"]),"lastRunOrdinal":run["ordinal"],"handoffIds":previous.get("handoffIds").cloned().unwrap_or(json!([])),"forkedFrom":previous["forkedFrom"],"providerInstanceId":thread["providerInstanceId"],"providerSessionId":session_id,"nativeThreadRef":{"driver":"codex","nativeId":native_thread,"strength":"strong"},"status":"active","createdAt":previous.get("createdAt").unwrap_or(&json!(at)),"updatedAt":at,"contextUsage":if previous["nativeThreadRef"]["nativeId"]==native_thread{previous["contextUsage"].clone()}else{Value::Null},"goal":if previous["nativeThreadRef"]["nativeId"]==native_thread{previous["goal"].clone()}else{Value::Null},"nativeMetadata":if previous["nativeThreadRef"]["nativeId"]==native_thread{previous["nativeMetadata"].clone()}else{Value::Null},"pendingBackgroundTasks":if previous["nativeThreadRef"]["nativeId"]==native_thread{previous.get("pendingBackgroundTasks").cloned().unwrap_or(json!([]))}else{json!([])}}),
            )?;
            record(
                db,
                tid,
                &t,
                "providerSessions",
                "provider-session.updated",
                json!({"id":session_id,"driver":"codex","providerInstanceId":thread["providerInstanceId"],"cwd":p["cwd"],"model":thread["modelSelection"]["model"],"capabilities":serde_json::from_str::<Value>(include_str!("codex-capabilities.json")).unwrap(),"lastError":null,"status":"ready","createdAt":at,"updatedAt":at}),
            )?;
        }
        // A native turn result can precede item completion. The supervisor owns
        // the terminal projection, after the adapter drains all owned items.
        "turn/completed" => {
            if p["turn"]["status"] == "failed" {
                run["errorClass"] = if p["turn"]["error"]["codexErrorInfo"] == "usageLimitExceeded"
                {
                    json!("usage_limit")
                } else {
                    json!("unknown")
                };
            }
        }
        "turn/started" => {
            let turn_ordinal=get(db,"providerTurns",&pv).ok().and_then(|v|v["ordinal"].as_i64()).unwrap_or(db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),0)+1 FROM records WHERE kind='providerTurns' AND thread_id=?1 AND json_extract(body,'$.providerThreadId')=?2",rusqlite::params![tid,pt],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?);
            run["status"] = json!("running");
            run["rootNodeId"] = json!(root);
            run["codexProviderTurnId"] = json!(pv);
            run["activeAttemptId"] = json!(aid);
            run["startedAt"] = json!(at);
            for (kind, event, v) in [
                (
                    "providerTurns",
                    "provider-turn.updated",
                    json!({"id":pv,"providerThreadId":pt,"nodeId":root,"runAttemptId":aid,"ordinal":turn_ordinal,"nativeTurnRef":{"driver":"codex","nativeId":native_turn,"strength":"strong"},"status":"running","startedAt":at,"completedAt":null}),
                ),
                (
                    "nodes",
                    "node.updated",
                    json!({"id":root,"threadId":tid,"runId":rid,"parentNodeId":null,"rootNodeId":root,"kind":"root_turn","status":"running","countsForRun":true,"providerThreadId":pt,"providerTurnId":pv,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":at,"completedAt":null}),
                ),
                (
                    "attempts",
                    "attempt.updated",
                    json!({"id":aid,"runId":rid,"attemptOrdinal":ordinal,"providerInstanceId":thread["providerInstanceId"],"providerThreadId":pt,"reason":reason,"rootNodeId":root,"nativeThreadId":native_thread,"providerTurnId":pv,"status":"running","startedAt":at,"completedAt":null}),
                ),
            ] {
                record(db, tid, thread, kind, event, v)?;
            }
        }
        "item/started" | "item/completed" => {
            let n = &p["item"];
            let complete = method == "item/completed";
            if matches!(
                n["type"].as_str(),
                Some("commandExecution" | "collabAgentToolCall")
            ) {
                crate::background::update(
                    db,
                    tid,
                    run,
                    thread,
                    at,
                    n["id"].as_str().ok_or("item id missing")?,
                    if n["type"] == "commandExecution" {
                        "command"
                    } else {
                        "subagent"
                    },
                    complete,
                    if n["status"] == "failed" {
                        "failed"
                    } else {
                        "completed"
                    },
                    Some(&id),
                )?;
            }
            if let Ok(old) = get(db, "turnItems", &id) {
                base = old;
            }
            base["updatedAt"] = json!(at);
            base["status"] = json!(if complete {
                match n["status"].as_str() {
                    Some("failed") => "failed",
                    Some("declined") => "cancelled",
                    _ => "completed",
                }
            } else {
                "running"
            });
            if complete {
                base["completedAt"] = json!(at);
            }
            match n["type"].as_str().unwrap_or("") {
                "commandExecution" => {
                    base["type"] = json!("command_execution");
                    base["input"] = n["command"].clone();
                    if let Some(o) = n["aggregatedOutput"].as_str() {
                        base["output"] = json!(o);
                    }
                    if let Some(e) = n["exitCode"].as_i64() {
                        base["exitCode"] = json!(e);
                        base["outputIndicatesFailure"] = json!(e != 0);
                    }
                }
                "fileChange" => {
                    base["type"] = json!("file_change");
                    base["fileName"] = json!(n["changes"][0]["path"].as_str().unwrap_or("files"));
                    base["diffStr"] = json!(n["changes"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|c| c["diff"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"));
                    base["changes"] = json!(n["changes"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .map(|c| json!({"path":c["path"],"operation":c["kind"]["type"]}))
                        .collect::<Vec<_>>());
                }
                "reasoning" => {
                    for (stream, field) in [("summary", "summary"), ("text", "content")] {
                        if let Some(parts) = n[field].as_array() {
                            for (index, text) in parts.iter().enumerate() {
                                let mut part = base.clone();
                                part["id"] = json!(format!("{id}-{stream}-{index}"));
                                part["type"] = json!("reasoning");
                                part["text"] = text.clone();
                                part["streaming"] = json!(!complete);
                                item(db, tid, thread, part)?;
                            }
                        }
                    }
                    return Ok(());
                }
                "plan" => {
                    base["type"] = json!("proposed_plan");
                    base["planId"] = json!(format!("plan-{id}"));
                    base["markdown"] = n["text"].clone();
                    base["streaming"] = json!(!complete);
                }
                "mcpToolCall" | "dynamicToolCall" => {
                    base["type"] = json!("dynamic_tool");
                    base["toolName"] = n["tool"].clone();
                    base["input"] = n["arguments"].clone();
                    base["output"] = n["result"].clone();
                }
                _ => return Ok(()),
            }
            item(db, tid, thread, base)?;
        }
        "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta"
        | "item/plan/delta"
        | "item/commandExecution/outputDelta" => {
            // Summary and raw reasoning parts have separate identities.
            let field = if method.contains("reasoning") {
                "text"
            } else if method.contains("plan") {
                "markdown"
            } else {
                "output"
            };
            if method.contains("reasoning") {
                base["id"] = json!(format!(
                    "{id}-{}-{}",
                    if method.contains("summary") {
                        "summary"
                    } else {
                        "text"
                    },
                    p["summaryIndex"]
                        .as_u64()
                        .or_else(|| p["contentIndex"].as_u64())
                        .unwrap_or(0)
                ));
            }
            let key = base["id"].as_str().unwrap();
            base = get(db, "turnItems", key).unwrap_or(base);
            base["type"] = json!(if field == "text" {
                "reasoning"
            } else if field == "markdown" {
                "proposed_plan"
            } else {
                "command_execution"
            });
            if field == "output" && base.get("input").is_none() {
                base["input"] = json!("");
            }
            if field == "markdown" {
                base["planId"] = json!(format!("plan-{id}"));
            }
            let text = format!(
                "{}{}",
                base[field].as_str().unwrap_or(""),
                p["delta"].as_str().unwrap_or("")
            );
            // Bound individual rendered bodies, while SQLite remains the authority.
            base[field] = json!(if text.len() > 256 * 1024 {
                text.chars().take(256 * 1024).collect::<String>()
            } else {
                text
            });
            base["streaming"] = json!(true);
            base["updatedAt"] = json!(at);
            item(db, tid, thread, base)?;
        }
        "turn/plan/updated" => {
            let plan_id = format!("plan-{rid}");
            let steps=json!(p["plan"].as_array().unwrap_or(&vec![]).iter().enumerate().map(|(i,s)|json!({"id":format!("step-{i}"),"text":s["step"],"status":match s["status"].as_str(){Some("inProgress")=>"running",Some("completed")=>"completed",_=>"pending"}})).collect::<Vec<_>>());
            record(
                db,
                tid,
                thread,
                "plans",
                "plan.updated",
                json!({"id":plan_id,"threadId":tid,"runId":rid,"nodeId":root,"status":"active","kind":"todo_list","steps":steps,"explanation":p["explanation"]}),
            )?;
            base["id"] = json!(format!("item-{plan_id}"));
            base["type"] = json!("todo_list");
            base["planId"] = json!(plan_id);
            base["steps"] = steps;
            if p["explanation"].is_string() {
                base["explanation"] = p["explanation"].clone();
            }
            item(db, tid, thread, base)?;
        }
        "thread/tokenUsage/updated" => {
            let usage = &p["tokenUsage"];
            let last = &usage["last"];
            let mut turn =
                get(db, "providerTurns", &pv).unwrap_or(json!({"id":pv,"providerThreadId":pt}));
            turn["tokenUsage"] = json!({"usedTokens":last["totalTokens"],"maxTokens":usage["modelContextWindow"],"inputTokens":last["inputTokens"],"cachedInputTokens":last["cachedInputTokens"],"outputTokens":last["outputTokens"],"reasoningOutputTokens":last["reasoningOutputTokens"],"updatedAt":at});
            if let Ok(mut provider_thread) = get(db, "providerThreads", &pt) {
                provider_thread["contextUsage"] = turn["tokenUsage"].clone();
                record(
                    db,
                    tid,
                    thread,
                    "providerThreads",
                    "provider-thread.updated",
                    provider_thread,
                )?;
            }
            record(
                db,
                tid,
                thread,
                "providerTurns",
                "provider-turn.updated",
                turn,
            )?;
        }
        "account/rateLimits/updated" => {
            let mut session = get(db, "providerSessions", &session_id).unwrap_or(
                json!({"id":session_id,"providerInstanceId":thread["providerInstanceId"]}),
            );
            let limits = &p["rateLimits"];
            let exhausted: Vec<_> = ["primary", "secondary"]
                .iter()
                .filter_map(|key| {
                    let window = &limits[*key];
                    (window["usedPercent"].as_f64().is_some_and(|v| v >= 100.0)).then_some(window)
                })
                .collect();
            let reset = exhausted
                .iter()
                .filter_map(|w| w["resetsAt"].as_i64())
                .max();
            let mut updated = get(db, "thread", tid)?;
            if limits["primary"].is_object() || limits["secondary"].is_object() {
                updated["usageLimitResetAt"] = reset
                    .and_then(|v| v.checked_mul(1000))
                    .filter(|v| time::OffsetDateTime::from_unix_timestamp(*v / 1000).is_ok())
                    .map(crate::store::iso)
                    .map(Value::String)
                    .unwrap_or(Value::Null);
                put(db, "thread", tid, tid, &updated)?;
                append(
                    db,
                    tid,
                    "thread.metadata-updated",
                    updated.clone(),
                    &updated,
                )?;
            }
            session["rateLimits"] = p["rateLimits"].clone();
            session["updatedAt"] = json!(at);
            record(
                db,
                tid,
                thread,
                "providerSessions",
                "provider-session.updated",
                session,
            )?;
        }
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/tool/requestUserInput"
        | "mcpServer/elicitation/request" => {
            let request_id = p["o3RequestId"]
                .as_str()
                .ok_or("Codex runtime request missing id")?;
            let user = method == "item/tool/requestUserInput";
            let kind = if user {
                "user_input"
            } else if method.contains("fileChange") {
                "file-change"
            } else if method.contains("elicitation") {
                "mcp-elicitation"
            } else {
                "command"
            };
            let node = format!("node-{request_id}");
            record(
                db,
                tid,
                thread,
                "nodes",
                "node.updated",
                json!({"id":node,"threadId":tid,"runId":rid,"parentNodeId":root,"rootNodeId":root,"kind":if user{"user_input_request"}else{"approval_request"},"status":"waiting","countsForRun":false,"providerThreadId":pt,"providerTurnId":pv,"nativeItemRef":base["nativeItemRef"],"runtimeRequestId":request_id,"checkpointScopeId":null,"startedAt":at,"completedAt":null}),
            )?;
            record(
                db,
                tid,
                thread,
                "runtimeRequests",
                "runtime-request.updated",
                json!({"id":request_id,"nodeId":node,"providerTurnId":pv,"nativeRequestRef":{"driver":"codex","nativeId":frame["id"].as_str().map(str::to_owned).unwrap_or_else(||frame["id"].to_string()),"strength":"strong"},"approvalPolicyKey":crate::approval_policy::key(&frame, false),"kind":kind,"status":"pending","responseCapability":{"type":"live","providerSessionId":session_id},"createdAt":at,"resolvedAt":null}),
            )?;
            base["id"] = json!(format!("item-{request_id}"));
            base["nodeId"] = json!(node);
            base["status"] = json!("waiting");
            base["type"] = json!(if user {
                "user_input_request"
            } else {
                "approval_request"
            });
            base["requestId"] = json!(request_id);
            if user {
                base["questions"] = p["questions"].clone();
            } else {
                base["requestKind"] = json!(kind);
                base["prompt"] = json!(p["reason"]
                    .as_str()
                    .or_else(|| p["message"].as_str())
                    .or_else(|| p["command"].as_str())
                    .unwrap_or("Codex requests approval"));
            }
            item(db, tid, thread, base)?;
            run["status"] = json!("waiting");
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn finish(
    db: &Connection,
    tid: &str,
    rid: &str,
    thread: &Value,
    status: &Value,
    at: &str,
) -> Result<(), String> {
    // Page by rowid: a long turn must not load all tool items at completion.
    let mut cursor = 0i64;
    loop {
        let rows = {
            let mut q=db.prepare("SELECT rowid,body FROM records WHERE kind='turnItems' AND thread_id=? AND rowid>? ORDER BY rowid LIMIT 16").map_err(|e|e.to_string())?;
            let rows = q
                .query_map(rusqlite::params![tid, cursor], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if rows.is_empty() {
            break;
        }
        for (row, body) in rows {
            cursor = row;
            let mut v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            if matches!(
                v["type"].as_str(),
                Some("approval_request" | "user_input_request")
            ) && get(db, "runtimeRequests", v["requestId"].as_str().unwrap_or(""))
                .is_ok_and(|r| r["status"] == "pending")
            {
                continue;
            }
            if v["runId"] != rid
                || matches!(
                    v["status"].as_str(),
                    Some("completed" | "failed" | "interrupted" | "cancelled" | "superseded")
                )
            {
                continue;
            }
            v["status"] = status.clone();
            v["streaming"] = json!(false);
            v["completedAt"] = json!(at);
            v["updatedAt"] = json!(at);
            item(db, tid, thread, v)?;
        }
    }
    for (kind, event, field) in [
        ("nodes", "node.updated", "runId"),
        ("attempts", "attempt.updated", "runId"),
        ("providerTurns", "provider-turn.updated", "nodeId"),
    ] {
        let mut cursor = 0i64;
        loop {
            let rows = {
                let mut q=db.prepare("SELECT rowid,body FROM records WHERE kind=? AND thread_id=? AND rowid>? ORDER BY rowid LIMIT 16").map_err(|e|e.to_string())?;
                let rows = q
                    .query_map(rusqlite::params![kind, tid, cursor], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(|e| e.to_string())?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
            };
            if rows.is_empty() {
                break;
            }
            for (row, body) in rows {
                cursor = row;
                let mut v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                let owner = if kind == "providerTurns" {
                    get(db, "runs", rid).unwrap_or_default()["rootNodeId"]
                        .as_str()
                        .unwrap_or(rid)
                        .to_owned()
                } else {
                    rid.into()
                };
                if v[field] != owner
                    || matches!(
                        v["status"].as_str(),
                        Some("completed" | "failed" | "interrupted" | "cancelled" | "superseded")
                    )
                {
                    continue;
                }
                v["status"] = status.clone();
                v["completedAt"] = json!(at);
                record(db, tid, thread, kind, event, v)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_items_reasoning_plans_and_usage() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        let thread = json!({"id":"app-thread","modelSelection":{"model":"test"}});
        put(&db, "thread", "app-thread", "app-thread", &thread).unwrap();
        let mut run = json!({"id":"run","threadId":"app-thread","ordinal":1,"status":"preparing"});
        for line in include_str!("../tests/fixtures/codex/notifications.synthetic.ndjson").lines() {
            persist(
                &db,
                "app-thread",
                "run",
                &thread,
                &mut run,
                "2026-10-31T00:00:00.000Z",
                serde_json::from_str(line).unwrap(),
            )
            .unwrap();
        }
        finish(
            &db,
            "app-thread",
            "run",
            &thread,
            &json!("completed"),
            "2026-10-31T00:01:00.000Z",
        )
        .unwrap();
        let command = get(&db, "turnItems", "codex-item-run-command").unwrap();
        assert_eq!(command["input"], "echo ok");
        assert_eq!(command["output"], "ok\n");
        assert_eq!(command["exitCode"], 0);
        let reason = get(&db, "turnItems", "codex-item-run-reason-summary-0").unwrap();
        assert_eq!(reason["text"], "Check the result");
        assert_eq!(reason["streaming"], false);
        assert_eq!(reason["status"], "completed");
        let file = get(&db, "turnItems", "codex-item-run-files").unwrap();
        assert_eq!(file["changes"][0]["path"], "file.txt");
        assert_eq!(file["diffStr"], "+ok");
        let plan = get(&db, "plans", "plan-run").unwrap();
        assert_eq!(plan["steps"][1]["status"], "running");
        let turn = get(&db, "providerTurns", "codex-turn-native-turn").unwrap();
        assert_eq!(turn["tokenUsage"]["usedTokens"], 12);
        assert_eq!(turn["status"], "completed");
        assert_eq!(
            get(&db, "providerSessions", "codex-session-app-thread").unwrap()["rateLimits"]
                ["primary"]["usedPercent"],
            25
        );
        assert_eq!(
            get(&db, "thread", "app-thread").unwrap()["codexNativeThreadId"],
            "native-thread"
        );
        let visible = get(&db, "visibleTurnItems", "codex-item-run-command").unwrap();
        assert_eq!(visible["item"], command);
    }
}
