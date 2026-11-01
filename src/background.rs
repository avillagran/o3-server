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

/// Provider health probes follow the effective activity profile even without a quota subscriber.
/// No automatic git fetch or host-power monitoring is implied by this timer.
pub(crate) async fn refresh_health(s: std::sync::Arc<crate::store::State>) {
    let mut changes = s.changed.subscribe();
    let interval = |s: &crate::store::State| {
        crate::settings::read(s)
            .ok()
            .and_then(|v| v["providerHealthRefreshInterval"].as_u64())
            .unwrap_or(300_000)
    };
    let delay = |ms: u64| {
        std::time::Duration::from_millis(if ms == 0 { u64::MAX / 2 } else { ms.max(1000) })
    };
    let mut ms = interval(&s);
    let mut next = tokio::time::Instant::now() + delay(ms);
    loop {
        tokio::select! {
            _=tokio::time::sleep_until(next)=>{
                let provider=s.provider.lock().unwrap().clone();
                if let Some(p)=provider {let _=tokio::time::timeout(std::time::Duration::from_secs(20),p.refresh()).await;s.notify();}
                crate::setup::check_versions(&s).await;
                next=tokio::time::Instant::now()+delay(ms);
            }
            result=changes.changed()=>{
                if result.is_err(){return}
                let current=interval(&s);
                if current!=ms{ms=current;next=tokio::time::Instant::now()+delay(ms);}
            }
        }
    }
}
