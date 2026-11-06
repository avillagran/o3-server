//! Signed inbound hooks on the ordinary listener. No payload/secret logging or network delivery.
use crate::{
    events::{get, put},
    store::{now_ms, random_id, State},
};
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
pub(crate) fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS hook_queue(id TEXT PRIMARY KEY,task TEXT NOT NULL,coalesce TEXT NOT NULL,due INTEGER NOT NULL,expires INTEGER NOT NULL,prompt TEXT NOT NULL); CREATE INDEX IF NOT EXISTS hook_queue_due ON hook_queue(due); CREATE TABLE IF NOT EXISTS hook_replay(task TEXT NOT NULL,key TEXT NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(task,key));").map_err(|e|e.to_string())?;
    let done: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='hook_coalesce_sha256_v1')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if done {
        return Ok(());
    }
    db.execute_batch("SAVEPOINT hook_coalesce_upgrade; CREATE TABLE hook_coalesce_sha256_v1(done INTEGER NOT NULL);").map_err(|e|e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut query = db
            .prepare("SELECT id,coalesce FROM hook_queue WHERE coalesce!=''")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(query);
        for (id, key) in rows {
            db.execute(
                "UPDATE hook_queue SET coalesce=?1 WHERE id=?2",
                params![hex(&Sha256::digest(key.as_bytes())), id],
            )
            .map_err(|e| e.to_string())?;
        }
        db.execute("INSERT INTO hook_coalesce_sha256_v1 VALUES(1)", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    let end = if result.is_ok() {
        "RELEASE hook_coalesce_upgrade"
    } else {
        "ROLLBACK TO hook_coalesce_upgrade; RELEASE hook_coalesce_upgrade"
    };
    db.execute_batch(end).map_err(|e| e.to_string())?;
    result
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
    // T3 `webhookPath`: `encodeURIComponent(taskId)`, since ids look like `scheduled-task:<id>`.
    task["webhook"] = json!({"path":format!("/api/hooks/{}/{token}", encode_component(id)),"url":null,"hasSecret":true});
    Ok(())
}
/// A task without the fields its runs write, for comparing definitions.
fn definition(task: &Value) -> Value {
    let mut t = task.clone();
    if let Some(m) = t.as_object_mut() {
        for key in [
            "lastRunAt",
            "lastRunStatus",
            "lastRunError",
            "runCount",
            "nextRunAt",
            "updatedAt",
        ] {
            m.remove(key);
        }
    }
    t
}

/// JavaScript `encodeURIComponent`.
pub fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
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
    query: &str,
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
    // Same decoders as T3's effect/encoding; a wrong-length tag fails the MAC check below.
    let bytes = if sig["encoding"] == "hex" {
        crate::auth::decode_hex(value)
    } else {
        crate::auth::decode_b64(value)
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
    // A definition edit or credential rotation invalidates the verified snapshot. Run state
    // (a concurrent run marking itself running or finished) is not a definition change.
    if get(&tx, "scheduledTasks", id).ok().map(|t| definition(&t)) != Some(definition(&task))
        || get(&tx, "hookCredentials", id).ok().as_ref() != Some(&creds)
    {
        return (409, "definition_changed");
    }
    let replay = hex(&Sha256::digest(body));
    let payload = crate::webhook_payload::Request {
        method: "POST".into(),
        path: format!("/api/hooks/{}/{token}", encode_component(id)),
        query: query.into(),
        headers: headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body_text: String::from_utf8_lossy(body).into_owned(),
    };
    if task["schedule"]["filters"]
        .as_array()
        .is_some_and(|filters| payload.failed_filter(filters).is_some())
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
    let coalesce = payload.dedupe_key(dedupe["coalesceKey"].as_str());
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
    let job = match pending {
        Some(job) => job,
        None => match random_id() {
            Ok(job) => job,
            Err(_) => return (503, "error"),
        },
    };
    let delay = if coalesce.is_empty() {
        0
    } else {
        dedupe["coalesceWindowSeconds"].as_i64().unwrap_or(120) * 1000
    };
    let age = task["schedule"]["maxDeliveryAgeMinutes"]
        .as_i64()
        .unwrap_or(60)
        * 60000;
    let Ok(frame) = random_id() else {
        return (503, "error");
    };
    let frame = frame.replace('-', "");
    let prompt = payload
        .render(task["prompt"].as_str().unwrap_or(""), &frame[..12], false)
        .prompt;
    if prompt.is_empty() || prompt.len() > 64 * 1024 {
        return (400, "invalid_prompt");
    }
    if tx.execute("INSERT INTO hook_queue VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prompt=excluded.prompt",params![job,id,coalesce,now+delay,now+age,prompt]).is_err() || tx.execute("INSERT INTO hook_replay VALUES(?,?,?)",params![id,replay,i64::MAX]).is_err() || tx.commit().is_err(){return (503,"error");}
    s.streams.ops.wake_scheduler();
    (202, "accepted")
}
pub fn next_due(s: &State) -> Option<i64> {
    let db = s.db.lock().unwrap();
    let active = s.streams.ops.hook_runs.lock().unwrap();
    let mut q = db
        .prepare("SELECT id,CASE WHEN due=9223372036854775807 THEN 0 ELSE due END FROM hook_queue")
        .ok()?;
    let rows = q
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .ok()?;
    rows.filter_map(Result::ok)
        .filter(|(id, _)| !active.contains(id))
        .map(|(_, due)| due)
        .min()
}

/// A persisted claim without an in-process owner is recoverable; a live owner is exclusive.
struct ActiveHook<'a> {
    state: &'a State,
    id: String,
}
impl Drop for ActiveHook<'_> {
    fn drop(&mut self) {
        self.state
            .streams
            .ops
            .hook_runs
            .lock()
            .unwrap()
            .remove(&self.id);
        self.state.streams.ops.scheduler_wake.notify_one();
    }
}
type QueuedHook = (String, String, i64, String);
fn claim_next(s: &State) -> Option<QueuedHook> {
    let db = s.db.lock().unwrap();
    let mut active = s.streams.ops.hook_runs.lock().unwrap();
    let mut q=db.prepare("SELECT id,task,expires FROM hook_queue WHERE due<=?1 OR due=9223372036854775807 ORDER BY due,id").ok()?;
    let rows = q
        .query_map([now_ms()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .ok()?;
    let job = rows
        .filter_map(Result::ok)
        .find(|job| !active.contains(&job.0))?;
    // Read the potentially large prompt only for the selected job, not other workers' claims.
    let prompt = db
        .query_row(
            "SELECT prompt FROM hook_queue WHERE id=?1",
            [&job.0],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    db.execute(
        "UPDATE hook_queue SET due=9223372036854775807 WHERE id=?1",
        [&job.0],
    )
    .ok()?;
    active.insert(job.0.clone());
    Some((job.0, job.1, job.2, prompt))
}
pub fn tick(s: &Arc<State>) {
    for _ in 0..8 {
        let Some((id, task, expires, prompt)) = claim_next(s) else {
            return;
        };
        let _claim = ActiveHook {
            state: s,
            id: id.clone(),
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
mod definition_tests {
    use super::*;
    #[test]
    fn run_state_is_not_a_definition_change() {
        let task = json!({"id":"t","enabled":true,"prompt":"p","schedule":{"type":"webhook"},"runCount":0,"lastRunStatus":"never","lastRunAt":null,"lastRunError":null,"nextRunAt":null,"updatedAt":"a"});
        let mut running = task.clone();
        running["lastRunStatus"] = json!("running");
        running["runCount"] = json!(1);
        running["updatedAt"] = json!("b");
        assert_eq!(definition(&task), definition(&running));
        let mut paused = task.clone();
        paused["enabled"] = json!(false);
        assert_ne!(definition(&task), definition(&paused));
        assert_eq!(
            encode_component("scheduled-task:a b/ñ"),
            "scheduled-task%3Aa%20b%2F%C3%B1"
        );
    }
    #[test]
    fn old_coalescing_keys_upgrade_once_without_changing_accepted_work() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE hook_queue(id TEXT PRIMARY KEY,task TEXT NOT NULL,coalesce TEXT NOT NULL,due INTEGER NOT NULL,expires INTEGER NOT NULL,prompt TEXT NOT NULL); INSERT INTO hook_queue VALUES('job','task','existing-group',123,456,'Already rendered prompt');").unwrap();
        initialize(&db).unwrap();
        let row: (String, i64, i64, String) = db
            .query_row(
                "SELECT coalesce,due,expires,prompt FROM hook_queue WHERE id='job'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                hex(&Sha256::digest(b"existing-group")),
                123,
                456,
                "Already rendered prompt".into()
            )
        );
        initialize(&db).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT coalesce FROM hook_queue WHERE id='job'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            row.0
        );
    }
    #[test]
    fn failed_key_upgrade_retains_original_rows_and_does_not_mark_success() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE hook_queue(id TEXT PRIMARY KEY,task TEXT NOT NULL,coalesce TEXT NOT NULL,due INTEGER NOT NULL,expires INTEGER NOT NULL,prompt TEXT NOT NULL); INSERT INTO hook_queue VALUES('job','task','existing-group',123,456,'Already rendered prompt'); CREATE TRIGGER refuse_key BEFORE UPDATE ON hook_queue BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
        assert!(initialize(&db)
            .unwrap_err()
            .contains("fixture write failure"));
        assert_eq!(
            db.query_row("SELECT coalesce FROM hook_queue", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "existing-group"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='hook_coalesce_sha256_v1'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
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
    fn live_claim_is_exclusive_and_an_abandoned_claim_is_immediately_recoverable() {
        let dir = std::env::temp_dir().join(format!("o3-hook-owner-{}", random_id().unwrap()));
        let state = State::open(&dir).unwrap();
        {
            let db = state.db.lock().unwrap();
            initialize(&db).unwrap();
            db.execute(
                "INSERT INTO hook_queue VALUES('job','task','',0,?1,'synthetic prompt')",
                [now_ms() + 60000],
            )
            .unwrap();
        }
        let first = claim_next(&state).unwrap();
        let owner = ActiveHook {
            state: &state,
            id: first.0.clone(),
        };
        assert_eq!(first.0, "job");
        assert!(
            claim_next(&state).is_none(),
            "a second worker must not claim live work"
        );
        assert!(
            next_due(&state).is_none(),
            "a live owner is not a recovery candidate"
        );
        drop(owner);
        assert_eq!(next_due(&state), Some(0));
        let recovered = claim_next(&state).unwrap();
        let _owner = ActiveHook {
            state: &state,
            id: recovered.0.clone(),
        };
        assert_eq!(recovered, first);
        assert!(claim_next(&state).is_none());
    }
    #[test]
    fn durable_claim_and_receipt_survive_interrupted_dispatch_once() {
        use t3_protocol::api::{
            orchestration,
            scheduled_tasks::{self, TaskUpsert},
        };
        let s = crate::checkpoint::recovery_test_support::start(|s| {
            // This fixture advances each crash boundary manually. The automatic timer
            // would be another live owner when next_due checks abandoned-claim recovery.
            s.streams
                .ops
                .scheduler
                .store(true, std::sync::atomic::Ordering::Release);
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
            let due = next_due(&s.state);
            assert!(
                due.is_some_and(|due| due <= now_ms()),
                "interrupted claim at {stage} must be immediately recoverable, got {due:?}"
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
