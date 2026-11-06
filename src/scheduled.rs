//! Scheduled tasks: stored definitions plus an in-process timer that launches threads.
//!
//! Schedules follow T3 nightly 4262: `interval`, `fixed_time` (UTC, optionally on chosen
//! weekdays), `once` and signed `webhook` (inbound route in `webhook.rs`). T3 reads tasks only
//! through the `scheduledTasks.subscribe` stream; `scheduledTasks.list` stays for older o3
//! clients. Without `orchestration:operate` both views drop each task's `webhook`.
use crate::{
    events::{get, put},
    store::{iso, now_ms, random_id, State},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

#[path = "webhook_history.rs"]
mod history;

type Reply = Result<Value, Value>;
const KIND: &str = "scheduledTasks";
const MIN_INTERVAL_MS: u64 = 60_000;
const MAX_TASKS: i64 = 500;
const DAY_MS: i64 = 86_400_000;
const SETTLE_INTERVAL_MS: i64 = 60_000;

fn failure(message: &str, task: Option<&str>) -> Value {
    let mut e = json!({"_tag":"ScheduledTaskError","message":message});
    if let Some(t) = task {
        e["taskId"] = json!(t);
    }
    e
}

pub fn call(s: &Arc<State>, tag: &str, p: &Value) -> Option<Reply> {
    Some(match tag {
        "scheduledTasks.list" => list_view(s, p["_hideWebhook"] == true),
        "scheduledTasks.upsert" => upsert(s, p).map(|task| json!({"task":task})),
        "scheduledTasks.setEnabled" => set_enabled(s, p).map(|task| json!({"task":task})),
        "scheduledTasks.delete" => delete(s, p).map(|id| json!({"id":id})),
        "scheduledTasks.runNow" => run_now(s, p).map(|task| json!({"task":task})),
        "scheduledTasks.listWebhookDeliveries" => history::list(s, p),
        "scheduledTasks.getWebhookDelivery" => history::get(s, p),
        "scheduledTasks.rotateWebhookToken" => {
            rotate_webhook_token(s, p).map(|task| json!({"task":task}))
        }
        _ => return None,
    })
}

fn list(s: &State) -> Result<Vec<Value>, Value> {
    let db = s.db.lock().unwrap();
    // T3 listRows: newest first, then task id.
    let mut q = db.prepare("SELECT body FROM records WHERE kind=? ORDER BY json_extract(body,'$.createdAt') DESC, id ASC").map_err(|e| failure(&e.to_string(), None))?;
    let rows = q
        .query_map([KIND], |r| r.get::<_, String>(0))
        .map_err(|e| failure(&e.to_string(), None))?;
    let mut out = Vec::new();
    for row in rows {
        let mut task: Value =
            serde_json::from_str(&row.map_err(|e| failure(&e.to_string(), None))?)
                .map_err(|e| failure(&e.to_string(), None))?;
        task["threadMode"] = json!(thread_mode(&task));
        out.push(task);
    }
    Ok(out)
}

/// `{tasks}` as T3's `subscribeList` emits it; read-only sessions do not see webhook URLs.
pub fn list_view(s: &State, hide_webhook: bool) -> Reply {
    let mut tasks = list(s)?;
    if hide_webhook {
        for task in &mut tasks {
            if let Some(t) = task.as_object_mut() {
                t.remove("webhook");
            }
        }
    }
    Ok(json!({ "tasks": tasks }))
}

/// T3's `ISO_WITH_OFFSET` check for `once.runAt`: a real calendar day, an explicit offset and
/// a UTC instant within years 0000-9999. Returns epoch milliseconds.
fn run_at_ms(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
    };
    if b.len() < 17
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
        || b[10] != b'T'
        || !digits(11..13)
        || b[13] != b':'
        || !digits(14..16)
    {
        return None;
    }
    let mut i = 16;
    let mut seconds = ":00".to_owned();
    if b.get(i) == Some(&b':') {
        if !digits(i + 1..i + 3) {
            return None;
        }
        i += 3;
        if b.get(i) == Some(&b'.') {
            let start = i + 1;
            i = start;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if i == start {
                return None;
            }
        }
        seconds.clear();
    }
    let offset = &text[i..];
    let offset_ok = offset == "Z"
        || (offset.len() == 6
            && matches!(offset.as_bytes()[0], b'+' | b'-')
            && offset.as_bytes()[3] == b':'
            && offset
                .bytes()
                .enumerate()
                .all(|(k, c)| matches!(k, 0 | 3) || c.is_ascii_digit()));
    let hour: u8 = text[11..13].parse().ok()?;
    let minute: u8 = text[14..16].parse().ok()?;
    if !offset_ok
        || hour > 23
        || minute > 59
        || (seconds.is_empty() && text[17..19].parse::<u8>().ok()? > 59)
    {
        return None;
    }
    let normalized = format!("{}{seconds}{offset}", &text[..i]);
    let at =
        time::OffsetDateTime::parse(&normalized, &time::format_description::well_known::Rfc3339)
            .ok()?;
    // Date.parse("0000-01-01T00:00:00Z") and Date.parse("+010000-01-01T00:00:00Z").
    const YEAR_0000: i64 = -62_167_219_200_000;
    const YEAR_10000: i64 = 253_402_300_800_000;
    let ms = (at.unix_timestamp_nanos() / 1_000_000) as i64;
    (YEAR_0000..YEAR_10000).contains(&ms).then_some(ms)
}

