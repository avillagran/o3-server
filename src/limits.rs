//! Deadline-driven usage-limit recovery. Only explicit settings authorize a retry.
use crate::{
    events,
    store::{iso, now_ms, State},
};
use serde_json::{json, Value};
use std::sync::Arc;
fn millis(value: &Value) -> Option<i64> {
    time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
    .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
}
/// The native worker decision; delivery remains subject to receipt-time admission.
pub(crate) fn recovery_command(
    t: &Value,
    automatic: bool,
    now: i64,
    snooze: bool,
) -> Option<Value> {
    if t["status"] != "failed"
        || t["lastErrorClass"] != "usage_limit"
        || t["latestRunId"].as_str().is_none_or(str::is_empty)
        || t["usageLimitResetAt"].as_str().is_none_or(str::is_empty)
        || !t["archivedAt"].is_null()
        || t["settledOverride"] == "settled"
        || !t["pendingRuntimeRequest"].is_null()
    {
        return None;
    }
    let reset = millis(&t["usageLimitResetAt"])?;
    let completed = if t["latestRunCompletedAt"].is_null() {
        &t["updatedAt"]
    } else {
        &t["latestRunCompletedAt"]
    };
    if reset <= millis(completed)? {
        return None;
    }
    let identity = format!(
        "{}:{}:{reset}",
        t["id"].as_str()?,
        t["latestRunId"].as_str()?
    );
    let recovery = &t["limitRecovery"];
    if recovery["runId"] != t["latestRunId"] || recovery["resetAt"] != t["usageLimitResetAt"] {
        if now.saturating_sub(reset) > 86_400_000 || !automatic && (!snooze || reset <= now) {
            return None;
        }
        return Some(
            json!({"type":"thread.metadata.update","commandId":format!("limit-arm:{identity}"),
            "threadId":t["id"],"limitRecovery":{"runId":t["latestRunId"],"resetAt":t["usageLimitResetAt"],
            "autoResume":automatic,"snooze":snooze && reset>now}}),
        );
    }
    if recovery["autoResume"] != true
        || reset > now
        || millis(&t["snoozedUntil"]).is_some_and(|at| at > now)
    {
        return None;
    }
    let delivery = format!(
        "{identity}:{}",
        recovery["requestId"].as_str().unwrap_or("legacy")
    );
    let mut command = json!({"type":"message.dispatch","commandId":format!("limit-resume:{delivery}"),
        "messageId":format!("limit-resume:{delivery}"),"threadId":t["id"],
        "usageLimitContinuationOfRunId":t["latestRunId"],"text":"Continue where you left off.",
        "attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"agent","creationSource":"server"});
    if let Some(id) = recovery.get("requestId") {
        command["usageLimitRecoveryRequestId"] = id.clone();
    }
    Some(command)
}
fn candidate(db: &rusqlite::Connection, t: &Value) -> Result<Value, String> {
    let tid = t["id"].as_str().ok_or("thread id missing")?;
    let runs = crate::context_transfer::list(db, "runs", tid)?;
    let run = runs
        .iter()
        .filter(|run| {
            run["status"] != "queued"
                && !(run["status"] == "cancelled" && run["startedAt"].is_null())
        })
        .max_by_key(|run| {
            (
                millis(&run["completedAt"]).unwrap_or(i64::MAX),
                run["ordinal"].as_u64().unwrap_or(0),
            )
        });
    let mut result = events::shell_thread(db, t)?;
    if let Some(run) = run {
        result["latestRunId"] = run["id"].clone();
        result["status"] = run["status"].clone();
        result["latestRunRequestedAt"] = run["requestedAt"].clone();
        result["latestRunStartedAt"] = run["startedAt"].clone();
        result["latestRunCompletedAt"] = run["completedAt"].clone();
        let mut query = db.prepare("SELECT body FROM records WHERE kind='turnItems' AND thread_id=?1 AND json_extract(body,'$.type')='error' AND json_extract(body,'$.status')='failed' AND json_extract(body,'$.runId')=?2 AND json_extract(body,'$.nodeId') IS ?3 ORDER BY rowid").map_err(|error| error.to_string())?;
        let rows = query
            .query_map(
                rusqlite::params![tid, run["id"].as_str(), run["rootNodeId"].as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let items = rows
            .into_iter()
            .map(|body| serde_json::from_str::<Value>(&body).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let failure = items
            .iter()
            .filter(|item| {
                run["status"] == "failed"
                    && item["type"] == "error"
                    && item["status"] == "failed"
                    && item["runId"] == run["id"]
                    && item["nodeId"] == run["rootNodeId"]
            })
            .max_by_key(|item| {
                (
                    millis(&item["updatedAt"]).unwrap_or(0),
                    item["ordinal"].as_u64().unwrap_or(0),
                    item["id"].as_str().unwrap_or(""),
                )
            });
        result["lastErrorClass"] =
            failure.map_or(Value::Null, |item| item["failure"]["class"].clone());
        result["usageLimitResetAt"] = failure
            .filter(|item| item["failure"]["class"] == "usage_limit")
            .map_or(Value::Null, |item| item["failure"]["resetAt"].clone());
    }
    let pending: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='runtimeRequests' AND thread_id=? AND json_extract(body,'$.status')='pending')", [tid], |row| row.get(0)).map_err(|error| error.to_string())?;
    if pending {
        result["pendingRuntimeRequest"] = json!({"status":"pending"});
    }
    Ok(result)
}

pub(crate) fn disarm(
    db: &rusqlite::Connection,
    t: &mut Value,
    c: &Value,
    at: &str,
) -> Result<bool, String> {
    let previous = t["limitRecovery"].clone();
    let mut next = previous.clone();
    let shell = candidate(db, t)?;
    if shell["status"] == "failed"
        && shell["lastErrorClass"] == "usage_limit"
        && millis(&shell["usageLimitResetAt"]).is_some()
        && (next["runId"] != shell["latestRunId"] || next["resetAt"] != shell["usageLimitResetAt"])
    {
        next = json!({"runId":shell["latestRunId"],"resetAt":shell["usageLimitResetAt"],"autoResume":false,"snooze":false});
    }
    if next.is_object() && (next != previous || next["autoResume"] == true) {
        next["autoResume"] = json!(false);
        next["requestId"] = c["commandId"].clone();
        t["limitRecovery"] = next;
        t["updatedAt"] = json!(at);
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn admit_continuation(
    db: &rusqlite::Connection,
    t: &mut Value,
    c: &Value,
    now: i64,
) -> Result<bool, String> {
    let Some(source) = c.get("usageLimitContinuationOfRunId") else {
        return Ok(true);
    };
    let shell = candidate(db, t)?;
    let recovery = &t["limitRecovery"];
    let run = events::get(db, "runs", source.as_str().unwrap_or("")).ok();
    let future_snooze = millis(&t["snoozedUntil"]).is_some_and(|at| at > now);
    let valid = shell["latestRunId"] == *source
        && shell["status"] == "failed"
        && shell["lastErrorClass"] == "usage_limit"
        && recovery["autoResume"] == true
        && recovery["requestId"] == c["usageLimitRecoveryRequestId"]
        && recovery["runId"] == *source
        && recovery["resetAt"] == shell["usageLimitResetAt"]
        && millis(&recovery["resetAt"]).is_some_and(|at| at <= now)
        && t["archivedAt"].is_null()
        && t["deletedAt"].is_null()
        && t["settledOverride"] != "settled"
        && run.is_some_and(|run| run["providerInstanceId"] == t["providerInstanceId"])
        && shell["pendingRuntimeRequest"].is_null()
        && !future_snooze;
    if !valid
        && recovery["autoResume"] == true
        && recovery["requestId"] == c["usageLimitRecoveryRequestId"]
        && recovery["runId"] == *source
        && future_snooze
    {
        t["limitRecovery"]["requestId"] = c["commandId"].clone();
    }
    Ok(valid)
}

/// Only tagged continuations proven not to have started receive a fresh identity.
pub(crate) fn recover_unsent(db: &rusqlite::Connection) -> Result<(), String> {
    let mut query = db.prepare("SELECT id,thread_id FROM records WHERE kind='runs' AND json_type(body,'$.usageLimitContinuationOfRunId')='text' AND json_extract(body,'$.startedAt') IS NULL AND json_extract(body,'$.status') IN ('preparing','starting','running','waiting') ORDER BY rowid").map_err(|error| error.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(query);
    if rows.is_empty() {
        return Ok(());
    }
    let at = iso(now_ms());
    let request = format!("limit-recovery:{}", crate::store::random_id()?);
    let mut refreshed = std::collections::HashSet::new();
    for (rid, tid) in rows {
        let mut run = events::get(db, "runs", &rid)?;
        if run["status"] == "preparing" && run["executionRequested"] != true {
            continue;
        }
        let mut thread = events::get(db, "thread", &tid)?;
        if thread["limitRecovery"]["autoResume"] == true && refreshed.insert(tid.clone()) {
            thread["limitRecovery"]["requestId"] = json!(request);
            thread["updatedAt"] = json!(at);
            events::put(db, "thread", &tid, &tid, &thread)?;
            events::append(db, &tid, "thread.metadata-updated", thread.clone(), &thread)?;
        }
        run["status"] = json!("cancelled");
        run["completedAt"] = json!(at);
        run["recoveryPending"] = json!(false);
        events::put(db, "runs", &rid, &tid, &run)?;
        events::append(db, &tid, "run.updated", run.clone(), &thread)?;
        for mut item in crate::context_transfer::list(db, "turnItems", &tid)? {
            if item["type"] != "user_message" || item["runId"] != rid {
                continue;
            }
            if matches!(
                item["status"].as_str(),
                Some("pending" | "running" | "waiting")
            ) {
                item["status"] = json!("cancelled");
                item["completedAt"] = json!(at);
            }
            item["inputIntent"] = json!("queued_turn");
            item["updatedAt"] = json!(at);
            crate::projection::save_item(db, &tid, &thread, item)?;
        }
        crate::projection::finish(db, &tid, &rid, &thread, &at, "cancelled")?;
        for kind in ["nodes", "attempts"] {
            for mut item in crate::context_transfer::list(db, kind, &tid)? {
                if item["runId"] != rid
                    || !matches!(
                        item["status"].as_str(),
                        Some("pending" | "preparing" | "starting" | "running" | "waiting")
                    )
                {
                    continue;
                }
                item["status"] = json!("cancelled");
                item["completedAt"] = json!(at);
                let id = item["id"]
                    .as_str()
                    .ok_or("recovery item id missing")?
                    .to_owned();
                events::put(db, kind, &id, &tid, &item)?;
                events::append(
                    db,
                    &tid,
                    if kind == "nodes" {
                        "node.updated"
                    } else {
                        "attempt.updated"
                    },
                    item,
                    &thread,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn update_recovery(
    db: &rusqlite::Connection,
    t: &mut Value,
    c: &Value,
    at: &str,
    automatic: bool,
) -> Result<(), String> {
    let Some(update) = c.get("limitRecovery") else {
        return Ok(());
    };
    let previous = t["limitRecovery"].clone();
    if automatic && previous["runId"] == update["runId"] && previous["resetAt"] == update["resetAt"]
    {
        return Ok(());
    }
    let mut next = Value::Null;
    if !update.is_null() {
        let shell = candidate(db, t)?;
        let reset = millis(&update["resetAt"])
            .ok_or("The provider limit changed before recovery could be configured.")?;
        if update["snooze"] == true && reset <= millis(&json!(at)).unwrap_or(0) {
            return Err("The reset time has passed. Retry the thread manually.".into());
        }
        if !update.is_object()
            || !t["archivedAt"].is_null()
            || t["settledOverride"] == "settled"
            || shell["latestRunId"] != update["runId"]
            || shell["status"] != "failed"
            || shell["lastErrorClass"] != "usage_limit"
            || shell["usageLimitResetAt"] != update["resetAt"]
            || !shell["pendingRuntimeRequest"].is_null()
            || reset
                <= millis(if shell["latestRunCompletedAt"].is_null() {
                    &shell["latestRunRequestedAt"]
                } else {
                    &shell["latestRunCompletedAt"]
                })
                .unwrap_or(i64::MAX)
        {
            return Err("The provider limit changed before recovery could be configured.".into());
        }
        let same = previous["runId"] == update["runId"] && previous["resetAt"] == update["resetAt"];
        next = update.clone();
        for flag in ["autoResume", "snooze"] {
            next[flag] = update
                .get(flag)
                .cloned()
                .unwrap_or_else(|| json!(same && previous[flag] == true));
        }
        if automatic
            && events::get(
                db,
                "turnItems",
                &format!(
                    "{}-interrupt-request",
                    update["runId"].as_str().unwrap_or("")
                ),
            )
            .is_ok()
        {
            next["autoResume"] = json!(false);
            next["snooze"] = json!(false);
        }
        next["requestId"] = c["commandId"].clone();
    }
    if next["snooze"] == true
        && millis(&next["resetAt"]).is_some_and(|reset| reset > millis(&json!(at)).unwrap_or(0))
    {
        t["snoozedUntil"] = next["resetAt"].clone();
        t["snoozedAt"] = json!(at);
    } else if previous["snooze"] == true
        && t["snoozedUntil"] == previous["resetAt"]
        && !t["snoozedUntil"].is_null()
    {
        t["snoozedUntil"] = Value::Null;
        t["snoozedAt"] = Value::Null;
        t["lastSnoozeWakeAt"] = json!(at);
    }
    t["limitRecovery"] = next;
    Ok(())
}
pub(crate) fn tick(s: &Arc<State>) -> Option<i64> {
    let settings = crate::settings::read(s).ok()?;
    let rows = {
        let db = s.db.lock().unwrap();
        let mut q=db.prepare("SELECT body FROM records WHERE kind='thread' AND (json_extract(body,'$.limitRecovery.status')='limited' OR json_type(body,'$.usageLimitResetAt')='text')").ok()?;
        let rows = q.query_map([], |r| r.get::<_, String>(0)).ok()?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };
    let mut next = None;
    for body in rows {
        let Ok(mut t) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        let overrides =
            &settings["projectSettingsOverrides"][t["projectId"].as_str().unwrap_or("")];
        let flag = |key: &str| overrides.get(key).unwrap_or(&settings[key]) == true;
        if !flag("autoResumeLimitedThreads")
            && !flag("snoozeLimitedThreads")
            && t["limitRecovery"]["autoResume"] != true
            && t["limitRecovery"]["status"] != "limited"
        {
            continue;
        }
        let candidate = candidate(&s.db.lock().unwrap(), &t).ok();
        if let Some(command) = candidate.as_ref().and_then(|candidate| {
            recovery_command(
                candidate,
                flag("autoResumeLimitedThreads"),
                now_ms(),
                flag("snoozeLimitedThreads"),
            )
        }) {
            let result = if command["type"] == "thread.metadata.update" {
                events::dispatch_auto_arm(s, &command)
            } else {
                events::dispatch(s, &command)
            };
            if result.is_ok() {
                s.streams.ops.scheduler_wake.notify_one();
                s.notify();
            }
            continue;
        }
        if let Some(at) = millis(&t["usageLimitResetAt"]).filter(|at| *at > now_ms()) {
            next = Some(next.map_or(at, |n: i64| n.min(at)));
        }
        // Legacy records without a run may expire, but cannot authorize a retry.
        if t["limitRecovery"]["status"] != "limited"
            || candidate
                .as_ref()
                .is_some_and(|c| c["latestRunId"].is_string())
        {
            continue;
        }
        let Some(at) = millis(&t["limitRecovery"]["resumeAt"]) else {
            continue;
        };
        if at > now_ms() {
            next = Some(next.map_or(at, |n: i64| n.min(at)));
            continue;
        }
        let Some(tid) = t["id"].as_str().map(str::to_owned) else {
            continue;
        };
        let mut db = s.db.lock().unwrap();
        let Ok(tx) = db.transaction() else { continue };
        if crate::thread_commands::has_blocking_run(&tx, &tid).unwrap_or(true) {
            continue;
        }
        t["limitRecovery"]["status"] = json!("expired");
        if t["limitRecovery"]["snoozed"] == true
            && t["snoozedUntil"] == t["limitRecovery"]["resumeAt"]
        {
            t["snoozedUntil"] = Value::Null;
            t["snoozedAt"] = Value::Null;
            t["lastSnoozeWakeAt"] = json!(iso(now_ms()));
        }
        if events::put(&tx, "thread", &tid, &tid, &t).is_err() {
            continue;
        }
        if events::append(&tx, &tid, "thread.metadata-updated", t.clone(), &t).is_err() {
            continue;
        }
        if tx.commit().is_err() {
            continue;
        }
        drop(db);
        s.notify();
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limited_state() -> Arc<State> {
        let dir = std::env::temp_dir().join(format!(
            "limit-recovery-{}",
            crate::store::random_id().unwrap()
        ));
        let state = Arc::new(State::open(&dir).unwrap());
        let reset = iso(now_ms() - 1000);
        let completed = iso(now_ms() - 10_000);
        let db = state.db.lock().unwrap();
        events::put(&db,"thread","t","t",&json!({"id":"t","providerInstanceId":"scripted","updatedAt":completed,
            "usageLimitResetAt":reset,"limitRecovery":{"runId":"source","resetAt":reset,"autoResume":true,"snooze":false,"requestId":"old"}})).unwrap();
        events::put(&db,"runs","source","t",&json!({"id":"source","threadId":"t","providerInstanceId":"scripted","ordinal":1,"status":"failed","rootNodeId":"source-root","requestedAt":completed,"startedAt":completed,"completedAt":completed,"errorClass":"usage_limit"})).unwrap();
        events::put(&db,"turnItems","failure","t",&json!({"id":"failure","threadId":"t","runId":"source","nodeId":"source-root","ordinal":1,"type":"error","status":"failed","updatedAt":completed,"failure":{"class":"usage_limit","message":"Synthetic limit","resetAt":reset}})).unwrap();
        drop(db);
        state
    }
    #[test]
    fn restart_rearms_only_tagged_unstarted_resume_and_hides_its_old_prompt() {
        for (tagged, started) in [(true, false), (true, true), (false, false)] {
            let state = limited_state();
            let mut db = state.db.lock().unwrap();
            let at = iso(now_ms());
            let mut run = json!({"id":"resume","threadId":"t","providerInstanceId":"scripted","ordinal":2,"status":"starting","rootNodeId":"resume-root","requestedAt":at,"startedAt":null,"completedAt":null,"executionRequested":true});
            if tagged {
                run["usageLimitContinuationOfRunId"] = json!("source");
            }
            if started {
                run["startedAt"] = json!(at);
                run["status"] = json!("running");
            }
            events::put(&db, "runs", "resume", "t", &run).unwrap();
            events::put(&db,"turnItems","prompt","t",&json!({"id":"prompt","threadId":"t","runId":"resume","nodeId":null,"type":"user_message","status":"pending","ordinal":2,"updatedAt":at})).unwrap();
            crate::recovery::recover(&mut db).unwrap();
            let thread = events::get(&db, "thread", "t").unwrap();
            let prompt = events::get(&db, "turnItems", "prompt").unwrap();
            let recovery = thread["limitRecovery"].clone();
            assert_eq!(recovery["requestId"] != "old", tagged && !started);
            if tagged && !started {
                assert_eq!(prompt["status"], "cancelled");
                assert_eq!(prompt["inputIntent"], "queued_turn");
                let next =
                    recovery_command(&candidate(&db, &thread).unwrap(), true, now_ms(), false)
                        .unwrap();
                assert_eq!(next["usageLimitRecoveryRequestId"], recovery["requestId"]);
                assert!(admit_continuation(&db, &mut thread.clone(), &next, now_ms()).unwrap());
                crate::recovery::recover(&mut db).unwrap();
                assert_eq!(
                    events::get(&db, "thread", "t").unwrap()["limitRecovery"],
                    recovery
                );
            } else {
                assert_ne!(prompt["inputIntent"], "queued_turn");
            }
        }
    }
    #[test]
    fn recovery_admission_rechecks_stop_snooze_identity_and_pending_requests() {
        let state = limited_state();
        let db = state.db.lock().unwrap();
        let original = events::get(&db, "thread", "t").unwrap();
        let command =
            recovery_command(&candidate(&db, &original).unwrap(), true, now_ms(), false).unwrap();
        assert!(admit_continuation(&db, &mut original.clone(), &command, now_ms()).unwrap());
        for (key, value) in [
            ("archivedAt", json!(iso(now_ms()))),
            ("deletedAt", json!(iso(now_ms()))),
            ("settledOverride", json!("settled")),
            ("providerInstanceId", json!("different")),
            ("snoozedUntil", json!(iso(now_ms() + 60_000))),
        ] {
            let mut thread = original.clone();
            thread[key] = value;
            assert!(
                !admit_continuation(&db, &mut thread, &command, now_ms()).unwrap(),
                "{key}"
            );
            if key == "snoozedUntil" {
                assert_eq!(thread["limitRecovery"]["requestId"], command["commandId"]);
            }
        }
        let mut stopped = original.clone();
        stopped["limitRecovery"]["autoResume"] = json!(false);
        assert!(!admit_continuation(&db, &mut stopped, &command, now_ms()).unwrap());
        events::put(
            &db,
            "runtimeRequests",
            "pending",
            "t",
            &json!({"id":"pending","threadId":"t","status":"pending"}),
        )
        .unwrap();
        assert!(!admit_continuation(&db, &mut original.clone(), &command, now_ms()).unwrap());
    }
    #[test]
    fn explicit_recovery_choice_survives_a_stale_automatic_arm_and_command_prefix_is_not_authority()
    {
        let state = limited_state();
        let db = state.db.lock().unwrap();
        let mut thread = events::get(&db, "thread", "t").unwrap();
        let reset = thread["limitRecovery"]["resetAt"].clone();
        let manual = json!({"commandId":"limit-arm:manual-choice","limitRecovery":{"runId":"source","resetAt":reset,"autoResume":false}});
        update_recovery(&db, &mut thread, &manual, &iso(now_ms()), false).unwrap();
        assert_eq!(thread["limitRecovery"]["autoResume"], false);
        let retained = thread["limitRecovery"].clone();
        let automatic = json!({"commandId":"sweep","limitRecovery":{"runId":"source","resetAt":reset,"autoResume":true,"snooze":false}});
        update_recovery(&db, &mut thread, &automatic, &iso(now_ms()), true).unwrap();
        assert_eq!(thread["limitRecovery"], retained);
        thread["limitRecovery"] = Value::Null;
        events::put(&db,"turnItems","source-interrupt-request","t",&json!({"id":"source-interrupt-request","type":"run_interrupt_request","runId":"source"})).unwrap();
        update_recovery(&db, &mut thread, &automatic, &iso(now_ms()), true).unwrap();
        assert_eq!(thread["limitRecovery"]["autoResume"], false);
        assert_eq!(thread["limitRecovery"]["snooze"], false);
    }
    #[test]
    fn recovery_candidate_ignores_child_failures_and_nonfailed_items() {
        let state = limited_state();
        let db = state.db.lock().unwrap();
        let thread = events::get(&db, "thread", "t").unwrap();
        for (id, node, status) in [
            ("child", "child-root", "failed"),
            ("active", "source-root", "running"),
        ] {
            events::put(&db,"turnItems",id,"t",&json!({"id":id,"threadId":"t","runId":"source","nodeId":node,"type":"error","status":status,"ordinal":99,"updatedAt":iso(now_ms()),"failure":{"class":"unknown","message":"Unrelated failure"}})).unwrap();
        }
        let current = candidate(&db, &thread).unwrap();
        assert_eq!(current["lastErrorClass"], "usage_limit");
        assert_eq!(
            current["usageLimitResetAt"],
            thread["limitRecovery"]["resetAt"]
        );
        let mut unarmed = thread.clone();
        unarmed["limitRecovery"] = Value::Null;
        assert!(disarm(
            &db,
            &mut unarmed,
            &json!({"commandId":"stop"}),
            &iso(now_ms())
        )
        .unwrap());
        assert_eq!(unarmed["limitRecovery"]["autoResume"], false);
        assert_eq!(unarmed["limitRecovery"]["requestId"], "stop");
        assert!(
            recovery_command(&candidate(&db, &unarmed).unwrap(), true, now_ms(), false).is_none()
        );
    }
    #[test]
    fn native_worker_decisions_match_pinned_4301() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/limit-recovery-worker-4301.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["revision"],
            "4cb6945dc60ed03f567dc5daafd083e52308e82b"
        );
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 760);
        for (index, case) in cases.iter().enumerate() {
            let input = &case["input"];
            assert_eq!(
                recovery_command(
                    &input["thread"],
                    input["autoResume"] == true,
                    input["nowMs"].as_i64().unwrap(),
                    input["snooze"] == true
                )
                .unwrap_or(Value::Null),
                case["expected"],
                "native case {index}: {input}"
            );
        }
    }
    #[test]
    fn deadline_wakes_only_automatic_snooze_and_disabled_resume_stays_stopped() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let s = Arc::new(State::open(&dir).unwrap());
        let past = iso(now_ms() - 1000);
        {
            let db = s.db.lock().unwrap();
            events::put(&db,"thread","t","t",&json!({"id":"t","title":"limited","snoozedAt":past,"snoozedUntil":past,"limitRecovery":{"runId":"r","status":"limited","resumeAt":past,"snoozed":true}})).unwrap();
        }
        assert_eq!(tick(&s), None);
        let t = events::get(&s.db.lock().unwrap(), "thread", "t").unwrap();
        assert!(t["snoozedUntil"].is_null());
        assert_eq!(t["limitRecovery"]["status"], "expired");
        assert!(s.active.lock().unwrap().is_empty());
        drop(s);
        // Fixture trees are retained for private container teardown.
    }
}
