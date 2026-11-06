//! Native webhook admission, receipt retention and pending-window dispatch.
use crate::{
    events::get,
    store::{iso, now_ms, random_id, State},
    webhook_dedupe::{self as dedupe, Admission, Error, Outcome},
    webhook_payload::{self, Rendered, Request},
};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{collections::HashMap, sync::Arc};

// A receipt can prove admission while imported preparation still needs an owner.
// Keep that delivery parked, preserving replay claims, until recovery resolves it.
const RECOVERABLE: &str = "(p.state IN ('completed','failed') OR NOT EXISTS(SELECT 1 FROM o3_shared_admission_recovery recovery JOIN o3_command_admissions admission ON admission.run_id=recovery.run_id JOIN records run ON run.kind='runs' AND run.id=recovery.run_id WHERE recovery.status IN ('blocked','export_pending','releasing') AND json_extract(run.body,'$.status') IN ('preparing','starting','queued') AND admission.command_id IN ('scheduled-task:'||p.task_id||':webhook:'||p.delivery_id,'scheduled-task:'||p.task_id||':webhook:'||p.delivery_id||':initial-message')))";

#[derive(Default)]
struct Window {
    accepted: Vec<(u64, i64)>,
    rejected: bool,
}
#[derive(Default)]
pub(crate) struct AdmissionState {
    next: u64,
    runs: HashMap<String, Window>,
    rejections: HashMap<String, Window>,
    deduped: HashMap<String, Vec<i64>>,
    filtered: HashMap<String, Vec<i64>>,
}
fn slot(window: &mut Window, now: i64, reservation: u64) -> Result<(), Error> {
    window
        .accepted
        .retain(|(_, at)| now.saturating_sub(*at) < 60_000);
    if window.accepted.len() >= 60 {
        let first = !window.rejected;
        window.rejected = true;
        return Err(if first {
            Error::FirstRejected
        } else {
            Error::Rejected
        });
    }
    window.accepted.push((reservation, now));
    window.rejected = false;
    Ok(())
}
fn log_slot(windows: &mut HashMap<String, Vec<i64>>, id: &str, now: i64) -> bool {
    let recent = windows.entry(id.into()).or_default();
    recent.retain(|at| now.saturating_sub(*at) < 60_000);
    if recent.len() >= 10 {
        false
    } else {
        recent.push(now);
        true
    }
}

pub(crate) struct Response {
    pub status: u16,
    pub outcome: &'static str,
    pub body: Value,
}
impl Response {
    pub(crate) fn error(status: u16, outcome: &'static str, error: &'static str) -> Self {
        Self {
            status,
            outcome,
            body: json!({"error":error}),
        }
    }
    fn accepted(id: &str, outcome: &'static str) -> Self {
        Self {
            status: 202,
            outcome,
            body: json!({"deliveryId":id}),
        }
    }
}