/// T3 `isSameSchedule`: an edit that keeps it keeps the pending `nextRunAt`.
fn same_schedule(a: &Value, b: &Value) -> bool {
    let weekdays = |v: &Value| {
        let mut d: Vec<i64> = v["weekdays"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        d.sort_unstable();
        d.dedup();
        if d.len() == 7 {
            Vec::new()
        } else {
            d
        }
    };
    match (a["type"].as_str(), b["type"].as_str()) {
        (Some("interval"), Some("interval")) => a["everyMs"] == b["everyMs"],
        (Some("webhook"), Some("webhook")) => true,
        (Some("once"), Some("once")) => {
            a["runAt"].as_str().and_then(run_at_ms) == b["runAt"].as_str().and_then(run_at_ms)
        }
        (Some("fixed_time"), Some("fixed_time")) => {
            let at = |v: &Value| v["timeOfDay"].as_str().and_then(time_of_day);
            at(a).is_some() && at(a) == at(b) && weekdays(a) == weekdays(b)
        }
        _ => false,
    }
}

/// T3 `requireFutureOnce`: an enabled one-time task whose time has passed would never run.
fn require_future_once(enabled: bool, schedule: &Value, now: i64, id: &str) -> Result<(), Value> {
    if enabled && schedule["type"] == "once" && next_run(schedule, now).is_none() {
        return Err(failure(
            "The run time has passed. Pick a time in the future.",
            Some(id),
        ));
    }
    Ok(())
}

fn time_of_day(text: &str) -> Option<i64> {
    let (h, m) = text.split_once(':')?;
    let digits = |v: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(h, 1..=2) || !digits(m, 2..=2) {
        return None;
    }
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h * 60 + m) * 60_000)
}

/// The next firing strictly after `now` for an enabled schedule.
pub fn next_run(schedule: &Value, now: i64) -> Option<i64> {
    match schedule["type"].as_str()? {
        "interval" => Some(now + schedule["everyMs"].as_i64()?),
        "once" => run_at_ms(schedule["runAt"].as_str()?).filter(|at| *at > now),
        "fixed_time" => {
            let at = time_of_day(schedule["timeOfDay"].as_str()?)?;
            let weekdays: Vec<i64> = schedule["weekdays"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let today = now.div_euclid(DAY_MS);
            (today..today + 8).map(|d| d * DAY_MS + at).find(|t| {
                *t > now
                    && (weekdays.is_empty()
                        || weekdays.contains(&((t.div_euclid(DAY_MS) + 4).rem_euclid(7))))
            })
        }
        _ => None,
    }
}

fn validate(p: &Value) -> Result<(), Value> {
    let text = |k: &str| {
        p[k].as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|_| ())
            .ok_or_else(|| failure(&format!("{k} is required"), None))
    };
    text("title")?;
    text("prompt")?;
    text("projectId")?;
    if !p["enabled"].is_boolean() {
        return Err(failure("enabled is required", None));
    }
    if p.get("threadMode").is_some_and(|mode| {
        !matches!(
            mode.as_str(),
            Some("new_thread" | "continue" | "fresh_session")
        )
    }) {
        return Err(failure("Invalid scheduled task thread mode.", None));
    }
    if p.get("threadId")
        .is_some_and(|id| !id.is_null() && !id.as_str().is_some_and(|id| !id.trim().is_empty()))
    {
        return Err(failure("Invalid scheduled task thread id.", None));
    }
    if p.get("createdBy")
        .is_some_and(|v| !matches!(v.as_str(), Some("user" | "agent" | "system")))
        || p.get("creationSource").is_some_and(|v| {
            !matches!(
                v.as_str(),
                Some("web" | "mobile" | "mcp" | "provider" | "server")
            )
        })
    {
        return Err(failure("Invalid scheduled task creation fields.", None));
    }
    let sched = &p["schedule"];
    match sched["type"].as_str() {
        Some("interval") => {
            if sched["everyMs"]
                .as_u64()
                .is_none_or(|n| n < MIN_INTERVAL_MS)
            {
                return Err(failure(
                    "An interval schedule must repeat at least every 60000 ms.",
                    None,
                ));
            }
        }
        Some("fixed_time") => {
            if sched["timeOfDay"].as_str().and_then(time_of_day).is_none() {
                return Err(failure("timeOfDay must look like HH:MM.", None));
            }
            if sched["weekdays"].as_array().is_some_and(|w| {
                w.iter()
                    .any(|d| d.as_i64().is_none_or(|d| !(0..=6).contains(&d)))
            }) {
                return Err(failure(
                    "weekdays must be integers from 0 (Sunday) to 6.",
                    None,
                ));
            }
        }
        Some("once") => {
            if sched["runAt"]
                .as_str()
                .map(str::trim)
                .and_then(run_at_ms)
                .is_none()
            {
                return Err(failure(
                    "Expected an ISO date and time with an offset.",
                    None,
                ));
            }
        }
        Some("webhook") => {
            crate::webhook::validate(sched).map_err(|e| failure(e, None))?;
        }
        _ => {
            return Err(failure(
                "A schedule of type interval, fixed_time, once or webhook is required.",
                None,
            ))
        }
    }
    if !p["workspaceStrategy"]["type"].is_string()
        || !p["modelSelection"]["instanceId"].is_string()
        || !p["modelSelection"]["model"].is_string()
    {
        return Err(failure(
            "workspaceStrategy and modelSelection are required.",
            None,
        ));
    }
    if !p["runtimeMode"].is_string() || !p["interactionMode"].is_string() {
        return Err(failure(
            "runtimeMode and interactionMode are required.",
            None,
        ));
    }
    Ok(())
}

