//! Native webhook receipt reads, including the shipped clients' outcome opt-in.
use super::{failure, Reply};
use crate::store::State;
use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn summary(row: &Row<'_>) -> rusqlite::Result<Value> {
    let headers = headers(row)?;
    let outcome: String = row.get("outcome")?;
    let missing: Vec<String> =
        serde_json::from_str(&row.get::<_, String>("missing_fields_json")?).unwrap_or_default();
    Ok(json!({
        "id":row.get::<_, String>("delivery_id")?,
        "taskId":row.get::<_, String>("task_id")?,
        "receivedAt":row.get::<_, String>("received_at")?,
        "method":row.get::<_, String>("method")?,
        "contentType":headers.get("content-type"),
        "bodyBytes":row.get::<_, i64>("body_bytes")?,
        "pendingWindow":outcome=="accepted" && row.get::<_,i64>("pending_window")?==1,
        "outcome":outcome,
        "signatureVerified":row.get::<_,i64>("signature_verified")?==1,
        "missingFields":missing,
        "error":row.get::<_, Option<String>>("error")?,
    }))
}

fn headers(row: &Row<'_>) -> rusqlite::Result<BTreeMap<String, String>> {
    Ok(serde_json::from_str(&row.get::<_, String>("headers_json")?).unwrap_or_default())
}

fn legacy(outcome: &str) -> bool {
    matches!(
        outcome,
        "accepted"
            | "dispatch_failed"
            | "rejected_signature"
            | "disabled"
            | "rate_limited"
            | "expired"
    )
}

fn id(input: &Value) -> Result<&str, Value> {
    input["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| failure("id is required", None))
}

pub(super) fn list(state: &State, input: &Value) -> Reply {
    let id = id(input)?;
    let read = || -> rusqlite::Result<Vec<Value>> {
        let db = state.reader.lock().unwrap();
        // Summary reads need no body/prompt bytes, even for a large delivery log.
        let mut query = db.prepare("SELECT d.delivery_id,d.task_id,d.received_at,d.method,d.headers_json,d.body_bytes,d.outcome,d.signature_verified,d.missing_fields_json,d.error,EXISTS(SELECT 1 FROM scheduled_task_webhook_pending p WHERE p.delivery_id=d.delivery_id AND p.state IN ('pending','dispatching')) AS pending_window FROM scheduled_task_webhook_deliveries d WHERE d.task_id=?1 ORDER BY d.received_at DESC,d.rowid DESC")?;
        let rows = query.query_map([id], summary)?;
        let mut deliveries = vec![];
        for row in rows {
            let row = row?;
            if input["acceptAnyOutcome"] == true || legacy(row["outcome"].as_str().unwrap_or("")) {
                deliveries.push(row);
            }
        }
        Ok(deliveries)
    };
    read()
        .map(|deliveries| json!({"deliveries":deliveries}))
        .map_err(|_| failure("Could not list webhook deliveries.", Some(id)))
}

pub(super) fn get(state: &State, input: &Value) -> Reply {
    let id = id(input)?;
    let delivery = input["deliveryId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| failure("deliveryId is required", Some(id)))?;
    let db = state.reader.lock().unwrap();
    let row = db.query_row("SELECT d.*,EXISTS(SELECT 1 FROM scheduled_task_webhook_pending p WHERE p.delivery_id=d.delivery_id AND p.state IN ('pending','dispatching')) AS pending_window FROM scheduled_task_webhook_deliveries d WHERE d.task_id=?1 AND d.delivery_id=?2",params![id,delivery],|row| {
        let mut value=summary(row)?;
        value["query"]=json!(row.get::<_,String>("query")?);
        value["headers"]=json!(headers(row)?);
        value["body"]=json!(row.get::<_,String>("body")?);
        value["bodyTruncated"]=json!(row.get::<_,i64>("body_truncated")?==1);
        value["renderedPrompt"]=json!(row.get::<_,Option<String>>("rendered_prompt")?);
        Ok(value)
    }).optional().map_err(|_|failure("Could not load webhook delivery.",Some(id)))?;
    match row {
        Some(delivery)
            if input["acceptAnyOutcome"] == true
                || legacy(delivery["outcome"].as_str().unwrap_or("")) =>
        {
            Ok(json!({"delivery":delivery}))
        }
        _ => Err(failure("Webhook delivery not found.", Some(id))),
    }
}