fn text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}
fn logged_body(body: &[u8], request: &Request) -> String {
    if body.len() <= 65536 {
        return request.body_text.clone();
    }
    let prefix = &body[..65536];
    let mut at = 0;
    let mut end = prefix.len();
    while at < prefix.len() {
        match std::str::from_utf8(&prefix[at..]) {
            Ok(_) => break,
            Err(error) => match error.error_len() {
                Some(len) => at += error.valid_up_to() + len,
                None => {
                    end = at + error.valid_up_to();
                    break;
                }
            },
        }
    }
    text(&prefix[..end])
}
fn logged_headers(request: &Request) -> String {
    let headers = request
        .headers
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                if webhook_payload::credential(key) {
                    "[redacted]".into()
                } else {
                    value.clone()
                },
            )
        })
        .collect::<indexmap::IndexMap<String, String>>();
    serde_json::to_string(&headers).expect("string headers serialize")
}
fn replace_request(db: &Connection, id: &str, request: &Request, body: &[u8]) -> Result<(), Error> {
    db.execute("UPDATE scheduled_task_webhook_deliveries SET method=?1,query=?2,headers_json=?3,body=?4,body_bytes=?5,body_truncated=?6 WHERE delivery_id=?7",params![request.method,webhook_payload::redact_query(&request.query),logged_headers(request),logged_body(body,request),body.len() as i64,i64::from(body.len()>65536),id])?;
    Ok(())
}
#[derive(Clone, Copy)]
struct Receipt<'a> {
    task: &'a str,
    id: &'a str,
    now: i64,
    request: &'a Request,
    body: &'a [u8],
}
impl Receipt<'_> {
    fn record(
        &self,
        db: &Connection,
        outcome: &str,
        verified: bool,
        rendered: Option<&Rendered>,
        error: Option<&str>,
    ) -> Result<(), Error> {
        db.execute_batch("SAVEPOINT native_webhook_receipt")?;
        let result = self.write(db, outcome, verified, rendered, error);
        db.execute_batch(if result.is_ok() {
            "RELEASE native_webhook_receipt"
        } else {
            "ROLLBACK TO native_webhook_receipt; RELEASE native_webhook_receipt"
        })?;
        result
    }
    fn write(
        &self,
        db: &Connection,
        outcome: &str,
        verified: bool,
        rendered: Option<&Rendered>,
        error: Option<&str>,
    ) -> Result<(), Error> {
        let Self {
            task,
            id,
            now,
            request,
            body,
        } = *self;
        let missing = rendered.map(|r| r.missing.as_slice()).unwrap_or_default();
        let prompt = rendered
            .map(|r| r.prompt.encode_utf16().take(65536).collect::<Vec<_>>())
            .map(|units| String::from_utf16_lossy(&units));
        db.execute("INSERT INTO scheduled_task_webhook_deliveries(delivery_id,task_id,received_at,method,query,headers_json,body,body_bytes,body_truncated,outcome,signature_verified,missing_fields_json,rendered_prompt,error) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14 WHERE EXISTS(SELECT 1 FROM records WHERE kind='scheduledTasks' AND id=?2) ON CONFLICT(delivery_id) DO UPDATE SET outcome=excluded.outcome,signature_verified=excluded.signature_verified,missing_fields_json=excluded.missing_fields_json,rendered_prompt=excluded.rendered_prompt,error=excluded.error",params![id,task,iso(now),request.method,webhook_payload::redact_query(&request.query),logged_headers(request),logged_body(body,request),body.len() as i64,i64::from(body.len()>65536),outcome,i64::from(verified),serde_json::to_string(missing)?,prompt,error])?;
        let bucket = if outcome == "filtered" {
            "filtered"
        } else if matches!(outcome, "duplicate" | "coalesced" | "rate_limited")
            || error == Some("The filled-in prompt is too long.")
        {
            "dedupe"
        } else {
            "main"
        };
        db.execute("DELETE FROM scheduled_task_webhook_deliveries WHERE task_id=?1 AND (CASE WHEN outcome='filtered' THEN 'filtered' WHEN outcome IN ('duplicate','coalesced','rate_limited') OR error='The filled-in prompt is too long.' THEN 'dedupe' ELSE 'main' END)=?2 AND delivery_id NOT IN (SELECT delivery_id FROM scheduled_task_webhook_deliveries WHERE task_id=?1 AND (CASE WHEN outcome='filtered' THEN 'filtered' WHEN outcome IN ('duplicate','coalesced','rate_limited') OR error='The filled-in prompt is too long.' THEN 'dedupe' ELSE 'main' END)=?2 ORDER BY received_at DESC,rowid DESC LIMIT ?3)",params![task,bucket,if bucket=="main"{50}else{20}])?;
        Ok(())
    }
}