/// T3's legacy interpretation and save resolution, including a stale form with no bound thread.
fn thread_mode(task: &Value) -> &str {
    task["threadMode"].as_str().unwrap_or_else(|| {
        if task["threadId"].is_null() {
            "new_thread"
        } else {
            "continue"
        }
    })
}
fn resolve_thread_mode<'a>(input: &'a Value, existing: Option<&'a Value>) -> &'a str {
    if let Some(mode) = input["threadMode"].as_str() {
        return mode;
    }
    let stored = existing.map(thread_mode).unwrap_or("new_thread");
    if stored == "new_thread" && !input["threadId"].is_null() {
        "continue"
    } else {
        stored
    }
}
fn saved_binding(input: &Value, existing: Option<&Value>, mode: &str) -> Value {
    let kept = input
        .get("threadId")
        .filter(|id| !id.is_null())
        .or_else(|| existing.and_then(|task| task.get("threadId")))
        .cloned()
        .unwrap_or(Value::Null);
    let moved = existing
        .is_some_and(|old| old["projectId"] != input["projectId"] && kept == old["threadId"]);
    if mode == "new_thread" || moved {
        Value::Null
    } else {
        kept
    }
}

fn upsert(s: &Arc<State>, p: &Value) -> Reply {
    validate(p)?;
    let now = now_ms();
    let at = iso(now);
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|e| failure(&e.to_string(), None))?;
    // T3: `id ?? scheduled-task:<commandId>`, else a fresh `scheduled-task:<uuid>`. A replay
    // with the same commandId resolves to the same row, keeping its run history.
    let id = match (p["id"].as_str(), p["commandId"].as_str()) {
        (Some(id), _) => id.to_owned(),
        (None, Some(cid)) => format!("scheduled-task:{cid}"),
        (None, None) => format!(
            "scheduled-task:{}",
            random_id()
                .map_err(|e| failure(&format!("Could not generate schedule task id. {e}"), None))?
        ),
    };
    let existing = get(&tx, KIND, &id).ok();
    if existing.is_none() && p["requireExisting"] == true {
        return Err(failure("Schedule task not found.", Some(&id)));
    }
    if existing.is_none() {
        let count: i64 = tx
            .query_row("SELECT count(*) FROM records WHERE kind=?", [KIND], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        if count >= MAX_TASKS {
            return Err(failure("Too many scheduled tasks.", None));
        }
    }
    let mode = resolve_thread_mode(p, existing.as_ref());
    let binding = saved_binding(p, existing.as_ref(), mode);
    let old = existing.clone().unwrap_or_else(|| json!({"createdAt":at,"lastRunAt":null,"lastRunStatus":"never","lastRunError":null,"runCount":0}));
    let enabled = p["enabled"] == true;
    let mut schedule = p["schedule"].clone();
    if schedule["type"] == "once" {
        let run_at = schedule["runAt"]
            .as_str()
            .map(str::trim)
            .and_then(run_at_ms);
        schedule = json!({"type":"once","runAt":run_at.map(iso)});
    }
    if schedule["type"] == "webhook" {
        // T3's stored webhook schedule: lowercase header, explicit null age, omitted
        // dedupe/filters keep what was stored, and an empty filter list is dropped.
        let prior = existing
            .as_ref()
            .map(|e| &e["schedule"])
            .filter(|s| s["type"] == "webhook");
        let sig = &p["schedule"]["signature"];
        let mut next = json!({"type":"webhook","signature":Value::Null,"maxDeliveryAgeMinutes":p["schedule"]["maxDeliveryAgeMinutes"].clone()});
        if sig.is_object() {
            let mut stored = json!({"header":sig["header"].as_str().map(|header| header.trim_matches(crate::webhook_payload::js_space).to_lowercase()),"encoding":sig["encoding"],"prefix":sig["prefix"]});
            if let Some(secret) = sig.get("secret") {
                stored["secret"] = secret.clone();
            }
            next["signature"] = stored;
        }
        if let Some(dedupe) = p["schedule"]
            .get("dedupe")
            .or_else(|| prior.and_then(|s| s.get("dedupe")))
        {
            next["dedupe"] = dedupe.clone();
        }
        let filters = p["schedule"]
            .get("filters")
            .or_else(|| prior.and_then(|s| s.get("filters")));
        if let Some(f) = filters.filter(|f| f.as_array().is_some_and(|a| !a.is_empty())) {
            next["filters"] = f.clone();
        }
        schedule = next;
    }
    let unchanged = existing
        .as_ref()
        .is_some_and(|e| e["enabled"] == enabled && same_schedule(&e["schedule"], &schedule));
    if !unchanged {
        require_future_once(enabled, &schedule, now, &id)?;
    }
    let mut task = json!({"id":id,"title":p["title"].as_str().map(str::trim),"prompt":p["prompt"].as_str().map(str::trim),"enabled":enabled,"schedule":schedule,"projectId":p["projectId"],"threadId":binding,"threadMode":mode,"workspaceStrategy":p["workspaceStrategy"],"modelSelection":p["modelSelection"],"runtimeMode":p["runtimeMode"],"interactionMode":p["interactionMode"],"createdBy":p.get("createdBy").cloned().unwrap_or(json!("user")),"creationSource":p.get("creationSource").cloned().unwrap_or(json!("web")),"createdAt":old["createdAt"],"updatedAt":at,"nextRunAt":Value::Null,"lastRunAt":old["lastRunAt"],"lastRunStatus":old["lastRunStatus"],"lastRunError":old["lastRunError"],"runCount":old["runCount"]});
    match &existing {
        Some(e) if unchanged => {
            task["nextRunAt"] = e["nextRunAt"].clone();
        }
        _ if enabled => {
            task["nextRunAt"] = next_run(&schedule, now)
                .map(|t| json!(iso(t)))
                .unwrap_or(Value::Null);
        }
        _ => {}
    }
    // T3 keeps the original creator but records this save's creationSource.
    if let Some(e) = &existing {
        task["createdBy"] = e["createdBy"].clone();
    }
    crate::webhook::prepare(&tx, &id, &mut task).map_err(|e| failure(&e, Some(&id)))?;
    put(&tx, KIND, &id, "", &task).map_err(|e| failure(&e, Some(&id)))?;
    tx.commit().map_err(|e| failure(&e.to_string(), None))?;
    drop(db);
    ensure_started(s);
    s.streams.ops.wake_scheduler();
    Ok(task)
}

