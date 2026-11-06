//! Thread MCP operations use the same durable command path as ordinary RPC.
use crate::{
    events,
    mcp_access::{command_error, Authority},
    mcp_tools::{definition, failure},
    store::{random_id, State},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
type Reply = Result<Value, Value>;
fn string() -> Value {
    json!({"type":"string","minLength":1})
}
fn definition_write(name: &str, desc: &str, properties: Value, required: &[&str]) -> Value {
    let mut t = definition(name, desc, properties, required);
    t["annotations"] = json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false});
    t
}
pub(crate) fn catalog() -> Vec<Value> {
    let target = string();
    let queue = json!({"threadId":target,"queuedRunId":string()});
    let mut edit = queue.clone();
    edit["text"] = json!({"type":"string","maxLength":100000});
    let mut reorder = queue.clone();
    reorder["beforeRunId"] = json!({"anyOf":[string(),{"type":"null"}]});
    let mut promote = queue.clone();
    promote["targetRunId"] = string();
    let point = json!({"anyOf":[{"type":"object","properties":{"type":{"type":"string","enum":["latest_stable"]}},"required":["type"]},{"type":"object","properties":{"type":{"type":"string","enum":["run"]},"runId":string()},"required":["type","runId"]},{"type":"object","properties":{"type":{"type":"string","enum":["checkpoint"]},"checkpointId":string()},"required":["type","checkpointId"]}]});
    let mut tools=vec![
        definition_write("t3_thread_transfer","Fork a conversation, merge a fork's context back to its parent, or read transfer status. Outside a thread, pass threadId. A merge is consumed by the target's next message.",json!({"action":{"type":"string","enum":["fork","merge_back","status"]},"threadId":string(),"targetThreadId":string(),"sourcePoint":point,"title":string()}),&["action"]),
        definition_write("t3_queue","Manage queued messages with action list, read, edit, cancel, reorder, or promote_to_steer. Read actions remain available to read-only clients.",json!({"action":{"type":"string","enum":["list","read","edit","cancel","reorder","promote_to_steer"]},"threadId":string(),"queuedRunId":string(),"text":{"type":"string","maxLength":100000},"beforeRunId":{"anyOf":[string(),{"type":"null"}]},"targetRunId":string(),"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),&["action"]),
        definition_write("t3_pending_request","List, read or respond to pending user questions. This cannot approve a permission request. Read actions remain available to read-only clients.",json!({"action":{"type":"string","enum":["list","read","respond"]},"threadId":string(),"requestId":string(),"answers":{"type":"object"}}),&["action"]),
        definition_write("t3_thread_send","Send a message through the existing dispatch lifecycle. auto steers a capable active turn, otherwise queues; queue waits behind an active turn. Reuse clientRequestId for retries. Restart replaces a running provider turn with another attempt of the same run when the provider supports it.",json!({"threadId":string(),"message":{"type":"string","minLength":1,"maxLength":120000},"mode":{"type":"string","enum":["auto","queue","steer","restart"]},"clientRequestId":{"type":"string","minLength":1,"maxLength":256}}),&["threadId","message"]),
        definition_write("t3_thread_interrupt","Interrupt the active or named run using the existing interrupt lifecycle. Reuse clientRequestId for retries.",json!({"threadId":string(),"runId":string(),"reason":{"type":"string","maxLength":2000},"clientRequestId":{"type":"string","minLength":1,"maxLength":256}}),&["threadId"]),
        definition("t3_thread_wait","Wait for the selected durable run. Timeout does not interrupt work; omit runId to select the latest run at call time.",json!({"threadId":string(),"runId":string(),"timeoutMs":{"type":"number"}}),&["threadId"]),
        definition_write("t3_thread_organize","Pin, snooze, settle, archive or mark a thread unread using existing lifecycle commands. External clients pass threadId.",json!({"threadId":string(),"action":{"type":"string","enum":["pin","unpin","snooze","unsnooze","settle","unsettle","archive","unarchive","mark_unread"]},"snoozedUntil":string()}),&["action"]),
        definition_write("t3_thread_configure","Change a thread's provider/model selection with the existing selection command. Permission modes are preserved. Omit modelSelection to read the current configuration.",json!({"threadId":string(),"modelSelection":{"type":"object","properties":{"instanceId":string(),"provider":string(),"model":string(),"options":{"anyOf":[{"type":"array","items":{"type":"object","properties":{"id":string(),"value":{"anyOf":[string(),{"type":"boolean"}]}},"required":["id","value"]}},{"type":"object"}]}},"required":["model"]}}),&[]),
        definition("t3_queue_list","List queued messages in delivery order with live offset pagination.",json!({"threadId":string(),"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),&[]),
        definition("t3_queue_read","Read up to 16,000 text units of a queued message.",queue.clone(),&["queuedRunId"]),
        definition_write("t3_queue_edit","Replace queued text, preserving its attachments; already-started messages are refused.",edit,&["queuedRunId","text"]),
        definition_write("t3_queue_cancel","Cancel a queued run through the normal queue command.",queue.clone(),&["queuedRunId"]),
        definition_write("t3_queue_reorder","Move a queued run before another, or to the end with beforeRunId=null.",reorder,&["queuedRunId","beforeRunId"]),
        definition_write("t3_queue_promote_to_steer","Deliver a queued message as steering to an active run using existing provider rules.",promote,&["queuedRunId","targetRunId"]),
        definition("t3_pending_request_list","List pending user questions, excluding permission approvals.",json!({"threadId":string()}),&[]),
        definition("t3_pending_request_read","Read a pending user question, excluding permission approvals.",json!({"threadId":string(),"requestId":string()}),&["requestId"]),
        definition_write("t3_pending_request_respond","Answer a pending user question. Cannot approve permissions.",json!({"threadId":string(),"requestId":string(),"answers":{"type":"object"}}),&["requestId","answers"]),
    ];
    for tool in &mut tools {
        if matches!(
            tool["name"].as_str(),
            Some("t3_thread_send" | "t3_pending_request_respond" | "t3_pending_request")
        ) {
            tool["annotations"]["openWorldHint"] = json!(true);
        }
    }
    tools
}
pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "t3_thread_transfer"
            | "t3_queue"
            | "t3_pending_request"
            | "t3_thread_send"
            | "t3_thread_interrupt"
            | "t3_thread_organize"
            | "t3_thread_configure"
            | "t3_queue_list"
            | "t3_queue_read"
            | "t3_queue_edit"
            | "t3_queue_cancel"
            | "t3_queue_reorder"
            | "t3_queue_promote_to_steer"
            | "t3_pending_request_list"
            | "t3_pending_request_read"
            | "t3_pending_request_respond"
    )
}
fn target<'a>(p: &'a Value, key: &str) -> Result<&'a str, Value> {
    p[key]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            failure(
                "target_required",
                &format!("Pass {key}: this MCP client is not running inside a T3 thread."),
            )
        })
}
fn command_id(authority: &Authority, name: &str, p: &Value) -> Result<String, Value> {
    let key = match p[if matches!(name, "t3_thread_send" | "t3_thread_interrupt") {
        "clientRequestId"
    } else {
        ""
    }]
    .as_str()
    {
        Some(key) => key.trim().to_owned(),
        None => random_id().map_err(|e| failure("orchestration_error", &e))?,
    };
    let material = serde_json::to_vec(&json!([authority.session_id, name, key]))
        .map_err(|e| failure("orchestration_error", &e.to_string()))?;
    Ok(format!("mcp:{:x}", Sha256::digest(material)))
}
pub(crate) fn call(s: &Arc<State>, authority: &Authority, name: &str, p: &Value) -> Reply {
    if name == "t3_thread_transfer" {
        let tid = target(p, "threadId")?;
        if p["action"] == "status" {
            let db = s.db.lock().unwrap();
            authority.session(&db, false)?;
            let thread = events::get(&db, "thread", tid).map_err(command_error)?;
            if !thread["deletedAt"].is_null() {
                return Err(failure("thread_not_found", "The thread was not found."));
            }
            let mut q=db.prepare("SELECT body FROM records WHERE kind='contextTransfers' AND thread_id=? ORDER BY rowid").map_err(|e|command_error(e.to_string()))?;
            let rows = q
                .query_map([tid], |r| r.get::<_, String>(0))
                .map_err(|e| command_error(e.to_string()))?;
            let mut transfers = vec![];
            for row in rows {
                let t: Value =
                    serde_json::from_str(&row.map_err(|e| command_error(e.to_string()))?)
                        .map_err(|e| command_error(e.to_string()))?;
                transfers.push(json!({"id":t["id"],"sourceThreadId":t["sourceThreadId"],"targetThreadId":t["targetThreadId"],"status":t["status"]}));
            }
            return Ok(json!({"transfers":transfers}));
        }
        let cid = random_id().map_err(command_error)?;
        let target_id = if p["action"] == "merge_back" {
            p["targetThreadId"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| failure("invalid_request", "targetThreadId is required."))?
                .to_owned()
        } else {
            format!("{cid}:fork")
        };
        let mut c = json!({"type":if p["action"]=="merge_back" {"thread.merge_back"}else{"thread.fork"},"commandId":cid,"sourceThreadId":tid,"targetThreadId":target_id,"sourcePoint":p.get("sourcePoint").cloned().unwrap_or(json!({"type":"latest_stable"})),"createdBy":"agent","creationSource":"mcp"});
        if let Some(title) = p["title"].as_str() {
            c["title"] = json!(title.trim());
        }
        let mut result = crate::thread_commands::dispatch_scoped(s, &c, Some(authority))
            .map_err(command_error)?;
        result["targetThreadId"] = json!(target_id);
        return Ok(result);
    }
    if matches!(name, "t3_queue" | "t3_pending_request") {
        let legacy = format!("{}_{}", name, p["action"].as_str().unwrap_or(""));
        crate::mcp_tools::validate(&legacy, p).map_err(|e| failure("invalid_request", &e))?;
        return call(s, authority, &legacy, p);
    }
    if name == "t3_thread_configure" && p.get("modelSelection").is_none() {
        authority.session(&s.reader.lock().unwrap(), false)?;
        return crate::mcp_tools::call(s, "t3_thread_configuration", p);
    }

    let mut normalized = p.clone();
    for key in [
        "threadId",
        "runId",
        "queuedRunId",
        "targetRunId",
        "beforeRunId",
        "requestId",
    ] {
        if let Some(text) = p[key].as_str() {
            normalized[key] = json!(text.trim());
        }
    }
    let p = &normalized;

    if matches!(
        name,
        "t3_queue_list" | "t3_queue_read" | "t3_pending_request_list" | "t3_pending_request_read"
    ) {
        return read(s, authority, name, p);
    }
    let tid = target(p, "threadId")?;
    let cid = command_id(authority, name, p)?;
    let mut c = json!({"threadId":tid,"commandId":cid});
    match name {
        "t3_thread_send" => {
            c["type"] = json!("message.dispatch");
            c["messageId"] = json!(format!("message-{cid}"));
            c["text"] = p["message"].clone();
            c["attachments"] = json!([]);
            match p["mode"].as_str().unwrap_or("auto") {
                "auto" | "steer" => {
                    c["deliveryIntent"] = json!(p["mode"].as_str().unwrap_or("auto"))
                }
                "queue" => c["dispatchMode"] = json!({"type":"queue_after_active"}),
                "restart" => c["deliveryIntent"] = json!("restart"),
                _ => return Err(failure("invalid_request", "Invalid delivery mode.")),
            }
        }
        "t3_thread_interrupt" => {
            c["type"] = json!("run.interrupt");
            if let Some(v) = p.get("runId") {
                c["runId"] = v.clone();
            }
            if let Some(v) = p.get("reason") {
                c["reason"] = v.clone();
            }
        }
        "t3_thread_organize" => {
            let action = p["action"].as_str().unwrap_or("");
            c["type"] = json!(if action == "mark_unread" {
                "thread.mark-unread".into()
            } else {
                format!("thread.{action}")
            });
            if action == "snooze" {
                c["snoozedUntil"] = p
                    .get("snoozedUntil")
                    .cloned()
                    .ok_or_else(|| failure("invalid_request", "snooze requires snoozedUntil."))?;
            }
        }
        "t3_thread_configure" => {
            c["type"] = json!("thread.model-selection.set");
            let mut selection = p["modelSelection"].clone();
            if selection.get("instanceId").is_none() {
                selection["instanceId"] = selection["provider"].clone();
            }
            if !selection["instanceId"].is_string() {
                return Err(failure(
                    "invalid_request",
                    "modelSelection.instanceId is required.",
                ));
            }
            for key in ["instanceId", "model"] {
                if let Some(text) = selection[key].as_str() {
                    selection[key] = json!(text.trim());
                }
            }
            if let Some(options) = selection["options"].as_object() {
                selection["options"] = json!(options
                    .iter()
                    .filter_map(|(id, value)| {
                        if id.trim().is_empty() {
                            return None;
                        }
                        match value {
                            Value::Bool(_) => Some(json!({"id":id.trim(),"value":value})),
                            Value::String(text) if !text.trim().is_empty() => {
                                Some(json!({"id":id.trim(),"value":text.trim()}))
                            }
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>());
            }
            selection
                .as_object_mut()
                .unwrap()
                .retain(|key, _| matches!(key.as_str(), "instanceId" | "model" | "options"));
            if let Some(options) = selection["options"].as_array_mut() {
                for option in options {
                    option["id"] = json!(option["id"].as_str().unwrap_or("").trim());
                    if let Some(value) = option["value"].as_str() {
                        option["value"] = json!(value.trim());
                    }
                }
            }
            c["modelSelection"] = selection;
        }
        "t3_pending_request_respond" => {
            c["type"] = json!("runtime-request.respond");
            c["requestId"] = p["requestId"].clone();
            c["answers"] = p["answers"].clone();
        }
        "t3_queue_edit" | "t3_queue_cancel" | "t3_queue_reorder" => {
            let action = name.strip_prefix("t3_queue_").unwrap();
            c["type"] = json!(format!("queued-run.{action}"));
            c["runId"] = p["queuedRunId"].clone();
            for k in ["text", "beforeRunId"] {
                if let Some(v) = p.get(k) {
                    c[k] = v.clone();
                }
            }
        }
        "t3_queue_promote_to_steer" => {
            c["type"] = json!("queued-message.promote-to-steer");
            c["queuedRunId"] = p["queuedRunId"].clone();
            c["targetRunId"] = p["targetRunId"].clone();
        }
        _ => return Err(failure("invalid_request", "Unknown tool.")),
    }
    let mut result = events::dispatch_scoped(s, &c, Some(authority)).map_err(command_error)?;
    if name == "t3_thread_send" {
        let run = events::get(
            &s.reader.lock().unwrap(),
            "runs",
            result["runId"].as_str().unwrap_or(""),
        )
        .map_err(command_error)?;
        result["status"] = run["status"].clone();
    }
    Ok(result)
}
fn read(s: &State, authority: &Authority, name: &str, p: &Value) -> Reply {
    let tid = target(p, "threadId")?;
    let db = s.reader.lock().unwrap();
    let tx = db
        .unchecked_transaction()
        .map_err(|e| failure("orchestration_error", &e.to_string()))?;
    authority.session(&tx, false)?;
    let t = events::get(&tx, "thread", tid)
        .ok()
        .filter(|t| t["deletedAt"].is_null())
        .ok_or_else(|| failure("thread_not_found", "The thread was not found."))?;
    let _ = t;
    if name.starts_with("t3_queue_") {
        let mut q=tx.prepare("SELECT body FROM records WHERE kind='runs' AND thread_id=? AND json_extract(body,'$.status')='queued' ORDER BY coalesce(json_extract(body,'$.queuePosition'),1e18),rowid").map_err(|e|failure("orchestration_error",&e.to_string()))?;
        let rows = q
            .query_map([tid], |r| r.get::<_, String>(0))
            .map_err(|e| failure("orchestration_error", &e.to_string()))?;
        let cursor = crate::mcp_tools::number(p, "cursor", 0);
        let limit = crate::mcp_tools::number(p, "limit", 20);
        let mut items = Vec::new();
        let mut count = 0;
        for body in rows {
            let run: Value = serde_json::from_str(
                &body.map_err(|e| failure("orchestration_error", &e.to_string()))?,
            )
            .map_err(|e| failure("orchestration_error", &e.to_string()))?;
            let selected = name == "t3_queue_read" && run["id"] == p["queuedRunId"];
            if selected || (name == "t3_queue_list" && count >= cursor && items.len() < limit) {
                let m = events::get(&tx, "messages", run["userMessageId"].as_str().unwrap_or(""))
                    .map_err(|_| {
                    failure(
                        "orchestration_error",
                        "The queued message could not be read.",
                    )
                })?;
                let text = m["text"].as_str().unwrap_or("");
                let max = if name == "t3_queue_read" { 16000 } else { 1000 };
                let (text, next) = crate::mcp_tools::text_page(text, 0, max);
                let entry = json!({"queuedRunId":run["id"],"text":text,"truncated":next.is_some()});
                if selected {
                    return Ok(entry);
                }
                items.push(entry);
            }
            count += 1;
        }
        return if name == "t3_queue_read" {
            Err(failure(
                "invalid_request",
                "The queued message was not found.",
            ))
        } else {
            let end = cursor.saturating_add(items.len());
            Ok(json!({"items":items,"nextCursor":if end<count{json!(end)}else{Value::Null}}))
        };
    }
    let mut q=tx.prepare("SELECT body FROM records WHERE kind='runtimeRequests' AND thread_id=? AND json_extract(body,'$.kind')='user_input' AND json_extract(body,'$.status')='pending' ORDER BY rowid").map_err(|e|failure("orchestration_error",&e.to_string()))?;
    let mut rows = q
        .query([tid])
        .map_err(|e| failure("orchestration_error", &e.to_string()))?;
    let mut ids = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| failure("orchestration_error", &e.to_string()))?
    {
        let body: String = row
            .get(0)
            .map_err(|e| failure("orchestration_error", &e.to_string()))?;
        let r: Value = serde_json::from_str(&body)
            .map_err(|e| failure("orchestration_error", &e.to_string()))?;
        if name == "t3_pending_request_read" && r["id"] == p["requestId"] {
            let item_body: String=tx.query_row("SELECT body FROM records WHERE kind='turnItems' AND thread_id=? AND json_extract(body,'$.type')='user_input_request' AND json_extract(body,'$.requestId')=? ORDER BY rowid DESC LIMIT 1",rusqlite::params![tid,r["id"].as_str()],|r|r.get(0)).map_err(|_|failure("invalid_request","The pending question was not found."))?;
            let item: Value = serde_json::from_str(&item_body)
                .map_err(|_| failure("orchestration_error", "Unable to read the question."))?;
            return Ok(json!({"requestId":r["id"],"questions":item["questions"]}));
        }
        ids.push(r["id"].clone());
    }
    if name == "t3_pending_request_read" {
        Err(failure(
            "invalid_request",
            "The pending question was not found.",
        ))
    } else {
        Ok(json!({"requestIds":ids}))
    }
}

pub(crate) async fn wait(s: Arc<State>, authority: Authority, p: Value) -> Reply {
    use rusqlite::OptionalExtension;
    use std::time::{Duration, Instant};
    let tid = target(&p, "threadId")?.to_owned();
    let mut selected = p["runId"].as_str().map(|s| s.trim().to_owned());
    let timeout = p["timeoutMs"]
        .as_f64()
        .unwrap_or(600_000.0)
        .clamp(1.0, 3_600_000.0);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout / 1000.0);
    let mut changed = s.changed.subscribe();
    loop {
        let state = s.clone();
        let a = authority.clone();
        let target = tid.clone();
        let run_id = selected.clone();
        let run=tokio::task::spawn_blocking(move|| -> Result<Option<Value>,Value> {
            let db=state.reader.lock().unwrap();let tx=db.unchecked_transaction().map_err(|e|failure("orchestration_error",&e.to_string()))?;
            a.session(&tx,false)?;
            let _=events::get(&tx,"thread",&target).ok().filter(|t|t["deletedAt"].is_null()).ok_or_else(||failure("thread_not_found","The thread was not found."))?;
            if let Some(rid)=run_id {
                let run=events::get(&tx,"runs",&rid).ok().filter(|r|r["threadId"]==target).ok_or_else(||failure("invalid_request","The run was not found in this thread."))?;
                Ok(Some(run))
            }else{
                let body:Option<String>=tx.query_row("SELECT body FROM records WHERE kind='runs' AND thread_id=? ORDER BY json_extract(body,'$.ordinal') DESC,rowid DESC LIMIT 1",[&target],|r|r.get(0)).optional().map_err(|e|failure("orchestration_error",&e.to_string()))?;
                body.map(|b|serde_json::from_str(&b).map_err(|e|failure("orchestration_error",&e.to_string()))).transpose()
            }
        }).await.map_err(|_|failure("orchestration_error","Unable to read the run."))??;
        let Some(run) = run else {
            return Ok(json!({"threadId":tid,"runId":null,"status":"idle","timedOut":false}));
        };
        selected = run["id"].as_str().map(str::to_owned);
        let status = run["status"].as_str().unwrap_or("idle");
        let terminal = matches!(
            status,
            "completed" | "interrupted" | "failed" | "cancelled" | "rolled_back"
        );
        if terminal || Instant::now() >= deadline {
            return Ok(
                json!({"threadId":tid,"runId":selected,"status":status,"timedOut":!terminal}),
            );
        }
        tokio::select! {_=changed.changed()=>{},_=tokio::time::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(1)))=>{}}
    }
}
