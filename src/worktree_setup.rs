//! WorktreeSetupTracker wire snapshots, with one latest value per subscriber.
use crate::store::{iso, now_ms, State};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
pub(crate) fn save_item(
    db: &Connection,
    tid: &str,
    thread: &Value,
    item: Value,
) -> Result<(), String> {
    let id = item["id"].as_str().ok_or("Preparation item id missing")?;
    crate::events::put(db, "turnItems", id, tid, &item)?;
    crate::events::put(
        db,
        "visibleTurnItems",
        id,
        tid,
        &json!({"position":item["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":item}),
    )?;
    crate::events::append(db, tid, "turn-item.updated", item, thread)?;
    Ok(())
}
pub(crate) fn clamp(text: &str, max: usize) -> String {
    let mut units = 0;
    if text.encode_utf16().count() <= max {
        return text.into();
    }
    let mut clamped: String = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units < max
        })
        .collect();
    clamped.push('\u{2026}');
    clamped
}
pub(crate) fn begin(db: &Connection, tid: &str, plan: &Value) -> Result<Value, String> {
    let previous:Option<String>=db.query_row("SELECT snapshot FROM thread_launches WHERE thread_id=? AND snapshot IS NOT NULL ORDER BY rowid DESC LIMIT 1",[tid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let sequence = previous
        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
        .and_then(|v| v["sequence"].as_u64())
        .map_or(0, |s| s + 1);
    let strategy = &plan["workspaceStrategy"];
    let tracked = strategy["type"] == "worktree" || plan["reusedWorktree"].is_object();
    if !tracked {
        return Ok(Value::Null);
    }
    let ids: Vec<_> = if strategy["type"] == "worktree" {
        vec!["fetch", "checkout", "setup-script", "agent"]
    } else {
        vec!["setup-script", "agent"]
    };
    let stages:Vec<_>=ids.into_iter().map(|id|json!({"id":id,"status":"pending","startedAt":null,"endedAt":null,"percent":null,"detail":null,"tail":[]})).collect();
    Ok(
        json!({"threadId":tid,"phase":"running","startedAt":iso(now_ms()),"endedAt":null,"branch":strategy["branch"],"baseRef":strategy.get("baseRef").unwrap_or(&plan["reusedWorktree"]["baseRef"]),"worktreePath":null,"setupScript":null,"stages":stages,"error":null,"sequence":sequence}),
    )
}
pub(crate) fn update(db: &Connection, id: &str, f: impl FnOnce(&mut Value)) -> Result<(), String> {
    let body: Option<String> = db
        .query_row(
            "SELECT snapshot FROM thread_launches WHERE id=?",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let Some(body) = body else {
        return Ok(());
    };
    let mut snapshot: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if !snapshot.is_object()
        || !snapshot["stages"]
            .as_array()
            .is_some_and(|stages| stages.iter().all(Value::is_object))
    {
        return Err("Invalid persisted setup snapshot".into());
    }
    f(&mut snapshot);
    snapshot["sequence"] = json!(snapshot["sequence"].as_u64().unwrap_or(0) + 1);
    db.execute(
        "UPDATE thread_launches SET snapshot=?1 WHERE id=?2",
        params![snapshot.to_string(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn stage(
    s: &State,
    id: &str,
    stage: &str,
    status: &str,
    detail: Option<&str>,
) -> Result<(), String> {
    update(&s.db.lock().unwrap(), id, |snapshot| {
        for entry in snapshot["stages"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .filter(|v| v["id"] == stage)
        {
            entry["status"] = json!(status);
            if status == "running" && entry["startedAt"].is_null() {
                entry["startedAt"] = json!(iso(now_ms()));
            }
            if matches!(status, "done" | "skipped" | "warning" | "failed") {
                entry["endedAt"] = json!(iso(now_ms()));
            }
            if let Some(detail) = detail {
                entry["detail"] = json!(clamp(detail, 200));
            }
        }
    })?;
    s.notify();
    Ok(())
}
pub(crate) fn tail(s: &State, id: &str, line: &str) -> Result<(), String> {
    update(&s.db.lock().unwrap(), id, |snapshot| {
        for stage in snapshot["stages"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .filter(|v| v["id"] == "setup-script")
        {
            if let Some(tail) = stage["tail"].as_array_mut() {
                tail.push(json!(clamp(line, 400)));
                if tail.len() > 4 {
                    tail.drain(..tail.len() - 4);
                }
            }
        }
    })?;
    s.notify();
    Ok(())
}
pub(crate) fn finish(
    db: &Connection,
    id: &str,
    phase: &str,
    error: Option<&str>,
) -> Result<(), String> {
    update(db, id, |snapshot| {
        let at = iso(now_ms());
        snapshot["phase"] = json!(phase);
        snapshot["endedAt"] = json!(at);
        snapshot["error"] = error.map(|e| json!(clamp(e, 1000))).unwrap_or(Value::Null);
        for entry in snapshot["stages"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .filter(|v| v["status"] == "running")
        {
            entry["status"] = json!(match phase {
                "done" => "done",
                "cancelled" => "skipped",
                _ => "failed",
            });
            entry["endedAt"] = json!(at);
        }
    })
}
pub struct Cursor {
    tid: String,
    last: Option<(i64, u64, bool)>,
    waiting: bool,
}
pub(crate) fn open(p: &Value) -> Result<Cursor, Value> {
    Ok(Cursor {
        tid: crate::p2::str_field(p, "threadId")?.into(),
        last: None,
        waiting: false,
    })
}
impl Cursor {
    pub(crate) fn ack(&mut self) {
        self.waiting = false;
    }
    pub(crate) fn is_full(&self) -> bool {
        self.waiting
    }
    pub(crate) fn next(&mut self, s: &State) -> crate::terminal::Step {
        if self.waiting {
            return crate::terminal::Step::Idle;
        }
        let row:Option<(i64,Option<String>)>=s.reader.lock().unwrap().query_row("SELECT rowid,snapshot FROM thread_launches WHERE thread_id=? AND snapshot IS NOT NULL ORDER BY rowid DESC LIMIT 1",[&self.tid],|r|Ok((r.get(0)?,r.get(1)?))).optional().ok().flatten();
        let (row, snapshot) = row.unwrap_or((0, None));
        let snapshot = snapshot
            .and_then(|v| serde_json::from_str::<Value>(&v).ok())
            .unwrap_or(Value::Null);
        let expired = snapshot["endedAt"]
            .as_str()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .is_some_and(|v| now_ms() - v.timestamp_millis() >= 30_000);
        let visible = !snapshot.is_null() && !expired;
        let identity = if visible {
            (row, snapshot["sequence"].as_u64().unwrap_or(0), true)
        } else {
            (0, 0, false)
        };
        if self.last == Some(identity) {
            return crate::terminal::Step::Idle;
        }
        self.last = Some(identity);
        self.waiting = true;
        crate::terminal::Step::Values(vec![if visible { snapshot } else { Value::Null }])
    }
}