fn set_enabled(s: &Arc<State>, p: &Value) -> Reply {
    let id = p["id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    let enabled = p["enabled"]
        .as_bool()
        .ok_or_else(|| failure("enabled is required", Some(id)))?;
    let db = s.db.lock().unwrap();
    let mut task = get(&db, KIND, id).map_err(|_| failure("Schedule task not found.", Some(id)))?;
    if task["enabled"] == enabled {
        return Ok(task);
    }
    require_future_once(enabled, &task["schedule"], now_ms(), id)?;
    task["enabled"] = json!(enabled);
    task["nextRunAt"] = if enabled {
        next_run(&task["schedule"], now_ms())
            .map(|t| json!(iso(t)))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    task["updatedAt"] = json!(iso(now_ms()));
    put(&db, KIND, id, "", &task).map_err(|e| failure(&e, Some(id)))?;
    drop(db);
    s.streams.ops.wake_scheduler();
    Ok(task)
}

pub(crate) fn remove_task(db: &rusqlite::Connection, id: &str) -> Result<(), Value> {
    let webhook = get(db, KIND, id).is_ok_and(|task| task["schedule"]["type"] == "webhook")
        || get(db, "hookCredentials", id).is_ok();
    db.execute(
        "DELETE FROM records WHERE kind='scheduledTasks' AND id=?1",
        [id],
    )
    .map_err(|e| failure(&e.to_string(), Some(id)))?;
    crate::task_shared::delete_ledger(db, id).map_err(|e| failure(&e, Some(id)))?;
    if webhook {
        for sql in [
            "DELETE FROM records WHERE kind='hookCredentials' AND id=?1",
            "DELETE FROM hook_queue WHERE task=?1",
            "DELETE FROM hook_replay WHERE task=?1",
        ] {
            db.execute(sql, [id])
                .map_err(|e| failure(&e.to_string(), Some(id)))?;
        }
    }
    Ok(())
}

fn delete(s: &Arc<State>, p: &Value) -> Result<String, Value> {
    let id = p["id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|e| failure(&e.to_string(), Some(id)))?;
    // T3 deletes idempotently: a missing task still answers `{id}`.
    remove_task(&tx, id)?;
    tx.commit().map_err(|e| failure(&e.to_string(), Some(id)))?;
    drop(db);
    s.streams.ops.wake_scheduler();
    Ok(id.to_owned())
}

fn run_now(s: &Arc<State>, p: &Value) -> Reply {
    let id = p["id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    let task = get(&s.db.lock().unwrap(), KIND, id)
        .map_err(|_| failure("Schedule task not found.", Some(id)))?;
    if task["schedule"]["type"] == "webhook" {
        return Err(failure(
            "Webhook tasks run when their URL receives a request.",
            Some(id),
        ));
    }
    fire(s, id)
}

fn rotate_webhook_token(s: &Arc<State>, input: &Value) -> Reply {
    use rusqlite::OptionalExtension;
    let id = input["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|_| failure("Could not rotate webhook token.", Some(id)))?;
    // Lookup and update share the writer transaction, so a same-id replacement
    // cannot receive a token generated for the previous task row.
    let mut task = get(&tx, KIND, id).map_err(|_| failure("Schedule task not found.", Some(id)))?;
    if task["schedule"]["type"] != "webhook" {
        return Err(failure("Only webhook tasks have a URL token.", Some(id)));
    }
    let token = crate::webhook::new_token().map_err(|e| failure(&e, None))?;
    let saved: Option<String> = tx
        .query_row(
            "SELECT body FROM records WHERE kind='hookCredentials' AND id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| failure("Could not rotate webhook token.", Some(id)))?;
    let mut credentials = match saved {
        Some(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| failure("Could not rotate webhook token.", Some(id)))?,
        None => json!({"secret":null}),
    };
    credentials["token"] = json!(token);
    task["webhook"] = json!({"path":format!("/api/hooks/{}/{token}",crate::webhook::encode_component(id)),"url":null,"hasSecret":!credentials["secret"].is_null()});
    task["updatedAt"] = json!(iso(now_ms()));
    let write = || -> Result<(), String> {
        put(&tx, "hookCredentials", id, "", &credentials)?;
        put(&tx, KIND, id, "", &task)
    };
    write().map_err(|_| failure("Could not rotate webhook token.", Some(id)))?;
    tx.commit()
        .map_err(|_| failure("Could not rotate webhook token.", Some(id)))?;
    drop(db);
    s.streams.ops.wake_scheduler();
    Ok(task)
}

/// Starts the task's thread (or message) and records the outcome on the task.
fn fire(s: &Arc<State>, id: &str) -> Reply {
    fire_prompt(s, id, None)
}
pub(crate) fn fire_prompt(s: &Arc<State>, id: &str, prompt: Option<&str>) -> Reply {
    fire_prompt_key(s, id, prompt, None)
}
struct ActiveTask<'a> {
    state: &'a State,
    id: &'a str,
}
impl Drop for ActiveTask<'_> {
    fn drop(&mut self) {
        self.state
            .streams
            .ops
            .scheduled_runs
            .lock()
            .unwrap()
            .remove(self.id);
    }
}

pub(crate) fn fire_prompt_key(
    s: &Arc<State>,
    id: &str,
    prompt: Option<&str>,
    job: Option<&str>,
) -> Reply {
    if !s
        .streams
        .ops
        .scheduled_runs
        .lock()
        .unwrap()
        .insert(id.to_owned())
    {
        return Err(failure("Schedule task is already running.", Some(id)));
    }
    let _active = ActiveTask { state: s, id };
    let started = now_ms();
    let mut task = {
        let db = s.db.lock().unwrap();
        let mut task =
            get(&db, KIND, id).map_err(|_| failure("Schedule task not found.", Some(id)))?;
        // T3 markRunning: subscribers see the run before the launch settles.
        let mut running = task.clone();
        running["lastRunAt"] = json!(iso(started));
        running["lastRunStatus"] = json!("running");
        running["lastRunError"] = Value::Null;
        running["updatedAt"] = json!(iso(started));
        put(&db, KIND, id, "", &running).map_err(|e| failure(&e, Some(id)))?;
        task["lastRunAt"] = running["lastRunAt"].clone();
        task
    };
    s.streams.ops.wake_scheduler();
    if let Some(prompt) = prompt {
        task["prompt"] = json!(prompt);
    }
    let number = task["runCount"].as_i64().unwrap_or(0) + 1;
    let cid = job
        .map(|j| format!("webhook:{id}:{j}"))
        .unwrap_or_else(|| format!("scheduled:{id}:{number}"));
    let message = job
        .map(|j| format!("webhook-{j}"))
        .unwrap_or_else(|| format!("scheduled-{id}-{number}"));
    let outcome = dispatch_task(s, &task, &cid, &message);
    let now = now_ms();
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|e| failure(&e.to_string(), Some(id)))?;
    let task = record_outcome(&tx, &task, &outcome, started, now)?;
    tx.commit().map_err(|e| failure(&e.to_string(), Some(id)))?;
    drop(db);
    s.streams.ops.wake_scheduler();
    if job.is_some() {
        outcome.map_err(|e| failure(&e, Some(id)))?;
    }
    Ok(task)
}

fn dispatch_task(s: &Arc<State>, task: &Value, cid: &str, message: &str) -> Result<String, String> {
    let id = task["id"].as_str().ok_or("Task identity missing")?;
    let mode = thread_mode(task).to_owned();
    match task["threadId"].as_str() {
        Some(thread) => {
            let target = get(&s.db.lock().unwrap(), "thread", thread);
            match target {
                Ok(target) if !target["deletedAt"].is_null() => Err("Thread not found.".into()),
                Ok(target) if !target["archivedAt"].is_null() => Err(format!("Thread {thread} is archived and cannot receive messages.")),
                Err(error) => Err(error),
                Ok(_) => {
                    let mut command = json!({"type":"message.dispatch","commandId":cid,"messageId":message,"threadId":thread,"text":task["prompt"],"attachments":[],"scheduledTaskId":id,"createdBy":task["createdBy"],"creationSource":task["creationSource"],"dispatchMode":{"type":"queue_after_active"}});
                    if mode == "fresh_session" { command["freshContext"] = json!(true); }
                    crate::events::dispatch(s, &command).map(|_| thread.to_owned())
                }
            }
        }
        None => crate::events::launch(s, &json!({"commandId":cid,"projectId":task["projectId"],"title":task["title"],"modelSelection":task["modelSelection"],"runtimeMode":task["runtimeMode"],"interactionMode":task["interactionMode"],"workspaceStrategy":task["workspaceStrategy"],"createdBy":task["createdBy"],"creationSource":task["creationSource"],"initialMessage":{"messageId":message,"scheduledTaskId":id,"text":task["prompt"],"attachments":[]}}))
            .and_then(|result| result["threadId"].as_str().map(str::to_owned).ok_or_else(|| "Launched thread has no identity".into())),
    }
}

#[derive(Debug)]
pub(crate) struct WebhookFailure {
    pub message: String,
    /// Accepted work owns its replay claims even if completion bookkeeping fails.
    pub started: bool,
}

pub(crate) fn fire_webhook(
    s: &Arc<State>,
    snapshot: &Value,
    delivery: &str,
    prompt: &str,
) -> Result<(), WebhookFailure> {
    use rusqlite::{params, OptionalExtension};
    let fail = |message: &str, started| WebhookFailure {
        message: message.into(),
        started,
    };
    let id = snapshot["id"]
        .as_str()
        .ok_or_else(|| fail("The pending webhook payload could not be prepared.", false))?;
    if !s
        .streams
        .ops
        .scheduled_runs
        .lock()
        .unwrap()
        .insert(id.into())
    {
        return Err(fail("Schedule task is already running.", false));
    }
    let _active = ActiveTask { state: s, id };
    let started = now_ms();
    let mut task = {
        let mut db = s.db.lock().unwrap();
        let tx = db
            .transaction()
            .map_err(|_| fail("Could not start webhook run.", false))?;
        let mut task = get(&tx, KIND, id)
            .map_err(|_| fail("The task was deleted before this delivery ran.", false))?;
        let completed=tx.query_row("SELECT 1 FROM scheduled_task_webhook_pending WHERE delivery_id=?1 AND state='completed'",[delivery],|_|Ok(())).optional().map_err(|_|fail("Could not start webhook run.",false))?.is_some();
        if completed {
            return Ok(());
        }
        let blocked = if task["createdAt"] != snapshot["createdAt"] {
            Some("The task was replaced before this delivery ran.")
        } else if task["schedule"]["type"] != "webhook" {
            Some("The task's trigger changed before this delivery ran.")
        } else if let Some(reason) = task["blockedByPolicy"].as_str() {
            Some(reason)
        } else if task["enabled"] != true {
            Some("The task was paused before this delivery ran.")
        } else {
            None
        };
        if let Some(reason) = blocked {
            return Err(fail(reason, false));
        }
        task["lastRunAt"] = json!(iso(started));
        task["lastRunStatus"] = json!("running");
        task["lastRunError"] = Value::Null;
        task["updatedAt"] = json!(iso(started));
        let write = || -> Result<(), String> {
            put(&tx, KIND, id, "", &task)?;
            put(
                &tx,
                "scheduledTaskNative",
                id,
                "",
                &json!({"lastRunWebhookDeliveryId":delivery}),
            )?;
            tx.execute(
                "UPDATE scheduled_task_webhook_pending SET state='running' WHERE delivery_id=?1",
                [delivery],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        };
        write().map_err(|_| fail("Could not start webhook run.", false))?;
        tx.commit()
            .map_err(|_| fail("Could not start webhook run.", false))?;
        task
    };
    s.streams.ops.wake_scheduler();
    task["prompt"] = json!(prompt);
    let cid = format!("scheduled-task:{id}:webhook:{delivery}");
    let message = format!("scheduled-task-message:{id}:webhook:{delivery}");
    let outcome = dispatch_task(s, &task, &cid, &message);
    if let Err(error) = &outcome {
        if error.starts_with("shared_admission_recovery_required:") {
            return Err(fail(error, true));
        }
    }

    let accepted = outcome.is_ok();
    let now = now_ms();
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|_| fail("Could not record webhook completion.", accepted))?;
    let counted=tx.execute("UPDATE scheduled_task_webhook_pending SET state=?1 WHERE delivery_id=?2 AND state NOT IN ('completed','failed')",params![if accepted{"completed"}else{"failed"},delivery]).map_err(|_|fail("Could not record webhook completion.",accepted))?;
    let exists = tx
        .query_row(
            "SELECT 1 FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
            [delivery],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| fail("Could not record webhook completion.", accepted))?
        .is_some();
    if counted > 0 || !exists {
        record_outcome(&tx, &task, &outcome, started, now)
            .map_err(|_| fail("Could not record webhook completion.", accepted))?;
    }
    tx.commit()
        .map_err(|_| fail("Could not record webhook completion.", accepted))?;
    drop(db);
    s.streams.ops.wake_scheduler();
    outcome
        .map(|_| ())
        .map_err(|_| fail("The run failed to start.", false))
}

/// Finish only the admitted task row, even if a first binding or concurrent edit changed it.
fn record_outcome(
    db: &rusqlite::Connection,
    active: &Value,
    outcome: &Result<String, String>,
    started: i64,
    now: i64,
) -> Reply {
    let id = active["id"]
        .as_str()
        .ok_or_else(|| failure("Task identity missing", None))?;
    let mode = thread_mode(active);
    // Match the run's original row, preserving concurrent edits and never stamping a replacement.
    let current = get(db, KIND, id).ok();
    let mut stored = current.is_some();
    let mut task = current.unwrap_or_else(|| active.clone());
    if stored && (task["createdAt"] != active["createdAt"] || task["lastRunAt"] != iso(started)) {
        return Ok(task);
    }
    if let Ok(thread) = &outcome {
        if stored
            && active["threadId"].is_null()
            && mode != "new_thread"
            && task["projectId"] == active["projectId"]
            && task["threadId"].is_null()
            && thread_mode(&task) != "new_thread"
        {
            task["threadId"] = json!(thread);
            // Deletion may have run after launch committed, before the first binding existed.
            let deleted = db.query_row("SELECT NOT EXISTS(SELECT 1 FROM records WHERE kind='thread' AND id=?1 AND json_extract(body,'$.deletedAt') IS NULL)", [thread], |row| row.get::<_,bool>(0)).unwrap_or(false);
            if deleted && stored {
                remove_task(db, id)?;
                stored = false;
                task["threadId"] = active["threadId"].clone();
            }
        }
    }
    task["lastRunAt"] = json!(iso(started));
    task["runCount"] = json!(task["runCount"].as_i64().unwrap_or(0) + 1);
    match &outcome {
        Ok(_) => {
            task["lastRunStatus"] = json!("succeeded");
            task["lastRunError"] = Value::Null;
        }
        Err(e) => {
            task["lastRunStatus"] = json!("failed");
            task["lastRunError"] = json!(e.chars().take(500).collect::<String>());
        }
    }
    // A one-time task counts from the run's start, so an early manual run keeps its slot.
    let from = if task["schedule"]["type"] == "once" {
        started
    } else {
        now
    };
    task["nextRunAt"] = if task["enabled"] == true {
        next_run(&task["schedule"], from)
            .map(|t| json!(iso(t)))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    task["updatedAt"] = json!(iso(now));
    if stored {
        put(db, KIND, id, "", &task).map_err(|e| failure(&e, Some(id)))?;
    }
    Ok(task)
}

fn due(s: &State, now: i64) -> (Vec<String>, Option<i64>) {
    let tasks = list(s).unwrap_or_default();
    let mut ready = Vec::new();
    let mut soonest = None::<i64>;
    for t in tasks.iter().filter(|t| t["enabled"] == true) {
        let Some(at) = t["nextRunAt"]
            .as_str()
            .and_then(|v| {
                time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339).ok()
            })
            .map(|v| (v.unix_timestamp_nanos() / 1_000_000) as i64)
        else {
            continue;
        };
        if at <= now {
            ready.push(t["id"].as_str().unwrap_or("").to_owned());
        } else {
            soonest = Some(soonest.map_or(at, |s| s.min(at)));
        }
    }
    if tasks.iter().any(|t| t["schedule"]["type"] == "webhook") {
        if let Some(at) = crate::webhook::next_due(s) {
            soonest = Some(soonest.map_or(at, |v| v.min(at)));
        }
        if let Some(at) = crate::webhook_native::next_due(s) {
            soonest = Some(soonest.map_or(at, |v| v.min(at)));
        }
    }
    (ready, soonest)
}

/// Starts the timer loop once per server. It sleeps until the next firing, a change to a task, or 30 s.
pub fn ensure_started(s: &Arc<State>) {
    if s.streams
        .ops
        .scheduler
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    tokio::spawn(crate::background::refresh_health(s.clone()));
    let cleanup_state = s.clone();
    tokio::spawn(async move {
        let mut changes = cleanup_state.changed.subscribe();
        let mut activity = cleanup_state.activity.subscribe();
        loop {
            if !cleanup_state.background_needed() {
                if (tokio::select! { r=changes.changed()=>r, r=activity.changed()=>r }).is_err() {
                    return;
                }
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {},
                _ = changes.changed() => { continue; },
                _ = activity.changed() => { continue; },
            }
            let s = cleanup_state.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let enabled = crate::settings::read(&s).is_ok_and(|v| {
                    v["worktreeCleanup"]["mode"] == "custom"
                        || v["storageCleanup"]
                            .as_object()
                            .is_some_and(|r| r.values().any(|v| v.is_number() || v == true))
                        || v["projectSettingsOverrides"].as_object().is_some_and(|r| {
                            r.values().any(|v| v["worktreeCleanup"]["mode"] == "custom")
                        })
                });
                if enabled {
                    let _ = crate::ops::cleanup_automatic(&s);
                }
            })
            .await;
        }
    });
    let s = s.clone();
    tokio::spawn(async move {
        let mut changes = s.changed.subscribe();
        let mut activity = s.activity.subscribe();
        // Auto-settle thresholds are in days; every client connection wakes this loop, so a
        // sweep of every thread runs at most once a minute (or when settings change).
        let mut settled_at = 0i64;
        let mut settled_settings = Value::Null;
        loop {
            if s.background_needed() {
                let now = now_ms();
                let settings = crate::settings::read(&s).unwrap_or(Value::Null);
                if now.saturating_sub(settled_at) >= SETTLE_INTERVAL_MS
                    || settings != settled_settings
                {
                    let _ = crate::settlement::tick(&s, now);
                    settled_at = now;
                    settled_settings = settings;
                }
            }
            let (ready, soonest) = due(&s, now_ms());
            if soonest.is_some_and(|at| at <= now_ms()) {
                crate::webhook::tick(&s);
                crate::webhook_native::tick(&s);
            }
            let limited = crate::limits::tick(&s);
            let soonest = match (soonest, limited) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            for id in ready {
                let _ = fire(&s, &id);
            }
            if soonest.is_none() && !s.background_needed() {
                tokio::select! { _ = changes.changed() => {},
                _ = activity.changed() => {}, _ = s.streams.ops.scheduler_wake.notified() => {} }
                continue;
            }
            let wait = soonest.map_or(30_000, |t| {
                if s.background_needed() {
                    (t - now_ms()).clamp(200, 30_000)
                } else {
                    (t - now_ms()).max(200)
                }
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
                _ = s.streams.ops.scheduler_wake.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_resolution_matches_the_executed_reference_functions() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/task-modes.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let existing = (!case["existing"].is_null()).then_some(&case["existing"]);
            assert_eq!(
                resolve_thread_mode(&case["input"], existing),
                case["expected"].as_str().unwrap(),
                "{case}"
            );
        }
        for case in fixture["legacy"].as_array().unwrap() {
            assert_eq!(
                thread_mode(&case["task"]),
                case["expected"].as_str().unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn completion_never_resurrects_tasks_or_overwrites_a_replacement() {
        let dir = std::env::temp_dir().join(format!("o3-task-completion-{}", random_id().unwrap()));
        let state = State::open(&dir).unwrap();
        let db = state.db.lock().unwrap();
        let at = iso(1000);
        let active = json!({"id":"task","projectId":"p","createdAt":iso(0),"lastRunAt":at,"runCount":0,"threadMode":"continue","threadId":null,"enabled":false,"schedule":{"type":"interval","everyMs":60000}});
        let result = record_outcome(&db, &active, &Ok("thread".into()), 1000, 2000).unwrap();
        assert_eq!(result["lastRunStatus"], "succeeded");
        assert!(result["threadId"].is_null());
        assert!(get(&db, KIND, "task").is_err());
        let mut replacement = active.clone();
        replacement["createdAt"] = json!(iso(1500));
        replacement["lastRunAt"] = Value::Null;
        put(&db, KIND, "task", "", &replacement).unwrap();
        assert_eq!(
            record_outcome(&db, &active, &Ok("thread".into()), 1000, 2000).unwrap(),
            replacement
        );
        assert_eq!(get(&db, KIND, "task").unwrap(), replacement);
    }

    #[test]
    fn completion_preserves_edits_and_cleans_a_thread_deleted_before_binding() {
        let dir = std::env::temp_dir().join(format!("o3-task-binding-{}", random_id().unwrap()));
        let state = State::open(&dir).unwrap();
        let db = state.db.lock().unwrap();
        let active = json!({"id":"task","projectId":"p","createdAt":iso(0),"lastRunAt":iso(1000),"runCount":0,"threadMode":"continue","threadId":null,"enabled":false,"schedule":{"type":"interval","everyMs":60000}});
        put(
            &db,
            "thread",
            "thread",
            "thread",
            &json!({"id":"thread","deletedAt":null}),
        )
        .unwrap();
        let mut edited = active.clone();
        edited["threadMode"] = json!("new_thread");
        put(&db, KIND, "task", "", &edited).unwrap();
        assert!(
            record_outcome(&db, &active, &Ok("thread".into()), 1000, 2000).unwrap()["threadId"]
                .is_null()
        );
        edited = active.clone();
        edited["projectId"] = json!("new-project");
        edited["prompt"] = json!("Edited while dispatching");
        edited["runCount"] = json!(5);
        put(&db, KIND, "task", "", &edited).unwrap();
        let finished = record_outcome(&db, &active, &Ok("thread".into()), 1000, 2000).unwrap();
        assert!(finished["threadId"].is_null());
        assert_eq!(finished["projectId"], "new-project");
        assert_eq!(finished["prompt"], "Edited while dispatching");
        assert_eq!(finished["runCount"], 6);
        put(&db, KIND, "task", "", &active).unwrap();
        put(
            &db,
            "thread",
            "thread",
            "thread",
            &json!({"id":"thread","deletedAt":iso(1500)}),
        )
        .unwrap();
        assert!(
            record_outcome(&db, &active, &Ok("thread".into()), 1000, 2000).unwrap()["threadId"]
                .is_null()
        );
        assert!(get(&db, KIND, "task").is_err());
    }

    #[test]
    fn next_firing_for_intervals_and_fixed_times() {
        assert_eq!(
            next_run(&json!({"type":"interval","everyMs":90000}), 1000),
            Some(91000)
        );
        // 1970-01-01 was a Thursday (4); Sunday is 0.
        let daily = json!({"type":"fixed_time","timeOfDay":"03:30"});
        assert_eq!(next_run(&daily, 0), Some(3 * 3_600_000 + 30 * 60_000));
        assert_eq!(
            next_run(&daily, 4 * 3_600_000),
            Some(DAY_MS + 3 * 3_600_000 + 30 * 60_000)
        );
        let mondays = json!({"type":"fixed_time","timeOfDay":"9:00","weekdays":[1]});
        assert_eq!(
            next_run(&mondays, 0),
            Some(4 * DAY_MS + 9 * 3_600_000),
            "the first Monday of 1970 is the 5th"
        );
        assert_eq!(time_of_day("7:5"), None);
        assert_eq!(time_of_day("24:00"), None);
        assert_eq!(next_run(&json!({"type":"webhook"}), 0), None);
    }
}
