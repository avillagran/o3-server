//! MCP attachment toolkit: signed pending uploads, caller-owned discard and
//! canonical thread references delivered through shared ThreadMessageIntake.
use crate::{
    events,
    mcp_access::{command_error, Authority},
    mcp_tools::{definition, failure},
    store::{now_ms, random_id, State},
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
type Reply = Result<Value, Value>;
#[derive(Default)]
pub(crate) struct Registry {
    owners: Mutex<HashMap<String, (String, i64)>>,
}
fn string() -> Value {
    json!({"type":"string","minLength":1})
}
pub(crate) fn input_schema() -> Value {
    let mut attachment = json!({"type":"object","properties":{"type":{"type":"string","enum":["image","file"]},"id":string(),"name":{"type":"string","minLength":1,"maxLength":255},"mimeType":{"type":"string","minLength":1,"maxLength":100},"sizeBytes":{"type":"integer","minimum":0,"maximum":52428800},"source":{"type":"object","properties":{"_tag":{"type":"string","enum":["pasted-text"]}},"required":["_tag"]}},"required":["type","id","name","mimeType","sizeBytes"]});
    attachment["properties"]["id"]["maxLength"] = json!(128);
    let mut image = attachment.clone();
    image["properties"]["type"]["enum"] = json!(["image"]);
    image["properties"]["mimeType"]["pattern"] = json!("(?i)^image/");
    image["properties"]["sizeBytes"]["maximum"] = json!(10 * 1024 * 1024);
    image["properties"]
        .as_object_mut()
        .unwrap()
        .remove("source");
    attachment["properties"]["type"]["enum"] = json!(["file"]);
    attachment["properties"]["sizeBytes"]["minimum"] = json!(1);
    json!({"anyOf":[image,attachment]})
}
pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "t3_attachment_prepare_upload" | "t3_attachment_discard" | "t3_thread_send_attachments"
    )
}
pub(crate) fn catalog() -> Vec<Value> {
    let file = json!({"type":"object","properties":{"type":{"type":"string","enum":["file"]},"name":{"type":"string","minLength":1,"maxLength":255},"mimeType":{"type":"string","minLength":1,"maxLength":100},"sizeBytes":{"type":"integer","minimum":1,"maximum":52428800}},"required":["type","name","mimeType","sizeBytes"]});
    let mut image = file.clone();
    image["properties"]["type"]["enum"] = json!(["image"]);
    image["properties"]["mimeType"] =
        json!({"type":"string","enum":["image/gif","image/jpeg","image/png","image/webp"]});
    image["properties"]["sizeBytes"]["maximum"] = json!(10 * 1024 * 1024);
    image["required"] = json!(["name", "mimeType", "sizeBytes"]);
    let upload = json!({"anyOf":[image,file]});
    let mut tools=vec![
        definition("t3_attachment_prepare_upload","Get an upload URL. POST the file's exact bytes to relativeUrl on this MCP server's HTTP origin, then send the returned attachmentId with t3_thread_send_attachments.",json!({"upload":upload}),&["upload"]),
        definition("t3_attachment_discard","Discard a pending upload prepared by this caller; sent attachments remain.",json!({"attachmentId":{"type":"string","minLength":1,"maxLength":256}}),&["attachmentId"]),
        definition("t3_thread_send_attachments","Send uploaded attachments as a new message to a thread. No retry key. Omit threadId for the calling thread. Modes cannot exceed the caller. Success does not mean the provider read the files or finished.",json!({"threadId":string(),"message":{"type":"string","maxLength":120000},"attachments":{"type":"array","minItems":1,"maxItems":8,"items":input_schema()}}),&["attachments"])
    ];
    for tool in &mut tools {
        tool["annotations"] = json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":tool["name"]=="t3_thread_send_attachments"});
    }
    tools
}
pub(crate) fn normalize(p: Value) -> Value {
    let mut p = crate::mcp_launch::normalize(p);
    for key in ["threadId", "attachmentId"] {
        if let Some(text) = p[key].as_str() {
            p[key] = json!(text.trim());
        }
    }
    if let Some(upload) = p.get_mut("upload").and_then(Value::as_object_mut) {
        upload.retain(|key, _| matches!(key.as_str(), "type" | "name" | "mimeType" | "sizeBytes"));
        for value in upload.values_mut() {
            if let Some(text) = value.as_str() {
                *value = json!(text.trim());
            }
        }
    }
    p
}
pub(crate) fn call(s: &Arc<State>, a: &Authority, name: &str, p: &Value) -> Reply {
    a.launch_context(&s.db.lock().unwrap())?;
    let owner = a
        .native_thread()
        .map(str::to_owned)
        .unwrap_or_else(|| a.request_namespace());
    match name {
        "t3_attachment_prepare_upload" => {
            let result = crate::assets::create_upload_url(s, &p["upload"]).map_err(|_| {
                failure(
                    "orchestration_error",
                    "Unable to prepare attachment upload.",
                )
            })?;
            let mut owners = s.attachment_uploads.owners.lock().unwrap();
            owners.retain(|_, (_, issued)| {
                now_ms() - *issued <= 24 * 60 * 60 * 1000 + 10 * 60 * 1000
            });
            owners.insert(
                result["attachmentId"].as_str().unwrap().into(),
                (owner, now_ms()),
            );
            Ok(result)
        }
        "t3_attachment_discard" => {
            let id = p["attachmentId"].as_str().unwrap_or("");
            let mut owners = s.attachment_uploads.owners.lock().unwrap();
            if !owners.get(id).is_some_and(|(who, _)| who == &owner) {
                return Err(failure(
                    "invalid_request",
                    "Only the caller that prepared a pending upload can discard it.",
                ));
            }
            crate::assets::delete_attachment(s, &json!({"attachmentId":id})).map_err(|_| {
                failure(
                    "orchestration_error",
                    "Unable to discard attachment upload.",
                )
            })?;
            owners.remove(id);
            Ok(json!({}))
        }
        "t3_thread_send_attachments" => send(s, a, p),
        _ => Err(failure("invalid_request", "Unknown attachment tool.")),
    }
}
fn send(s: &Arc<State>, a: &Authority, p: &Value) -> Reply {
    let tid = p["threadId"]
        .as_str()
        .or(a.native_thread())
        .ok_or_else(|| {
            failure(
                "target_required",
                "Pass threadId: this client is not running inside a T3 thread.",
            )
        })?;
    let mut attachments = Vec::new();
    {
        let db = s.db.lock().unwrap();
        a.check(&db, &json!({"type":"message.dispatch","threadId":tid}))
            .map_err(command_error)?;
        for attachment in p["attachments"].as_array().into_iter().flatten() {
            if crate::attachment_claims::pending(attachment) {
                attachments.push(attachment.clone());
                continue;
            }
            let body:Option<String>=db.query_row("SELECT a.value FROM records m,json_each(m.body,'$.attachments') a WHERE m.kind='messages' AND m.thread_id=?1 AND json_extract(a.value,'$.id')=?2 LIMIT 1",rusqlite::params![tid,attachment["id"].as_str()],|r|r.get(0)).optional().map_err(|e|command_error(e.to_string()))?;
            let canonical=body.and_then(|s|serde_json::from_str::<Value>(&s).ok()).ok_or_else(||failure("invalid_request","Attachments must be pending uploads or already belong to the target thread."))?;
            attachments.push(canonical);
        }
    }
    let cid = format!("mcp:{}", random_id().map_err(command_error)?);
    let mut c = json!({"type":"message.dispatch","commandId":cid,"threadId":tid,"messageId":cid,"text":p["message"].as_str().unwrap_or(""),"attachments":attachments,"deliveryIntent":"auto"});
    if let Some(sender) = a.native_thread() {
        c["senderThreadId"] = json!(sender);
    }
    events::dispatch_scoped(s, &c, Some(a)).map_err(command_error)?;
    let db = s.db.lock().unwrap();
    let message = events::get(&db, "messages", &cid).map_err(command_error)?;
    let run =
        events::get(&db, "runs", message["runId"].as_str().unwrap_or("")).map_err(command_error)?;
    Ok(json!({"threadId":tid,"messageId":cid,"runId":run["id"],"status":run["status"]}))
}
