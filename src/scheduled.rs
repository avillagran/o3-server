//! Scheduled tasks: stored definitions plus an in-process timer that launches threads.
//!
//! Supported schedules are `interval` and `fixed_time` (UTC, optionally on chosen weekdays).
//! Webhook schedules need an inbound endpoint and secret handling that this server does not
//! have, so defining one is refused instead of accepted and never fired.
use crate::{
    events::{get, put},
    store::{iso, now_ms, random_id, State},
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

type Reply = Result<Value, Value>;
const KIND: &str = "scheduledTasks";
const MIN_INTERVAL_MS: u64 = 60_000;
const MAX_TASKS: i64 = 500;
const DAY_MS: i64 = 86_400_000;

fn failure(message: &str, task: Option<&str>) -> Value {
    let mut e = json!({"_tag":"ScheduledTaskError","message":message});
    if let Some(t) = task {
        e["taskId"] = json!(t);
    }
    e
}

pub fn call(s: &Arc<State>, tag: &str, p: &Value) -> Option<Reply> {
    Some(match tag {
        "scheduledTasks.list" => list(s).map(|tasks| json!({"tasks":tasks})),
        "scheduledTasks.upsert" => upsert(s, p).map(|task| json!({"task":task})),
        "scheduledTasks.setEnabled" => set_enabled(s, p).map(|task| json!({"task":task})),
        "scheduledTasks.delete" => delete(s, p).map(|id| json!({"id":id})),
        "scheduledTasks.runNow" => run_now(s, p).map(|task| json!({"task":task})),
        _ => return None,
    })
}

fn list(s: &State) -> Result<Vec<Value>, Value> {
    let db = s.db.lock().unwrap();
    let mut q = db.prepare("SELECT body FROM records WHERE kind=? ORDER BY json_extract(body,'$.createdAt'), rowid").map_err(|e| failure(&e.to_string(), None))?;
    let rows = q
        .query_map([KIND], |r| r.get::<_, String>(0))
        .map_err(|e| failure(&e.to_string(), None))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(
            serde_json::from_str(&row.map_err(|e| failure(&e.to_string(), None))?)
                .map_err(|e| failure(&e.to_string(), None))?,
        );
    }
    Ok(out)
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
        Some("webhook") => {
            return Err(failure(
                "Webhook schedules are not supported by this server.",
                None,
            ))
        }
        _ => {
            return Err(failure(
                "A schedule of type interval or fixed_time is required.",
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

fn upsert(s: &Arc<State>, p: &Value) -> Reply {
    validate(p)?;
    let now = now_ms();
    let at = iso(now);
    let mut db = s.db.lock().unwrap();
    let tx = db
        .transaction()
        .map_err(|e| failure(&e.to_string(), None))?;
    if let Some(cid) = p["commandId"].as_str() {
        let receipt: Option<String> = tx
            .query_row(
                "SELECT result FROM receipts WHERE command_id=?",
                [format!("scheduled-upsert:{cid}")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| failure(&e.to_string(), None))?;
        if let Some(r) = receipt {
            return serde_json::from_str(&r).map_err(|e| failure(&e.to_string(), None));
        }
    }
    let id = p["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| random_id().unwrap_or_default());
    let existing = get(&tx, KIND, &id).ok();
    if existing.is_none() && p["requireExisting"] == true {
        return Err(failure("The scheduled task no longer exists.", Some(&id)));
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
    let old = existing.clone().unwrap_or_else(|| json!({"createdAt":at,"lastRunAt":null,"lastRunStatus":"never","lastRunError":null,"runCount":0}));
    let enabled = p["enabled"] == true;
    let mut task = json!({"id":id,"title":p["title"].as_str().map(str::trim),"prompt":p["prompt"].as_str().map(str::trim),"enabled":enabled,"schedule":p["schedule"],"projectId":p["projectId"],"threadId":p.get("threadId").cloned().unwrap_or(Value::Null),"workspaceStrategy":p["workspaceStrategy"],"modelSelection":p["modelSelection"],"runtimeMode":p["runtimeMode"],"interactionMode":p["interactionMode"],"createdBy":p.get("createdBy").cloned().unwrap_or(json!("user")),"creationSource":p.get("creationSource").cloned().unwrap_or(json!("web")),"createdAt":old["createdAt"],"updatedAt":at,"nextRunAt":Value::Null,"lastRunAt":old["lastRunAt"],"lastRunStatus":old["lastRunStatus"],"lastRunError":old["lastRunError"],"runCount":old["runCount"]});
    if enabled {
        task["nextRunAt"] = next_run(&p["schedule"], now)
            .map(|t| json!(iso(t)))
            .unwrap_or(Value::Null);
    }
    if let Some(e) = &existing {
        task["createdBy"] = e["createdBy"].clone();
        task["creationSource"] = e["creationSource"].clone();
    }
    put(&tx, KIND, &id, "", &task).map_err(|e| failure(&e, Some(&id)))?;
    let result = json!({"task":task});
    if let Some(cid) = p["commandId"].as_str() {
        tx.execute(
            "INSERT INTO receipts VALUES(?,?)",
            params![format!("scheduled-upsert:{cid}"), result.to_string()],
        )
        .map_err(|e| failure(&e.to_string(), None))?;
    }
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
    let mut task =
        get(&db, KIND, id).map_err(|_| failure("The scheduled task does not exist.", Some(id)))?;
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

fn delete(s: &Arc<State>, p: &Value) -> Result<String, Value> {
    let id = p["id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    let n =
        s.db.lock()
            .unwrap()
            .execute(
                "DELETE FROM records WHERE kind=? AND id=?",
                params![KIND, id],
            )
            .map_err(|e| failure(&e.to_string(), Some(id)))?;
    if n == 0 {
        return Err(failure("The scheduled task does not exist.", Some(id)));
    }
    s.streams.ops.wake_scheduler();
    Ok(id.to_owned())
}

fn run_now(s: &Arc<State>, p: &Value) -> Reply {
    let id = p["id"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| failure("id is required", None))?;
    fire(s, id)
}

/// Starts the task's thread (or message) and records the outcome on the task.
fn fire(s: &Arc<State>, id: &str) -> Reply {
    let task = get(&s.db.lock().unwrap(), KIND, id)
        .map_err(|_| failure("The scheduled task does not exist.", Some(id)))?;
    let number = task["runCount"].as_i64().unwrap_or(0) + 1;
    let cid = format!("scheduled:{id}:{number}");
    let message = format!("scheduled-{id}-{number}");
    let outcome = match task["threadId"].as_str() {
        Some(thread) => crate::events::dispatch(s, &json!({"type":"message.dispatch","commandId":cid,"messageId":message,"threadId":thread,"text":task["prompt"],"attachments":[],"scheduledTaskId":id,"dispatchMode":{"type":"start_immediately"}})).map(|_| ()),
        None => crate::events::launch(s, &json!({"commandId":cid,"projectId":task["projectId"],"title":task["title"],"modelSelection":task["modelSelection"],"runtimeMode":task["runtimeMode"],"interactionMode":task["interactionMode"],"workspaceStrategy":task["workspaceStrategy"],"creationSource":task["creationSource"],"initialMessage":{"messageId":message,"text":task["prompt"],"attachments":[]}})).map(|_| ()),
    };
    let now = now_ms();
    let db = s.db.lock().unwrap();
    // The task may have been deleted or edited while launching; update what is stored now.
    let mut task =
        get(&db, KIND, id).map_err(|_| failure("The scheduled task does not exist.", Some(id)))?;
    task["lastRunAt"] = json!(iso(now));
    task["runCount"] = json!(number);
    match &outcome {
        Ok(()) => {
            task["lastRunStatus"] = json!("succeeded");
            task["lastRunError"] = Value::Null;
        }
        Err(e) => {
            task["lastRunStatus"] = json!("failed");
            task["lastRunError"] = json!(e.chars().take(500).collect::<String>());
        }
    }
    task["nextRunAt"] = if task["enabled"] == true {
        next_run(&task["schedule"], now)
            .map(|t| json!(iso(t)))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    task["updatedAt"] = json!(iso(now));
    put(&db, KIND, id, "", &task).map_err(|e| failure(&e, Some(id)))?;
    drop(db);
    s.streams.ops.wake_scheduler();
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
    let s = s.clone();
    tokio::spawn(async move {
        loop {
            let _ = crate::settlement::tick(&s, now_ms());
            let (ready, soonest) = due(&s, now_ms());
            for id in ready {
                let _ = fire(&s, &id);
            }
            let wait = soonest.map_or(30_000, |t| (t - now_ms()).clamp(200, 30_000));
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
