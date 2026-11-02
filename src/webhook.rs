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

fn signed_template(template: &str) -> bool {
    let mut rest = template;
    while let Some((_, tail)) = rest.split_once("{{") {
        let Some((path, after)) = tail.split_once("}}") else {
            return false;
        };
        if path.trim() != "body" && !path.trim().starts_with("body.") {
            return false;
        }
        rest = after;
    }
    true
}
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
                ) || f["path"]
                    .as_str()
                    .is_none_or(|p| p.len() > 1024 || !matches!(p.split('.').next(), Some("body")))
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
    for key in ["replayKey", "coalesceKey"] {
        if s["dedupe"][key]
            .as_str()
            .is_some_and(|t| !signed_template(t))
        {
            return Err("webhook templates require signed body fields");
        }
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
    validate(&task["schedule"])?;
    if !signed_template(task["prompt"].as_str().unwrap_or("")) {
        return Err("webhook prompt requires signed body fields".into());
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
/// Whether the path names an enabled-or-not webhook task and carries its token. Checked before
/// any body is read, so only holders of the hook URL cost more than one keyed lookup.
pub fn admits(s: &State, id: &str, token: &str) -> bool {
    let reader = s.reader.lock().unwrap();
    let webhook =
        get(&reader, "scheduledTasks", id).is_ok_and(|t| t["schedule"]["type"] == "webhook");
    let Ok(creds) = get(&reader, "hookCredentials", id) else {
        return false;
    };
    drop(reader);
    let mut expected =
        Hmac::<Sha256>::new_from_slice(creds["token"].as_str().unwrap_or("").as_bytes()).unwrap();
    expected.update(b"hook-token");
    let mut candidate = Hmac::<Sha256>::new_from_slice(token.as_bytes()).unwrap();
    candidate.update(b"hook-token");
    webhook
        && expected
            .verify_slice(&candidate.finalize().into_bytes())
            .is_ok()
}
pub fn accept(
    s: &State,
    id: &str,
    token: &str,
    headers: &hyper::HeaderMap,
    _query: &str,
    body: &[u8],
) -> (u16, &'static str) {
    let reader = s.reader.lock().unwrap();
    let Ok(task) = get(&reader, "scheduledTasks", id) else {
        return (404, "not_found");
    };
    let Ok(creds) = get(&reader, "hookCredentials", id) else {
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
    // The wire contract authenticates only raw body bytes. Never expose unsigned
    // headers (including the signature) or query values to execution decisions.
    if validate(&task["schedule"]).is_err()
        || !signed_template(task["prompt"].as_str().unwrap_or(""))
    {
        return (400, "unsigned_template");
    }
    drop(reader);
    let mut db = s.db.lock().unwrap();
    let Ok(tx) = db.transaction() else {
        return (503, "error");
    };
    // A definition edit or credential rotation invalidates the verified snapshot.
    if get(&tx, "scheduledTasks", id).ok().as_ref() != Some(&task)
        || get(&tx, "hookCredentials", id).ok().as_ref() != Some(&creds)
    {
        return (409, "definition_changed");
    }
    let replay = hex(&Sha256::digest(body));
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let root = json!({"body":body});
    if task["schedule"]["filters"]
        .as_array()
        .is_some_and(|a| a.iter().any(|f| !matches(f, &root)))
    {
        return (202, "filtered");
    }
    let dedupe = &task["schedule"]["dedupe"];
    let now = now_ms();
    // No signed timestamp exists in this contract. Body digests never expire;
    // maxDeliveryAgeMinutes only bounds time spent waiting in our queue.
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
    let queue_bytes: i64 = tx.query_row("SELECT coalesce(sum(length(CAST(prompt AS BLOB))+length(CAST(coalesce AS BLOB))),0) FROM hook_queue", [], |r| r.get(0)).unwrap_or(i64::MAX);
    let replay_bytes: i64 = tx
        .query_row(
            "SELECT coalesce(sum(length(CAST(key AS BLOB))),0) FROM hook_replay",
            [],
            |r| r.get(0),
        )
        .unwrap_or(i64::MAX);
    if count >= 500
        || replays >= 100000
        || queue_bytes >= 8 * 1024 * 1024 - 70 * 1024
        || replay_bytes >= 8 * 1024 * 1024 - 64
    {
        return (429, "queue_full");
    }
    let coalesce = dedupe["coalesceKey"]
        .as_str()
        .map(|t| render(t, &root, false))
        .unwrap_or_default();
    if coalesce.len() > 4096 {
        return (400, "invalid_key");
    }
    let pending = if coalesce.is_empty() {
        None
    } else {
        tx.query_row(
            "SELECT id FROM hook_queue WHERE task=? AND coalesce=? AND due != 9223372036854775807",
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
    if tx.execute("INSERT INTO hook_queue VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prompt=excluded.prompt",params![job,id,coalesce,now+delay,now+age,prompt]).is_err() || tx.execute("INSERT INTO hook_replay VALUES(?,?,?)",params![id,replay,i64::MAX]).is_err() || tx.commit().is_err(){return (503,"error");}
    s.streams.ops.wake_scheduler();
    (202, "accepted")
}
pub fn next_due(s: &State) -> Option<i64> {
    s.db.lock()
        .unwrap()
        .query_row(
            "SELECT min(CASE WHEN due=9223372036854775807 THEN 0 ELSE due END) FROM hook_queue",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten()
}
pub fn tick(s: &Arc<State>) {
    for _ in 0..8 {
        let job = {
            let db = s.db.lock().unwrap();
            let v=db.query_row("SELECT id,task,expires,prompt FROM hook_queue WHERE due<=? OR due=9223372036854775807 ORDER BY due LIMIT 1",[now_ms()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).optional().ok().flatten();
            if let Some((id, _, _, _)) = &v {
                // Freeze the prompt while dispatching. A new coalesced delivery gets
                // its own job; a crash leaves this claim eligible for receipt replay.
                if db
                    .execute(
                        "UPDATE hook_queue SET due=9223372036854775807 WHERE id=?",
                        [id],
                    )
                    .is_err()
                {
                    return;
                }
            }
            v
        };
        let Some((id, task, expires, prompt)) = job else {
            return;
        };
        #[cfg(test)]
        if crash_tests::stop(s, "claim") {
            return;
        }
        let enabled = get(&s.db.lock().unwrap(), "scheduledTasks", &task)
            .is_ok_and(|t| t["enabled"] == true && t["schedule"]["type"] == "webhook");
        let cid = format!("webhook:{task}:{id}");
        let dispatched =
            s.db.lock()
                .unwrap()
                .query_row("SELECT 1 FROM receipts WHERE command_id=?", [&cid], |_| {
                    Ok(())
                })
                .optional()
                .ok()
                .flatten()
                .is_some();
        if !dispatched
            && enabled
            && now_ms() <= expires
            && crate::scheduled::fire_prompt_key(s, &task, Some(&prompt), Some(&id)).is_err()
        {
            let _ = s.db.lock().unwrap().execute(
                "UPDATE hook_queue SET due=? WHERE id=?",
                params![now_ms() + 30000, id],
            );
            return;
        }
        #[cfg(test)]
        if crash_tests::stop(s, "dispatch") {
            return;
        }
        // A durable command receipt deduplicates retries after a crash at handoff.
        let _ =
            s.db.lock()
                .unwrap()
                .execute("DELETE FROM hook_queue WHERE id=?", [&id]);
    }
}

#[cfg(test)]
mod crash_tests {
    use super::*;
    static FAULT: std::sync::Mutex<Option<(std::path::PathBuf, &'static str)>> =
        std::sync::Mutex::new(None);
    pub(super) fn stop(s: &State, stage: &str) -> bool {
        let fault = FAULT.lock().unwrap();
        fault
            .as_ref()
            .is_some_and(|(p, at)| p == &s.dir && *at == stage)
    }
    #[test]
    fn durable_claim_and_receipt_survive_interrupted_dispatch_once() {
        use t3_protocol::api::{
            orchestration,
            scheduled_tasks::{self, TaskUpsert},
        };
        let s = crate::checkpoint::recovery_test_support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(crate::provider::ScriptedProvider));
        });
        let c = s.connect();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Hook","modelSelection":{"instanceId":"scripted","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let input = json!({"id":"crash-hook","title":"Hook","threadId":"t","projectId":"p","prompt":"inspect {{body.action}}","enabled":true,"schedule":{"type":"webhook","signature":{"header":"x-signature","encoding":"hex","prefix":"","secret":"fixture-key"},"dedupe":{"coalesceKey":"{{body.action}}","coalesceWindowSeconds":60}},"workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default"});
        let task = scheduled_tasks::upsert(&c, &TaskUpsert::from_json(&input).unwrap()).unwrap();
        let path = task["task"]["webhook"]["path"].as_str().unwrap();
        let token = path.rsplit('/').next().unwrap();
        let body = br#"{"action":"go"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"fixture-key").unwrap();
        mac.update(body);
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "x-signature",
            hex(&mac.finalize().into_bytes()).parse().unwrap(),
        );
        assert_eq!(
            accept(&s.state, "crash-hook", token, &headers, "", body).1,
            "accepted"
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        for stage in ["claim", "dispatch"] {
            *FAULT.lock().unwrap() = Some((s.dir.clone(), stage));
            if stage == "claim" {
                s.state
                    .db
                    .lock()
                    .unwrap()
                    .execute("UPDATE hook_queue SET due=0", [])
                    .unwrap();
            }
            runtime.block_on(async {
                tick(&s.state);
            });
            assert!(
                next_due(&s.state).is_some_and(|due| due <= now_ms()),
                "interrupted claims must be scheduled immediately on recovery"
            );
            assert_eq!(
                s.state
                    .db
                    .lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM hook_queue", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                1,
                "interrupted {stage} lost queue item"
            );
        }
        *FAULT.lock().unwrap() = None;
        runtime.block_on(async {
            tick(&s.state);
        });
        assert_eq!(
            s.state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM hook_queue", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            scheduled_tasks::list(&c).unwrap()["tasks"][0]["runCount"],
            1
        );
        let projection = orchestration::thread_projection(&c, "t").unwrap();
        assert_eq!(
            projection["runs"].as_array().unwrap().len(),
            1,
            "{projection}"
        );
    }
}
