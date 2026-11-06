//! ThreadMessageIntake: claim uploads outside SQLite's write lock, then admit
//! through the normal command/receipt transactions and retain only accepted copies.
use crate::{
    attachment_claims::{self, Claims},
    events,
    mcp_access::Authority,
    store::State,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::sync::Arc;
type Result<T> = std::result::Result<T, String>;
fn remap(context: &mut Value, before: &Value, after: &Value) {
    if let Some(records) = context.get_mut("records").and_then(Value::as_array_mut) {
        for record in records {
            if !matches!(record["kind"].as_str(), Some("image" | "file")) {
                continue;
            }
            if let Some(index) = before
                .as_array()
                .into_iter()
                .flatten()
                .position(|a| a["id"] == record["attachmentId"])
            {
                if let Some(id) = after.get(index).and_then(|v| v.get("id")) {
                    record["attachmentId"] = id.clone();
                }
            }
        }
    }
}
fn receipt(s: &State, c: &Value, a: Option<&Authority>) -> Result<bool> {
    let db = s.db.lock().unwrap();
    if let Some(a) = a {
        a.check(&db, c)?;
    }
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM receipts WHERE command_id=?)",
        [c["commandId"].as_str().ok_or("commandId required")?],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
fn accepted(s: &State, c: &Value) -> Result<Value> {
    let db = s.db.lock().unwrap();
    let tid = c["threadId"].as_str().unwrap_or("");
    let (kind, id) = match c["type"].as_str() {
        Some("queued-run.edit") => {
            let run = events::get(&db, "runs", c["runId"].as_str().unwrap_or(""))?;
            (
                "messages",
                run["userMessageId"].as_str().unwrap_or("").to_owned(),
            )
        }
        Some("runtime-request.respond") => (
            "runtimeRequests",
            c["requestId"].as_str().unwrap_or("").to_owned(),
        ),
        _ => ("messages", c["messageId"].as_str().unwrap_or("").to_owned()),
    };
    let body: Option<String> = db
        .query_row(
            "SELECT body FROM records WHERE kind=?1 AND id=?2 AND thread_id=?3",
            rusqlite::params![kind, id, tid],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    body.map(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
        .transpose()
        .map(|v| v.unwrap_or(Value::Null))
}
pub(crate) fn dispatch(s: &Arc<State>, c: &Value, a: Option<&Authority>) -> Result<Value> {
    let kind = c["type"].as_str().unwrap_or("");
    let handles = kind == "message.dispatch"
        || (kind == "queued-run.edit" && c.get("attachments").is_some())
        || (kind == "runtime-request.respond" && c.get("attachmentsByQuestionId").is_some());
    if !handles || receipt(s, c, a)? {
        return events::dispatch_prepared(s, c, a);
    }
    let tid = c["threadId"].as_str().ok_or("threadId required")?;
    let mut prepared = c.clone();
    let mut claims = Claims::empty();
    if kind == "runtime-request.respond" {
        let by_question = c["attachmentsByQuestionId"]
            .as_object()
            .ok_or("attachmentsByQuestionId must be an object")?;
        let all = by_question
            .values()
            .map(|v| {
                v.as_array()
                    .ok_or_else(|| "Question attachments must be arrays".to_owned())
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        attachment_claims::validate(&json!(all))?;
        for (question, attachments) in by_question {
            let claimed = attachment_claims::claim(s, tid, attachments)?;
            prepared["attachmentsByQuestionId"][question] = claimed.attachments.clone();
            claims.absorb(claimed);
        }
        append_paths(s, &mut prepared)?;
    } else {
        let before = c.get("attachments").cloned().unwrap_or(json!([]));
        let claimed = attachment_claims::claim(s, tid, &before)?;
        if let Some(context) = prepared.get_mut("context") {
            remap(context, &before, &claimed.attachments);
        }
        prepared["attachments"] = claimed.attachments.clone();
        claims.absorb(claimed);
    }
    let result = events::dispatch_prepared(s, &prepared, a);
    match accepted(s, &prepared) {
        Ok(value) => claims.retain_used(&value),
        Err(_) => claims.keep(),
    };
    result
}
fn append_paths(s: &State, c: &mut Value) -> Result<()> {
    let mut answers = c.get("answers").cloned().unwrap_or(json!({}));
    if !answers.is_object() {
        return Err("Answers must be an object".into());
    }
    for (question, attachments) in c["attachmentsByQuestionId"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let mut references = Vec::new();
        for attachment in attachments.as_array().into_iter().flatten() {
            let path = crate::assets::attachment_path(s, attachment["id"].as_str().unwrap_or(""))
                .ok_or("Question attachment is no longer available; attach it again")?;
            let extracted = s
                .document_text
                .ensure(&path)
                .map(|p| format!(" (text extracted to: {})", json!(p)))
                .unwrap_or_default();
            references.push(format!(
                "Attached {} {}: {}{extracted}",
                attachment["type"].as_str().unwrap_or("file"),
                json!(attachment["name"]),
                json!(path)
            ));
        }
        if references.is_empty() {
            continue;
        }
        let text = references.join("\n");
        match answers.get_mut(question) {
            Some(Value::Array(values)) => values.push(json!(text)),
            Some(Value::String(value)) if !value.is_empty() => {
                value.push_str("\n\n");
                value.push_str(&text);
            }
            _ => answers[question] = json!(text),
        }
    }
    c["answers"] = answers;
    Ok(())
}
pub(crate) fn launch(s: &Arc<State>, p: &Value, a: Option<&Authority>) -> Result<Value> {
    if let Some(a) = a {
        a.check_launch(&s.db.lock().unwrap(), p)?;
    }
    let Some(initial) = p.get("initialMessage") else {
        return crate::thread_launch::launch_prepared(s, p, a);
    };
    let before = &initial["attachments"];
    attachment_claims::validate(before)?;
    let accepted = {
        let db = s.db.lock().unwrap();
        db.query_row(
            "SELECT run_id IS NOT NULL FROM launch_claims WHERE command_id=?",
            [p["commandId"].as_str().unwrap_or("")],
            |r| r.get::<_, bool>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM receipts WHERE command_id=?)",
                [p["commandId"].as_str().unwrap_or("")],
                |r| r.get(0),
            )
            .unwrap_or(false)
        })
    };
    if accepted {
        return crate::thread_launch::launch_prepared(s, p, a);
    }
    let has_pending = before
        .as_array()
        .into_iter()
        .flatten()
        .any(attachment_claims::pending);
    let tid = p["threadId"].as_str().unwrap_or("");
    if has_pending && tid.is_empty() {
        return Err("Uploaded attachments need a thread id at launch".into());
    }
    let mut claims = attachment_claims::claim(s, tid, before)?;
    let mut prepared = p.clone();
    prepared["initialMessage"]["attachments"] = claims.attachments.clone();
    if let Some(context) = prepared["initialMessage"].get_mut("context") {
        remap(context, before, &claims.attachments);
    }
    let result = crate::thread_launch::launch_prepared(s, &prepared, a);
    let command = json!({"type":"message.dispatch","threadId":tid,"messageId":initial["messageId"].as_str().map(str::to_owned).unwrap_or(format!("message-{}",p["commandId"].as_str().unwrap_or("")))});
    match self::accepted(s, &command) {
        Ok(value) => claims.retain_used(&value),
        Err(_) => claims.keep(),
    };
    result
}
