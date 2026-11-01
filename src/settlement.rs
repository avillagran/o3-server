//! Conservative idle settlement. Pending work always wins over age.
use crate::{
    events::{append, put},
    store::{iso, State},
};
use serde_json::{json, Value};
pub(crate) fn tick(s: &State, now: i64) -> Result<(), String> {
    let settings = crate::settings::read(s)?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut q = tx
        .prepare("SELECT id,body FROM records WHERE kind='thread'")
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(q);
    for (tid, body) in rows {
        let mut t: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if [
            "deletedAt",
            "archivedAt",
            "settledAt",
            "settledOverride",
            "pinnedAt",
            "autoSettleDisabledAt",
            "linkedPullRequest",
        ]
        .iter()
        .any(|k| !t[k].is_null())
            || t["pullRequests"].as_array().is_some_and(|a| !a.is_empty())
        {
            continue;
        }
        let overrides =
            &settings["projectSettingsOverrides"][t["projectId"].as_str().unwrap_or("")];
        let Some(days) = overrides
            .get("sidebarAutoSettleAfterDays")
            .unwrap_or(&settings["sidebarAutoSettleAfterDays"])
            .as_i64()
            .filter(|d| *d > 0 && *d <= 36500)
        else {
            continue;
        };
        let blocked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE thread_id=? AND ((kind='runs' AND json_extract(body,'$.status') NOT IN ('completed','rolled_back')) OR (kind='runtimeRequests' AND json_extract(body,'$.status')='pending') OR (kind='subagents' AND json_extract(body,'$.status') NOT IN ('completed','failed','cancelled')) OR (kind='controlOutbox' AND json_extract(body,'$.status') IN ('pending','delivery-unknown'))))",[&tid],|r|r.get(0)).map_err(|e|e.to_string())?;
        if blocked
            || s.active
                .lock()
                .unwrap()
                .keys()
                .any(|rid| crate::events::get(&tx, "runs", rid).is_ok_and(|r| r["threadId"] == tid))
        {
            continue;
        }
        let stamp:Option<String>=tx.query_row("SELECT max(stamp) FROM (SELECT json_extract(body,'$.updatedAt') stamp FROM records WHERE thread_id=? UNION ALL SELECT json_extract(body,'$.completedAt') FROM records WHERE thread_id=?)",[&tid,&tid],|r|r.get(0)).map_err(|e|e.to_string())?;
        let Some(last) = stamp
            .and_then(|v| {
                time::OffsetDateTime::parse(&v, &time::format_description::well_known::Rfc3339).ok()
            })
            .map(|v| v.unix_timestamp() * 1000)
        else {
            continue;
        };
        if now.saturating_sub(last) < days * 86_400_000 {
            continue;
        }
        t["settledAt"] = json!(iso(now));
        put(&tx, "thread", &tid, &tid, &t)?;
        append(&tx, &tid, "thread.settled", t.clone(), &t)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_threads_settle_only_without_pending_work_or_opt_out() {
        let dir =
            std::env::temp_dir().join(format!("o3-settle-{}", crate::store::random_id().unwrap()));
        let s = State::open(&dir).unwrap();
        {
            let db = s.db.lock().unwrap();
            for id in [
                "idle", "queued", "approval", "subagent", "disabled", "pinned", "active", "recent",
                "unknown",
            ] {
                let mut t = json!({"id":id,"projectId":"p","updatedAt":"2000-01-01T00:00:00Z"});
                match id {
                    "disabled" => t["autoSettleDisabledAt"] = json!("2000-01-01T00:00:00Z"),
                    "pinned" => t["pinnedAt"] = json!("2000-01-01T00:00:00Z"),
                    "active" => t["settledOverride"] = json!("active"),
                    "recent" => t["updatedAt"] = json!(iso(crate::store::now_ms())),
                    _ => {}
                }
                put(&db, "thread", id, id, &t).unwrap();
            }
            for (kind, id, status) in [
                ("runs", "queued", "queued"),
                ("runtimeRequests", "approval", "pending"),
                ("subagents", "subagent", "running"),
                ("controlOutbox", "unknown", "delivery-unknown"),
            ] {
                put(&db, kind, id, id, &json!({"id":id,"status":status})).unwrap();
            }
        }
        tick(&s, crate::store::now_ms()).unwrap();
        let db = s.db.lock().unwrap();
        for id in [
            "idle", "queued", "approval", "subagent", "disabled", "pinned", "active", "recent",
            "unknown",
        ] {
            let t = crate::events::get(&db, "thread", id).unwrap();
            assert_eq!(t["settledAt"].is_string(), id == "idle", "{id}");
        }
        drop(db);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