fn verify(signature: &Value, credentials: &Value, request: &Request, body: &[u8]) -> bool {
    let Some(secret) = credentials["secret"].as_str() else {
        return false;
    };
    // Native Mac.key rejects empty key material before verification.
    if secret.is_empty() {
        return false;
    }
    let name = signature["header"].as_str().unwrap_or("").to_lowercase();
    let Some(value) = request.headers.get(&name) else {
        return false;
    };
    let prefix = signature["prefix"].as_str().unwrap_or("");
    let value = value.trim_matches(webhook_payload::js_space);
    if !value.to_lowercase().starts_with(&prefix.to_lowercase()) {
        return false;
    }
    let candidate = String::from_utf16_lossy(
        &value
            .encode_utf16()
            .skip(prefix.encode_utf16().count())
            .collect::<Vec<_>>(),
    );
    let decoded = if signature["encoding"] == "hex" {
        crate::auth::decode_hex(&candidate)
    } else {
        crate::auth::decode_b64(&candidate)
    };
    let Some(decoded) = decoded else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(body);
    mac.verify_slice(&decoded).is_ok()
}
pub(crate) fn accept(
    s: &Arc<State>,
    id: &str,
    token: &str,
    method: &str,
    headers: &hyper::HeaderMap,
    query: &str,
    body: &[u8],
) -> Response {
    if !crate::webhook::admits(s, id, token) {
        return Response::error(404, "not_found", "hook_not_found");
    }
    let mut request = Request {
        method: method.into(),
        path: format!("/api/hooks/{}", crate::webhook::encode_component(id)),
        query: query.into(),
        headers: Default::default(),
        body_text: text(body),
    };
    for (name, value) in headers {
        let name = name.as_str().to_owned();
        let value = value
            .as_bytes()
            .iter()
            .map(|b| char::from(*b))
            .collect::<String>();
        request
            .headers
            .entry(name.clone())
            .and_modify(|old| {
                old.push_str(if name == "cookie" { "; " } else { ", " });
                old.push_str(&value)
            })
            .or_insert(value);
    }
    let mut rates = s.streams.ops.native_webhooks.lock().unwrap();
    let mut db = s.db.lock().unwrap();
    let result = (|| -> Result<Response, Error> {
        let task = get(&db, "scheduledTasks", id).map_err(Error::Persistence)?;
        if task["schedule"]["type"] != "webhook" {
            return Ok(Response::error(404, "not_found", "hook_not_found"));
        }
        if task["blockedByPolicy"].is_string() {
            return Ok(Response::error(
                403,
                "blocked_by_policy",
                "blocked_by_policy",
            ));
        }
        let credentials = get(&db, "hookCredentials", id).map_err(Error::Persistence)?;
        // Recheck after acquiring the writer: rotation cannot revive a retired URL.
        if credentials["token"] != token {
            return Ok(Response::error(404, "not_found", "hook_not_found"));
        }
        let now = now_ms();
        let delivery = format!("delivery:{}", random_id().map_err(Error::Persistence)?);
        let receipt = Receipt {
            task: id,
            id: &delivery,
            now,
            request: &request,
            body,
        };
        rates.next = rates.next.wrapping_add(1);
        let reservation = rates.next;
        let tx = db.transaction()?;
        let rejected = slot(
            rates.rejections.entry(id.into()).or_default(),
            now,
            reservation,
        );
        if let Err(error) = rejected {
            if error == Error::FirstRejected {
                receipt.record(&tx, "rate_limited", false, None, None)?;
            }
            tx.commit()?;
            return Ok(Response::error(429, "rate_limited", "rate_limited"));
        }
        let signed = task["schedule"]["signature"].is_object();
        if task["enabled"] != true {
            receipt.record(&tx, "disabled", false, None, None)?;
            tx.commit()?;
            return Ok(Response::error(409, "disabled", "hook_disabled"));
        }
        if signed && !verify(&task["schedule"]["signature"], &credentials, &request, body) {
            receipt.record(&tx, "rejected_signature", false, None, None)?;
            tx.commit()?;
            return Ok(Response::error(
                401,
                "rejected_signature",
                "invalid_signature",
            ));
        }
        let refund = |rates: &mut AdmissionState| {
            if let Some(window) = rates.rejections.get_mut(id) {
                window.accepted.retain(|(r, _)| *r != reservation);
            }
        };
        if let Some(filters) = task["schedule"]["filters"].as_array() {
            if let Some(failed) = request.failed_filter(filters) {
                if log_slot(&mut rates.filtered, id, now) {
                    receipt.record(
                        &tx,
                        "filtered",
                        signed,
                        None,
                        Some(&format!(
                            "Filter {} ({}) did not match.",
                            failed.index + 1,
                            failed.path
                        )),
                    )?;
                }
                refund(&mut rates);
                tx.commit()?;
                return Ok(Response::accepted(&delivery, "filtered"));
            }
        }
        if task["schedule"].get("dedupe").is_none()
            && rates.runs.get(id).is_some_and(|window| {
                window
                    .accepted
                    .iter()
                    .filter(|(_, at)| now.saturating_sub(*at) < 60000)
                    .count()
                    >= 60
            })
        {
            let limited = slot(rates.runs.entry(id.into()).or_default(), now, reservation);
            if limited == Err(Error::FirstRejected) {
                receipt.record(&tx, "rate_limited", signed, None, None)?;
            }
            tx.commit()?;
            return Ok(Response::error(429, "rate_limited", "rate_limited"));
        }
        struct Slots<'a> {
            rates: &'a mut AdmissionState,
            id: &'a str,
            now: i64,
            reservation: u64,
            request: &'a Request,
            body: &'a [u8],
        }
        impl Admission for Slots<'_> {
            fn check_capacity(&mut self, db: &Connection) -> Result<(), Error> {
                let count:i64=db.query_row("SELECT count(*) FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state IN ('pending','dispatching','running')",[self.id],|row|row.get(0))?;
                if count >= 20 {
                    Err(Error::QueueFull)
                } else {
                    Ok(())
                }
            }
            fn reserve(&mut self) -> Result<(), Error> {
                slot(
                    self.rates.runs.entry(self.id.into()).or_default(),
                    self.now,
                    self.reservation,
                )
            }
            fn reserve_coalesced_log(&mut self) -> bool {
                log_slot(&mut self.rates.deduped, self.id, self.now)
            }
            fn replace_request(&mut self, db: &Connection, owner: &str) -> Result<(), Error> {
                replace_request(db, owner, self.request, self.body)
            }
        }
        let deduped = {
            let mut slots = Slots {
                rates: &mut rates,
                id,
                now,
                reservation,
                request: &request,
                body,
            };
            dedupe::accept(&tx, &task, &request, &delivery, now, &mut slots)
        };
        let deduped = match deduped {
            Ok(outcome) => outcome,
            Err(error @ (Error::FirstRejected | Error::Rejected | Error::QueueFull)) => {
                tx.rollback()?;
                refund(&mut rates);
                let (outcome, status) = if error == Error::QueueFull {
                    ("queue_full", 429)
                } else {
                    ("rate_limited", 429)
                };
                if error == Error::FirstRejected {
                    receipt.record(&db, outcome, signed, None, None)?;
                }
                return Ok(Response::error(status, outcome, outcome));
            }
            Err(error) => return Err(error),
        };
        let outcome = match deduped {
            Outcome::Pending => {
                receipt.record(&tx, "accepted", signed, None, None)?;
                "accepted"
            }
            Outcome::Coalesced { logged, .. } => {
                if logged {
                    receipt.record(&tx, "coalesced", signed, None, None)?;
                }
                "coalesced"
            }
            Outcome::Duplicate => {
                if log_slot(&mut rates.deduped, id, now) {
                    receipt.record(&tx, "duplicate", signed, None, None)?;
                }
                refund(&mut rates);
                "duplicate"
            }
            Outcome::Run => {
                let nonce = random_id().map_err(Error::Persistence)?.replace('-', "");
                let rendered =
                    request.render(task["prompt"].as_str().unwrap_or(""), &nonce[..12], false);
                if rendered
                    .prompt
                    .trim_matches(webhook_payload::js_space)
                    .encode_utf16()
                    .count()
                    > 120000
                {
                    receipt.record(
                        &tx,
                        "dispatch_failed",
                        signed,
                        Some(&rendered),
                        Some("The filled-in prompt is too long."),
                    )?;
                    dedupe::release_replay(&tx, &delivery)?;
                    tx.commit()?;
                    return Ok(Response::accepted(&delivery, "prompt_too_long"));
                }
                refund(&mut rates);
                let immediate =
                    dedupe::accept_immediate(&tx, &task, &request, &delivery, now, || {
                        let mut slots = Slots {
                            rates: &mut rates,
                            id,
                            now,
                            reservation,
                            request: &request,
                            body,
                        };
                        slots.check_capacity(&tx)?;
                        slots.reserve()
                    });
                match immediate {
                    Ok(Outcome::Duplicate) => {
                        if log_slot(&mut rates.deduped, id, now) {
                            receipt.record(&tx, "duplicate", signed, None, None)?;
                        }
                        tx.commit()?;
                        return Ok(Response::accepted(&delivery, "duplicate"));
                    }
                    Ok(_) => {}
                    Err(error @ (Error::FirstRejected | Error::Rejected | Error::QueueFull)) => {
                        tx.rollback()?;
                        let outcome = if error == Error::QueueFull {
                            "queue_full"
                        } else {
                            "rate_limited"
                        };
                        if error == Error::FirstRejected {
                            receipt.record(&db, outcome, signed, None, None)?;
                        }
                        return Ok(Response::error(429, outcome, outcome));
                    }
                    Err(error) => return Err(error),
                }
                receipt.record(&tx, "accepted", signed, Some(&rendered), None)?;
                // Immediate dispatch uses the same durable ownership format, already out
                // of a coalescing window. Its public pendingWindow flag stays false.
                tx.execute("INSERT INTO scheduled_task_webhook_pending(delivery_id,task_id,coalesce_key,due_at,state,task_json,request_json,delivery_ids_json) VALUES(?1,?2,'',?3,'running',?4,?5,?6)",params![delivery,id,now,task.to_string(),serde_json::to_string(&request)?,json!([delivery]).to_string()])?;
                "accepted"
            }
        };
        if outcome != "coalesced" {
            refund(&mut rates);
        }
        tx.commit()?;
        Ok(Response::accepted(&delivery, outcome))
    })();
    drop(db);
    drop(rates);
    match result {
        Ok(response) => {
            if response.status == 202 && crate::command_shared::ensure_exported(s, None).is_err() {
                return Response::error(500, "error", "internal_error");
            }
            s.streams.ops.wake_scheduler();
            response
        }
        Err(_) => Response::error(500, "error", "internal_error"),
    }
}

