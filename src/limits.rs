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
    .map(|t| t.unix_timestamp() * 1000)
}
pub(crate) fn tick(s: &Arc<State>) -> Option<i64> {
    let settings = crate::settings::read(s).ok()?;
    let rows = {
        let db = s.db.lock().unwrap();
        let mut q=db.prepare("SELECT body FROM records WHERE kind='thread' AND json_extract(body,'$.limitRecovery.status')='limited'").ok()?;
        let rows = q.query_map([], |r| r.get::<_, String>(0)).ok()?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };
    let mut next = None;
    for body in rows {
        let Ok(mut t) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
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
        let overrides =
            &settings["projectSettingsOverrides"][t["projectId"].as_str().unwrap_or("")];
        let enabled = overrides
            .get("autoResumeLimitedThreads")
            .unwrap_or(&settings["autoResumeLimitedThreads"])
            == true;
        let allowed = t["deletedAt"].is_null() && t["archivedAt"].is_null();
        let rid = t["limitRecovery"]["runId"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let retry = enabled && allowed;
        t["limitRecovery"]["status"] = json!(if retry { "ready" } else { "expired" });
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
        let command = json!({"type":"message.dispatch","commandId":format!("limit-retry-{rid}"),"threadId":tid,"messageId":format!("limit-retry-{rid}"),"text":"Continue the interrupted task after the usage limit reset. Inspect current state first; previous tools may have executed.","attachments":[],"dispatchMode":{"type":"start_immediately"}});
        if tx.commit().is_err() {
            continue;
        }
        drop(db);
        if retry {
            let _ = events::dispatch(s, &command);
        }
        s.notify();
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
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
