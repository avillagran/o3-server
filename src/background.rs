//! Durable ownership of native work. A terminal turn is not a terminal run.
use crate::events::{append, get, put};
use rusqlite::Connection;
use serde_json::{json, Value};
#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    db: &Connection,
    tid: &str,
    rid: &str,
    t: &Value,
    at: &str,
    task: &str,
    kind: &str,
    done: bool,
    status: &str,
    item: Option<&str>,
) -> Result<(), String> {
    if task.is_empty() || task.len() > 1024 {
        return Err("native background task id missing".into());
    }
    let id = format!("background-{rid}-{task}");
    let mut v=get(db,"backgroundTasks",&id).unwrap_or_else(|_|json!({"id":id,"taskId":task,"threadId":tid,"runId":rid,"providerThreadId":t["activeProviderThreadId"],"kind":kind,"awaited":kind=="subagent","startedAt":at,"completedAt":null}));
    // Duplicate start/roster events cannot resurrect a terminal task.
    if v["completedAt"].is_string() && !done {
        return Ok(());
    }
    v["status"] = json!(if done { status } else { "running" });
    v["updatedAt"] = json!(at);
    if done {
        v["completedAt"] = json!(at);
    }
    if let Some(item) = item {
        v["itemId"] = json!(item);
    }
    put(db, "backgroundTasks", &id, tid, &v)?;
    refresh(db, tid, t, at)
}
fn refresh(db: &Connection, tid: &str, t: &Value, at: &str) -> Result<(), String> {
    if let Some(id) = t["activeProviderThreadId"].as_str() {
        if let Ok(mut p) = get(db, "providerThreads", id) {
            let mut q=db.prepare("SELECT body FROM records WHERE kind='backgroundTasks' AND thread_id=? AND json_extract(body,'$.status')='running' ORDER BY rowid").map_err(|e|e.to_string())?;
            let tasks = q
                .query_map([tid], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .map(|r| {
                    r.map_err(|e| e.to_string())
                        .and_then(|b| serde_json::from_str::<Value>(&b).map_err(|e| e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            p["pendingBackgroundTasks"] = json!(tasks);
            p["updatedAt"] = json!(at);
            put(db, "providerThreads", id, tid, &p)?;
            append(db, tid, "provider-thread.updated", p, t)?;
        }
    }
    let mut thread = get(db, "thread", tid)?;
    thread["pendingBackgroundTasks"] =
        crate::events::shell_thread(db, &thread)?["pendingBackgroundTasks"].clone();
    put(db, "thread", tid, tid, &thread)?;
    append(db, tid, "thread.metadata-updated", thread.clone(), &thread)?;
    Ok(())
}
pub(crate) fn finish(
    db: &Connection,
    tid: &str,
    rid: &str,
    t: &Value,
    at: &str,
    status: &str,
) -> Result<(), String> {
    let mut q=db.prepare("SELECT id,body FROM records WHERE kind='backgroundTasks' AND thread_id=? AND json_extract(body,'$.runId')=? AND json_extract(body,'$.status')='running'").map_err(|e|e.to_string())?;
    let rows = q
        .query_map([tid, rid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(q);
    if rows.is_empty() {
        return Ok(());
    }
    for (id, b) in rows {
        let mut v: Value = serde_json::from_str(&b).map_err(|e| e.to_string())?;
        v["status"] = json!(status);
        v["completedAt"] = json!(at);
        v["resolutionReason"] =
            json!("native execution channel closed; prior effects may have occurred");
        put(db, "backgroundTasks", &id, tid, &v)?;
    }
    refresh(db, tid, t, at)
}

/// Profile-controlled provider/git work, suspended by observed power/lock state.
pub(crate) async fn refresh_health(s: std::sync::Arc<crate::store::State>) {
    use std::time::Duration;
    let delay = |ms: u64| Duration::from_millis(if ms == 0 { u64::MAX / 2 } else { ms.max(1000) });
    let mut prior = Value::Null;
    let mut health = tokio::time::Instant::now();
    let mut fetch = health;
    let mut power_at = health;
    let mut power = (false, false, false);
    let mut changes = s.changed.subscribe();
    let mut activity = s.activity.subscribe();
    let mut cached = Value::Null;
    let mut stamp = None;
    loop {
        if !s.background_needed() {
            if (tokio::select! { r=changes.changed()=>r, r=activity.changed()=>r }).is_err() {
                return;
            }
            continue;
        }
        let current = std::fs::metadata(s.dir.join("settings.json"))
            .ok()
            .and_then(|m| m.modified().ok());
        if cached.is_null() || current != stamp {
            cached = crate::settings::read(&s).unwrap_or_default();
            stamp = current;
        }
        let settings = &cached;
        let profile = crate::settings::background_resolved(settings);
        let now = tokio::time::Instant::now();
        if prior != profile {
            health = now
                + delay(
                    profile["providerHealthRefreshInterval"]
                        .as_u64()
                        .unwrap_or(300000),
                );
            fetch = now
                + delay(
                    profile["automaticGitFetchInterval"]
                        .as_u64()
                        .unwrap_or(300000),
                );
            power_at = now;
            prior = profile.clone();
        }
        if now >= power_at {
            power = host_power().await;
            let key = if s.streams.ops.active_clients() > 0 {
                "hostPowerMonitorActiveInterval"
            } else {
                "hostPowerMonitorIdleInterval"
            };
            power_at = now + delay(profile[key].as_u64().unwrap_or(30000));
        }
        let paused = profile["pauseWhenHostLocked"] == true && power.0
            || profile["pauseWhenHostLowPower"] == true && power.1
            || profile["pauseWhenOnBattery"] == true && power.2
            || profile["pauseWhenClientLowPower"] == true && s.streams.ops.client_low_power();
        if !paused && now >= health {
            let provider = s.provider.lock().unwrap().clone();
            if let Some(p) = provider {
                let _ = tokio::time::timeout(Duration::from_secs(20), p.refresh()).await;
            }
            crate::setup::check_versions(&s).await;
            s.notify();
            health = tokio::time::Instant::now()
                + delay(
                    profile["providerHealthRefreshInterval"]
                        .as_u64()
                        .unwrap_or(300000),
                );
        }
        if !paused && now >= fetch {
            let roots = crate::projects::snapshot(&s)
                .ok()
                .and_then(|v| v["projects"].as_array().cloned())
                .unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            let viewed = s.streams.ops.viewed_projects();
            for p in roots {
                if !viewed.contains(p["id"].as_str().unwrap_or("")) {
                    continue;
                }
                if let Some(root) = p["workspaceRoot"]
                    .as_str()
                    .filter(|r| seen.insert((*r).to_owned()))
                {
                    let root = std::path::Path::new(root);
                    let _guard = crate::proc::lock_repo(root).await;
                    let _ = crate::proc::git(
                        root,
                        &["fetch", "--all", "--prune"],
                        crate::proc::Opts {
                            timeout: Duration::from_secs(20),
                            ..crate::proc::Opts::READ
                        },
                    )
                    .await;
                }
            }
            s.notify();
            fetch = tokio::time::Instant::now()
                + delay(
                    profile["automaticGitFetchInterval"]
                        .as_u64()
                        .unwrap_or(300000),
                );
        }
        let deadline = if paused {
            power_at
        } else {
            health.min(fetch).min(power_at)
        };
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {},
            _ = changes.changed() => {},
            _ = activity.changed() => {},
        }
    }
}
async fn host_power() -> (bool, bool, bool) {
    let root = std::env::var_os("O3_POWER_SUPPLY_DIRECTORY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/sys/class/power_supply".into());
    let mut battery = false;
    let mut low = false;
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let read = |name: &str| {
                std::fs::read_to_string(e.path().join(name))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            if read("type") == "Battery" && read("status") == "Discharging" {
                battery = true;
                low |= read("capacity").parse::<u8>().is_ok_and(|v| v <= 20);
            }
        }
    }
    let mut locked = false;
    if let Ok(session) = std::env::var("XDG_SESSION_ID") {
        if !session.is_empty() && !session.starts_with('-') {
            locked = crate::proc::run(
                "loginctl",
                std::path::Path::new("/"),
                &["show-session", &session, "-p", "LockedHint", "--value"],
                crate::proc::Opts {
                    timeout: std::time::Duration::from_secs(2),
                    cap: 1024,
                    allow_truncate: false,
                },
                &[],
            )
            .await
            .is_ok_and(|o| o.ok() && o.text().trim() == "yes");
        }
    }
    (locked, low, battery)
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use std::future::Future;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[tokio::test]
    async fn disconnected_worker_has_no_periodic_polls_or_child_processes() {
        let dir = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let s = Arc::new(crate::store::State::open(&dir).unwrap());
        // A one-second custom deadline must never run without demand.
        crate::settings::update(&s, &json!({"patch":{"backgroundActivity":{"profile":"custom","baseProfile":"balanced","overrides":{"providerHealthRefreshInterval":1000,"automaticGitFetchInterval":1000,"hostPowerMonitorIdleInterval":1000}}}})).unwrap();
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let mut worker = Box::pin(refresh_health(s.clone()));
        let task = tokio::spawn(std::future::poll_fn(move |cx| {
            count.fetch_add(1, Ordering::SeqCst);
            worker.as_mut().poll(cx)
        }));
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        assert_eq!(
            polls.load(Ordering::SeqCst),
            1,
            "idle worker woke periodically"
        );
        task.abort();
        let _ = task.await;
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