pub(crate) fn next_due(s: &State) -> Option<i64> {
    let db = s.db.lock().unwrap();
    db.query_row(&format!("SELECT min(CASE WHEN p.state='pending' THEN p.due_at ELSE 0 END) FROM scheduled_task_webhook_pending p WHERE p.state IN ('pending','dispatching','running','completed','failed') AND {RECOVERABLE}"),[],|row|row.get(0)).ok().flatten()
}

fn fail_delivery(db: &Connection, delivery: &str, error: &str, release: bool) -> Result<(), Error> {
    db.execute("UPDATE scheduled_task_webhook_deliveries SET outcome='dispatch_failed',error=?1 WHERE delivery_id=?2 OR delivery_id IN (SELECT value FROM scheduled_task_webhook_pending,json_each(delivery_ids_json) WHERE delivery_id=?2)",params![error,delivery])?;
    if release {
        dedupe::release_replay(db, delivery)?;
        db.execute("DELETE FROM scheduled_task_webhook_relay_deliveries WHERE relay_delivery_id IN (SELECT substr(value,16) FROM scheduled_task_webhook_pending,json_each(delivery_ids_json) WHERE delivery_id=?1 AND value LIKE 'delivery:relay:%')",[delivery])?;
        if let Some(relay) = delivery.strip_prefix("delivery:relay:") {
            db.execute(
                "DELETE FROM scheduled_task_webhook_relay_deliveries WHERE relay_delivery_id=?1",
                [relay],
            )?;
        }
    }
    Ok(())
}
pub(crate) fn tick(s: &Arc<State>) {
    // A task-level guard serializes windows while independent scheduler callers
    // can still handle different tasks. Durable state alone is not a live claim.
    for _ in 0..8 {
        let entry = {
            let db = s.db.lock().unwrap();
            let active = s.streams.ops.hook_runs.lock().unwrap();
            let mut query=match db.prepare(&format!("SELECT p.delivery_id,p.task_id,p.state,p.task_json,p.request_json FROM scheduled_task_webhook_pending p WHERE ((p.state='pending' AND p.due_at<=?1) OR p.state IN ('dispatching','running','completed','failed')) AND {RECOVERABLE} ORDER BY p.due_at,p.rowid")) {Ok(q)=>q,Err(_)=>return};
            let rows = match query.query_map([now_ms()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            }) {
                Ok(r) => r,
                Err(_) => return,
            };
            let found = rows
                .filter_map(Result::ok)
                .find(|row| !active.contains(&format!("native-webhook:{}", row.1)));
            found
        };
        let Some((delivery, task_id, state, snapshot, request)) = entry else {
            return;
        };
        let key = format!("native-webhook:{task_id}");
        if !s.streams.ops.hook_runs.lock().unwrap().insert(key.clone()) {
            continue;
        }
        struct Claim<'a>(&'a State, String);
        impl Drop for Claim<'_> {
            fn drop(&mut self) {
                self.0.streams.ops.hook_runs.lock().unwrap().remove(&self.1);
                self.0.streams.ops.scheduler_wake.notify_one();
            }
        }
        let _claim = Claim(s, key);
        let attempt = (|| -> Result<bool, Error> {
            if matches!(state.as_str(), "completed" | "failed") {
                if state == "failed" {
                    fail_delivery(
                        &s.db.lock().unwrap(),
                        &delivery,
                        "The run failed to start.",
                        true,
                    )?;
                }
                return Ok(true);
            }
            let prepared = serde_json::from_str::<Value>(&snapshot).and_then(|snapshot| {
                serde_json::from_str::<Request>(&request).map(|request| (snapshot, request))
            });
            let (mut snapshot, request) = match prepared {
                Ok(value) => value,
                Err(_) => {
                    fail_delivery(
                        &s.db.lock().unwrap(),
                        &delivery,
                        "The pending webhook payload could not be prepared.",
                        true,
                    )?;
                    return Ok(true);
                }
            };
            if state != "running" {
                let db = s.db.lock().unwrap();
                if let Ok(current) = get(&db, "scheduledTasks", &task_id) {
                    if current["createdAt"] == snapshot["createdAt"] {
                        snapshot["prompt"] = current["prompt"].clone();
                    }
                }
                db.execute("UPDATE scheduled_task_webhook_pending SET state='dispatching',task_json=?1 WHERE delivery_id=?2",params![snapshot.to_string(),delivery])?;
            }
            let nonce = random_id().map_err(Error::Persistence)?.replace('-', "");
            let rendered = request.render(
                snapshot["prompt"].as_str().unwrap_or(""),
                &nonce[..12],
                false,
            );
            if rendered
                .prompt
                .trim_matches(webhook_payload::js_space)
                .encode_utf16()
                .count()
                > 120000
            {
                fail_delivery(
                    &s.db.lock().unwrap(),
                    &delivery,
                    "The filled-in prompt is too long.",
                    true,
                )?;
                return Ok(true);
            }
            s.db.lock().unwrap().execute("UPDATE scheduled_task_webhook_deliveries SET outcome='accepted',error=NULL,rendered_prompt=?1,missing_fields_json=?2 WHERE delivery_id=?3",params![String::from_utf16_lossy(&rendered.prompt.encode_utf16().take(65536).collect::<Vec<_>>()),serde_json::to_string(&rendered.missing)?,delivery])?;
            match crate::scheduled::fire_webhook(s, &snapshot, &delivery, &rendered.prompt) {
                Ok(()) => Ok(true),
                Err(error) => {
                    fail_delivery(
                        &s.db.lock().unwrap(),
                        &delivery,
                        &error.message,
                        !error.started,
                    )?;
                    Ok(!error.started)
                }
            }
        })();
        match attempt {
            Ok(true) => {
                let _ = s.db.lock().unwrap().execute(
                    "DELETE FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
                    [&delivery],
                );
            }
            Ok(false) => return,
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_pinned_signature_and_template_functions_supply_admission_cases() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/webhook-admission.json")).unwrap();
        assert_eq!(fixture["effect"], "4.1.2");
        assert_eq!(fixture["verification"].as_array().unwrap().len(), 360);
        for (index, case) in fixture["verification"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let body = case["body"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b.as_u64().unwrap() as u8)
                .collect::<Vec<_>>();
            let request = Request {
                method: "POST".into(),
                path: "/api/hooks/test".into(),
                query: String::new(),
                headers: serde_json::from_value(case["headers"].clone()).unwrap(),
                body_text: text(&body),
            };
            assert_eq!(
                verify(
                    &case["signature"],
                    &json!({"secret":case["secret"]}),
                    &request,
                    &body
                ),
                case["expected"].as_bool().unwrap(),
                "signature case {index}"
            );
        }
        for case in fixture["templates"].as_array().unwrap() {
            assert_eq!(
                crate::webhook::dedupe_template(case["template"].as_str().unwrap()),
                case["expected"].as_bool().unwrap(),
                "{}",
                case["template"]
            );
        }
    }

    #[test]
    fn sliding_windows_refund_by_identity_and_reset_rejection_after_capacity_returns() {
        let mut window = Window::default();
        for id in 0..60 {
            assert_eq!(slot(&mut window, 1000, id), Ok(()));
        }
        assert_eq!(slot(&mut window, 1000, 60), Err(Error::FirstRejected));
        assert_eq!(slot(&mut window, 1000, 61), Err(Error::Rejected));
        window.accepted.retain(|(id, _)| *id != 17);
        assert_eq!(slot(&mut window, 1000, 62), Ok(()));
        assert_eq!(slot(&mut window, 60999, 63), Err(Error::FirstRejected));
        assert_eq!(slot(&mut window, 61000, 64), Ok(()));
        assert_eq!(window.accepted, vec![(64, 61000)]);
    }

    #[test]
    fn logging_matches_streaming_utf8_boundary_bom_and_independent_retention_buckets() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        crate::task_shared::initialize(&db).unwrap();
        crate::events::put(&db, "scheduledTasks", "t", "", &json!({"id":"t"})).unwrap();
        let body = [vec![b'a'; 65535], "😀".as_bytes().to_vec()].concat();
        let request = Request {
            method: "POST".into(),
            path: "/api/hooks/t".into(),
            query: "token=synthetic&normal=yes".into(),
            headers: [
                ("authorization".into(), "synthetic-token".into()),
                ("content-type".into(), "application/json".into()),
            ]
            .into(),
            body_text: text(&body),
        };
        assert_eq!(logged_body(&body, &request), "a".repeat(65535));
        assert_eq!(text(b"\xef\xbb\xbfhello"), "hello");
        for (outcome, count, keep) in [
            ("accepted", 55, 50),
            ("filtered", 25, 20),
            ("duplicate", 25, 20),
        ] {
            for index in 0..count {
                Receipt {
                    task: "t",
                    id: &format!("{outcome}-{index}"),
                    now: 1000,
                    request: &request,
                    body: &body,
                }
                .record(&db, outcome, true, None, None)
                .unwrap();
            }
            let actual: i64 = db
                .query_row(
                    "SELECT count(*) FROM scheduled_task_webhook_deliveries WHERE outcome=?1",
                    [outcome],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, keep);
        }
        let header: String = db
            .query_row(
                "SELECT headers_json FROM scheduled_task_webhook_deliveries LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!header.contains("synthetic-token"));
        let query: String = db
            .query_row(
                "SELECT query FROM scheduled_task_webhook_deliveries LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(query, "token=[redacted]&normal=yes");
        db.execute_batch("CREATE TRIGGER refuse_prune BEFORE DELETE ON scheduled_task_webhook_deliveries BEGIN SELECT RAISE(ABORT,'synthetic prune refusal'); END;").unwrap();
        assert!(Receipt {
            task: "t",
            id: "must-rollback",
            now: 1001,
            request: &request,
            body: &body
        }
        .record(&db, "accepted", true, None, None)
        .is_err());
        assert_eq!(db.query_row("SELECT count(*) FROM scheduled_task_webhook_deliveries WHERE delivery_id='must-rollback'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
}
