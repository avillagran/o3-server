//! Signed inbound hooks on the ordinary listener. No payload/secret logging or network delivery.
use crate::{
    events::{get, put},
    store::{now_ms, random_id, State},
};
use base64::Engine;
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub fn validate(s: &Value) -> Result<(), &'static str> {
    let sig = &s["signature"];
    if sig["header"].as_str().is_none_or(|h| {
        h.is_empty() || h.len() > 256 || h.parse::<hyper::header::HeaderName>().is_err()
    }) || !matches!(sig["encoding"].as_str(), Some("hex" | "base64"))
        || sig["prefix"]
            .as_str()
            .is_none_or(|p| p.len() > 256 || p.chars().any(char::is_control))
    {
        return Err("Webhook requires a signature header, encoding and prefix");
    }
    if sig.get("secretRef").is_some() {
        return Err("Webhook secretRef is unavailable; supply a signing secret");
    }
    if s.get("maxDeliveryAgeMinutes")
        .is_some_and(|v| !v.is_null() && v.as_u64().is_none_or(|n| !(1..=1440).contains(&n)))
    {
        return Err("invalid delivery age");
    }
    if s["filters"].as_array().is_some_and(|a| {
        a.len() > 32
            || a.iter().any(|f| {
                !matches!(
                    f["op"].as_str(),
                    Some("exists" | "equals" | "notEquals" | "in" | "startsWith")
                ) || f["path"].as_str().is_none_or(|p| {
                    p.len() > 1024
                        || !matches!(p.split('.').next(), Some("body" | "headers" | "query"))
                })
            })
    }) {
        return Err("invalid webhook filters");
    }
    if s["dedupe"]
        .get("coalesceWindowSeconds")
        .is_some_and(|v| v.as_u64().is_none_or(|n| !(10..=3600).contains(&n)))
    {
        return Err("invalid coalesce window");
    }
    Ok(())
}
fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS hook_queue(id TEXT PRIMARY KEY,task TEXT NOT NULL,coalesce TEXT NOT NULL,due INTEGER NOT NULL,expires INTEGER NOT NULL,prompt TEXT NOT NULL); CREATE INDEX IF NOT EXISTS hook_queue_due ON hook_queue(due); CREATE TABLE IF NOT EXISTS hook_replay(task TEXT NOT NULL,key TEXT NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(task,key));").map_err(|e|e.to_string())
}
pub fn prepare(db: &Connection, id: &str, task: &mut Value) -> Result<(), String> {
    if task["schedule"]["type"] != "webhook" {
        return Ok(());
    }
    initialize(db)?;
    let old = get(db, "hookCredentials", id).unwrap_or(Value::Null);
    let secret = task["schedule"]["signature"]["secret"]
        .as_str()
        .or_else(|| old["secret"].as_str())
        .filter(|v| !v.is_empty() && v.len() <= 4096)
        .ok_or("Webhook requires a signing secret")?
        .to_owned();
    let token = old["token"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(random_id()?);
    put(
        db,
        "hookCredentials",
        id,
        "",
        &json!({"secret":secret,"token":token}),
    )?;
    task["schedule"]["signature"]
        .as_object_mut()
        .ok_or("signature required")?
        .remove("secret");
    task["webhook"] =
        json!({"path":format!("/api/hooks/{id}/{token}"),"url":null,"hasSecret":true});
    Ok(())
}
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}
fn resolve(root: &Value, path: &str) -> Value {
    path.split('.').fold(root.clone(), |v, k| {
        if let Ok(n) = k.parse::<usize>() {
            v.get(n).cloned().unwrap_or(Value::Null)
        } else {
            v.get(k).cloned().unwrap_or(Value::Null)
        }
    })
}
fn render(template: &str, root: &Value, framed: bool) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some((before, tail)) = rest.split_once("{{") {
        out.push_str(before);
        let Some((path, after)) = tail.split_once("}}") else {
            out.push_str("{{");
            rest = tail;
            break;
        };
        let value = text(&resolve(root, path.trim()));
        if framed {
            out.push_str("<webhook-data>");
            out.push_str(
                &value
                    .replace('@', "@\u{2060}")
                    .replace('$', "$\u{2060}")
                    .replace('/', "/\u{2060}")
                    .replace("t3-context", "t3\u{2060}-context")
                    .replace("webhook-data", "webhook\u{2060}-data"),
            );
            out.push_str("</webhook-data>");
        } else {
            out.push_str(&value);
        }
        rest = after;
        if out.len() > 64 * 1024 {
            return String::new();
        }
    }
    out.push_str(rest);
    out
}
fn matches(f: &Value, root: &Value) -> bool {
    let v = resolve(root, f["path"].as_str().unwrap_or(""));
    let t = text(&v);
    match f["op"].as_str().unwrap_or("") {
        "exists" => !v.is_null(),
        "equals" => !v.is_null() && t == text(&f["value"]),
        "notEquals" => !v.is_null() && t != text(&f["value"]),
        "startsWith" => !v.is_null() && t.starts_with(&text(&f["value"])),
        "in" => f["value"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| text(v) == t)),
        _ => false,
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn accept(
    s: &State,
    id: &str,
    token: &str,
    headers: &hyper::HeaderMap,
    query: &str,
    body: &[u8],
) -> (u16, &'static str) {
    let mut db = s.db.lock().unwrap();
    let Ok(tx) = db.transaction() else {
        return (503, "error");
    };
    let Ok(task) = get(&tx, "scheduledTasks", id) else {
        return (404, "not_found");
    };
    let Ok(creds) = get(&tx, "hookCredentials", id) else {
        return (404, "not_found");
    };
    // HMAC verification compares fixed digests in constant time, including token length.
    let mut token_mac =
        Hmac::<Sha256>::new_from_slice(creds["token"].as_str().unwrap_or("").as_bytes()).unwrap();
    token_mac.update(b"hook-token");
    let mut candidate = Hmac::<Sha256>::new_from_slice(token.as_bytes()).unwrap();
    candidate.update(b"hook-token");
    if token_mac
        .verify_slice(&candidate.finalize().into_bytes())
        .is_err()
    {
        return (404, "not_found");
    }
    if task["enabled"] != true || task["schedule"]["type"] != "webhook" {
        return (409, "disabled");
    }
    let sig = &task["schedule"]["signature"];
    let Some(value) = headers
        .get(sig["header"].as_str().unwrap_or(""))
        .and_then(|v| v.to_str().ok())
    else {
        return (401, "rejected_signature");
    };
    let prefix = sig["prefix"].as_str().unwrap_or("");
    let value = value.trim();
    if !value
        .get(..prefix.len())
        .is_some_and(|v| v.eq_ignore_ascii_case(prefix))
    {
        return (401, "rejected_signature");
    }
    let value = &value[prefix.len()..];
    let bytes = if sig["encoding"] == "hex" {
        if value.len() != 64 {
            return (401, "rejected_signature");
        }
        (0..value.len())
            .step_by(2)
            .map(|i| {
                value
                    .get(i..i + 2)
                    .and_then(|v| u8::from_str_radix(v, 16).ok())
            })
            .collect::<Option<Vec<_>>>()
    } else {
        base64::engine::general_purpose::STANDARD.decode(value).ok()
    };
    let Some(bytes) = bytes else {
        return (401, "rejected_signature");
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(creds["secret"].as_str().unwrap_or("").as_bytes()).unwrap();
    mac.update(body);
    if mac.verify_slice(&bytes).is_err() {
        return (401, "rejected_signature");
    }
    let mut hs = json!({});
    for (k, v) in headers {
        if let Ok(v) = v.to_str() {
            hs[k.as_str()] = if k.as_str() == sig["header"].as_str().unwrap_or("")
                || ["authorization", "cookie", "x-api-key"].contains(&k.as_str())
            {
                json!("[redacted]")
            } else {
                json!(v)
            };
        }
    }
    let qs: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let root = json!({"body":body,"headers":hs,"query":qs});
    if task["schedule"]["filters"]
        .as_array()
        .is_some_and(|a| a.iter().any(|f| !matches(f, &root)))
    {
        return (202, "filtered");
    }
    let dedupe = &task["schedule"]["dedupe"];
    let replay = dedupe["replayKey"]
        .as_str()
        .map(|t| render(t, &root, false))
        .unwrap_or_else(|| hex(&Sha256::digest(root["body"].to_string().as_bytes())));
    if replay.len() > 4096 {
        return (400, "invalid_key");
    }
    let now = now_ms();
    if tx
        .execute("DELETE FROM hook_replay WHERE expires<=?", [now])
        .is_err()
    {
        return (503, "error");
    }
    let exists = tx
        .query_row(
            "SELECT 1 FROM hook_replay WHERE task=? AND key=?",
            params![id, replay],
            |_| Ok(()),
        )
        .optional();
    if matches!(exists, Ok(Some(()))) {
        return (202, "duplicate");
    }
    let count: i64 = tx
        .query_row("SELECT count(*) FROM hook_queue", [], |r| r.get(0))
        .unwrap_or(500);
    let replays: i64 = tx
        .query_row("SELECT count(*) FROM hook_replay", [], |r| r.get(0))
        .unwrap_or(100000);
    if count >= 500 || replays >= 100000 {
        return (429, "queue_full");
    }
    let coalesce = dedupe["coalesceKey"]
        .as_str()
        .map(|t| render(t, &root, false))
        .unwrap_or_default();
    let pending = if coalesce.is_empty() {
        None
    } else {
        tx.query_row(
            "SELECT id FROM hook_queue WHERE task=? AND coalesce=?",
            params![id, coalesce],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    };
    let job = pending.unwrap_or_else(|| random_id().unwrap_or_default());
    let delay = if coalesce.is_empty() {
        0
    } else {
        dedupe["coalesceWindowSeconds"].as_i64().unwrap_or(60) * 1000
    };
    let age = task["schedule"]["maxDeliveryAgeMinutes"]
        .as_i64()
        .unwrap_or(60)
        * 60000;
    let prompt = render(task["prompt"].as_str().unwrap_or(""), &root, true);
    if prompt.is_empty() || prompt.len() > 64 * 1024 {
        return (400, "invalid_prompt");
    }
    if tx.execute("INSERT INTO hook_queue VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prompt=excluded.prompt",params![job,id,coalesce,now+delay,now+age,prompt]).is_err() || tx.execute("INSERT INTO hook_replay VALUES(?,?,?)",params![id,replay,now+86400000]).is_err() || tx.commit().is_err(){return (503,"error");}
    s.streams.ops.wake_scheduler();
    (202, "accepted")
}
pub fn next_due(s: &State) -> Option<i64> {
    s.db.lock()
        .unwrap()
        .query_row("SELECT min(due) FROM hook_queue", [], |r| r.get(0))
        .ok()
        .flatten()
}
pub fn tick(s: &Arc<State>) {
    for _ in 0..8 {
        let job = {
            let db = s.db.lock().unwrap();
            let v=db.query_row("SELECT id,task,expires,prompt FROM hook_queue WHERE due<=? ORDER BY due LIMIT 1",[now_ms()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).optional().ok().flatten();
            if let Some((id, _, _, _)) = &v {
                if db
                    .execute("DELETE FROM hook_queue WHERE id=?", [id])
                    .is_err()
                {
                    return;
                }
            }
            v
        };
        let Some((_, task, expires, prompt)) = job else {
            return;
        };
        let enabled = get(&s.db.lock().unwrap(), "scheduledTasks", &task)
            .is_ok_and(|t| t["enabled"] == true && t["schedule"]["type"] == "webhook");
        if enabled && now_ms() <= expires {
            let _ = crate::scheduled::fire_prompt(s, &task, Some(&prompt));
        }
    }
}
