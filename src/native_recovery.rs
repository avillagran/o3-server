//! Fresh native conversations retain app row identity and reconstruct earlier context.
//! Adapted from nightly4165 ProviderTurnStartService (MIT).
use crate::{
    context_transfer::{self, fork_items, list},
    events::{append, get, put},
    handoff_history as history,
    provider::{NativeRecoveryReason, ProviderEvent, RunInput},
    store::{iso, now_ms, random_id, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
type Result<T> = std::result::Result<T, String>;
type BindingReceipt = (String, Option<String>, Option<String>, Option<String>);
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS native_recoveries(id TEXT PRIMARY KEY,run_id TEXT NOT NULL,attempt_id TEXT NOT NULL,thread_id TEXT NOT NULL,provider_thread_id TEXT NOT NULL,old_native TEXT,provider_instance_id TEXT NOT NULL,pool_key TEXT NOT NULL,released_at TEXT,reason TEXT NOT NULL,handoff_id TEXT NOT NULL,new_native TEXT,created_at TEXT NOT NULL,UNIQUE(run_id,attempt_id)); CREATE INDEX IF NOT EXISTS native_recoveries_pending_cleanup ON native_recoveries(thread_id) WHERE released_at IS NULL; CREATE INDEX IF NOT EXISTS native_recoveries_pending_binding ON native_recoveries(provider_thread_id) WHERE new_native IS NULL;").map_err(|e|e.to_string())
}
pub(crate) fn handoff_id(db: &Connection, run: &Value) -> Result<Option<String>> {
    db.query_row(
        "SELECT handoff_id FROM native_recoveries WHERE run_id=? AND attempt_id=?",
        params![run["id"].as_str(), run["activeAttemptId"].as_str()],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}
fn reply(db: &Connection, thread: &Value, run: &Value, mut budget: Value) -> Result<Value> {
    budget["nativeThreadId"] = Value::Null;
    budget["contextUsage"] = Value::Null;
    budget["nativeContextEstimate"] = json!(0);
    budget["uncertainDelivery"] = json!(false);
    let context = context_transfer::input(db, thread, run)?;
    Ok(
        json!({"handoffs":context["handoffs"],"deliveredItemIds":context["deliveredItemIds"],"budget":budget}),
    )
}
fn background_work(db: &Connection, tid: &str, pt: &Value) -> Result<bool> {
    if pt["pendingBackgroundTasks"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        return Ok(true);
    }
    let runs = list(db, "runs", tid)?;
    for kind in ["subagents", "backgroundTasks"] {
        if list(db, kind, tid)?.iter().any(|item| {
            item["origin"] != "app_owned"
                && matches!(
                    item["status"].as_str(),
                    Some("pending" | "running" | "starting" | "waiting" | "queued")
                )
                && (item["providerThreadId"] == pt["id"]
                    || (item["providerThreadId"].is_null()
                        && runs.iter().any(|r| {
                            r["id"] == item["runId"] && r["providerThreadId"] == pt["id"]
                        })))
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(crate) fn prepare(
    s: &State,
    tid: &str,
    rid: &str,
    expected: Option<&str>,
    previous: Option<String>,
    reason: NativeRecoveryReason,
    current_text: String,
) -> Result<Value> {
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if !crate::run_restart::matches(&tx, rid, expected) {
        return Err("Stale native recovery attempt".into());
    }
    let mut thread = get(&tx, "thread", tid)?;
    let mut run = get(&tx, "runs", rid)?;
    if !thread["deletedAt"].is_null()
        || !thread["archivedAt"].is_null()
        || !matches!(
            run["status"].as_str(),
            Some("preparing" | "starting" | "running")
        )
    {
        return Err("Native recovery run is no longer admissible".into());
    }
    let aid_owned = run["activeAttemptId"]
        .as_str()
        .ok_or("Recovery attempt missing")?
        .to_owned();
    let aid = aid_owned.as_str();
    if list(&tx, "providerTurns", tid)?
        .iter()
        .any(|t| t["runAttemptId"] == aid)
    {
        return Err("Native input was already accepted; recovery cannot replay this turn".into());
    }
    let ptid_owned = run["providerThreadId"]
        .as_str()
        .ok_or("Recovery provider thread missing")?
        .to_owned();
    let ptid = ptid_owned.as_str();
    let mut pt = get(&tx, "providerThreads", ptid)?;
    let existing: Option<(Option<String>, String)> = tx
        .query_row(
            "SELECT old_native,id FROM native_recoveries WHERE run_id=? AND attempt_id=?",
            params![rid, aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let budget = crate::handoff_budget::context(&tx, &thread, &run)?;
    if let Some((old, id)) = existing {
        if old == previous && pt["nativeThreadRef"].is_null() {
            let mut data = reply(&tx, &thread, &run, budget)?;
            data["id"] = json!(id);
            return Ok(data);
        }
        return Err("Native recovery was already admitted".into());
    }
    if pt["nativeThreadRef"]["nativeId"].as_str() != previous.as_deref() {
        return Err("Native recovery binding changed".into());
    }
    let has_background =
        matches!(reason, NativeRecoveryReason::HandoffBudget) && background_work(&tx, tid, &pt)?;
    let at = iso(now_ms());
    let id = if matches!(reason, NativeRecoveryReason::ForkFailed) {
        if !thread["nativeForkTransferId"].is_string() {
            return Err("Native fork fallback has no pending transfer".into());
        }
        context_transfer::portable_fork_fallback(s, &tx, &mut thread, &mut run, &at)?
    } else {
        let items = fork_items(&tx, tid, run["ordinal"].as_i64().unwrap_or(1) - 1)?;

        let coverage = history::coverage(
            tid,
            1,
            (run["ordinal"].as_i64().unwrap_or(1) - 1).max(1),
            &items,
        );
        let messages = items
            .iter()
            .filter_map(history::message)
            .collect::<Vec<_>>();
        let selected = history::select(&messages, &coverage, 0, history::cap());
        let id = random_id()?;
        let transfer_id = random_id()?;
        let at = iso(now_ms());
        let h = json!({"id":id,"transferId":transfer_id,"threadId":tid,"targetRunId":rid,"fromProviderThreadIds":[ptid],"toProviderThreadId":ptid,"coveredRunOrdinals":{"from":1,"to":(run["ordinal"].as_i64().unwrap_or(1)-1).max(1)},"strategy":"full_thread_summary","status":"ready","summaryMessageId":null,"summaryText":history::render(selected["messages"].as_array().unwrap(),selected["context"].as_str().unwrap()),"history":{"messages":selected["messages"],"coverage":coverage,"omittedItems":selected["omittedItems"],"omittedItemIds":selected["omittedItemIds"]},"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at});

        put(&tx, "contextHandoffs", &id, tid, &h)?;
        append(&tx, tid, "context-handoff.updated", h, &thread)?;
        let transfer = json!({"id":transfer_id,"type":"provider_handoff","sourceThreadId":tid,"targetThreadId":tid,"sourcePoint":{"threadId":tid},"basePoint":null,"sourceProviderInstanceId":pt["providerInstanceId"],"targetProviderInstanceId":run["providerInstanceId"],"targetRunId":rid,"status":"resolved_portable","resolution":{"strategy":"portable_context","contextHandoffId":id},"createdBy":"system","error":null,"createdAt":at,"updatedAt":at,"consumedAt":null});
        put(&tx, "contextTransfers", &transfer_id, tid, &transfer)?;
        append(&tx, tid, "context-transfer.updated", transfer, &thread)?;
        id
    };
    pt["nativeThreadRef"] = Value::Null;
    pt["nativeConversationHeadRef"] = Value::Null;
    pt["contextUsage"] = Value::Null;
    pt["goal"] = Value::Null;
    pt["nativeMetadata"] = Value::Null;
    pt["forkedFrom"] = Value::Null;
    pt["pendingBackgroundTasks"] = json!([]);
    pt["status"] = json!("not_loaded");
    pt["updatedAt"] = json!(at);
    if pt["handoffIds"].is_null() {
        pt["handoffIds"] = json!([]);
    }
    pt["handoffIds"]
        .as_array_mut()
        .ok_or("provider handoff ids missing")?
        .push(json!(id));
    put(&tx, "providerThreads", ptid, tid, &pt)?;
    append(&tx, tid, "provider-thread.updated", pt, &thread)?;
    thread["codexNativeThreadId"] = Value::Null;
    thread["nativeForkSource"] = Value::Null;
    thread["nativeForkLastTurnId"] = Value::Null;
    thread["nativeForkTransferId"] = Value::Null;
    thread["providerHandoff"] = Value::Null;
    thread["providerHandoffAttachments"] = json!([]);
    put(&tx, "thread", tid, tid, &thread)?;
    append(&tx, tid, "thread.metadata-updated", thread.clone(), &thread)?;
    let recovery_id = random_id()?;
    let pool_key = format!(
        "{}:{}",
        tid,
        thread["providerGeneration"].as_str().unwrap_or("")
    );
    tx.execute("INSERT INTO native_recoveries(id,run_id,attempt_id,thread_id,provider_thread_id,old_native,provider_instance_id,pool_key,reason,handoff_id,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![recovery_id,rid,aid,tid,ptid,previous,run["providerInstanceId"].as_str(),pool_key,reason.label(),id,at]).map_err(|e|e.to_string())?;
    let mut result = reply(&tx, &thread, &run, budget)?;
    result["id"] = json!(recovery_id);
    if matches!(reason, NativeRecoveryReason::HandoffBudget) {
        let m = get(&tx, "messages", run["userMessageId"].as_str().unwrap_or(""))?;
        let mut selection = run["modelSelection"].clone();
        selection["o3TransferHandoffs"] = result["handoffs"].clone();
        selection["o3DeliveredHistoryItemIds"] = json!([]);
        selection["o3HandoffBudget"] = result["budget"].clone();
        let preview = RunInput {
            native_mcp: None,
            provider_instance_id: run["providerInstanceId"].as_str().unwrap_or("").into(),
            native_session_id: None,
            thread_id: tid.into(),
            run_id: rid.into(),
            cwd: s.dir.clone(),
            text: current_text.clone(),
            model_selection: selection,
            runtime_mode: thread["runtimeMode"].as_str().unwrap_or("").into(),
            interaction_mode: thread["interactionMode"].as_str().unwrap_or("").into(),
            attachment_root: s.dir.clone(),
            attachments: m["attachments"].clone(),
            context: None,
        };
        let mut blocks = vec![json!({"type":"text","text":current_text,"text_elements":[]})];
        crate::handoff_delivery::manual(&preview, &mut blocks)?;
        crate::handoff_delivery::prepare(&preview, &blocks)?;
        if has_background {
            return Err("Context handoff cannot replace a native thread while its background work is active".into());
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(result)
}
pub(crate) fn bound(db: &Connection, run: &Value, native: &str) -> Result<()> {
    let entry:Option<BindingReceipt>=db.query_row("SELECT id,old_native,new_native,released_at FROM native_recoveries WHERE (run_id=?1 AND attempt_id=?2) OR (provider_thread_id=?3 AND new_native IS NULL) ORDER BY CASE WHEN run_id=?1 AND attempt_id=?2 THEN 0 ELSE 1 END,rowid DESC LIMIT 1",params![run["id"].as_str(),run["activeAttemptId"].as_str(),run["providerThreadId"].as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
    if let Some((id, old, bound, released)) = entry {
        if released.is_none() {
            return Err("Native recovery cleanup has not been acknowledged".into());
        }
        if old.as_deref() == Some(native) {
            return Err("Replacement reused an uncertain native identity".into());
        }
        if bound.as_deref().is_some_and(|id| id != native) {
            return Err("Replacement native identity changed during admission".into());
        }
        db.execute(
            "UPDATE native_recoveries SET new_native=? WHERE id=?",
            params![native, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) async fn request(
    input: &mut RunInput,
    output: &mpsc::Sender<ProviderEvent>,
    reason: NativeRecoveryReason,
    current: &[Value],
) -> Result<String> {
    if input.model_selection["o3ForceNewNativeThread"] == true {
        return Err(
            "Fresh native recovery failed; refusing another replacement in this attempt".into(),
        );
    }
    let (reply, ready) = oneshot::channel();
    output
        .send(ProviderEvent::RecoverNative {
            previous_native: input.native_session_id.clone(),
            reason,
            current_text: current
                .first()
                .and_then(|v| v["text"].as_str())
                .unwrap_or(&input.text)
                .to_owned(),
            reply,
        })
        .await
        .map_err(|_| "Recovery supervisor closed")?;
    let data = ready.await.map_err(|_| "Recovery acknowledgement lost")??;
    input.native_session_id = None;
    for key in [
        "nativeForkSource",
        "nativeForkLastTurnId",
        "o3HistoricalContext",
        "o3ColdCompact",
        "o3NewSessionId",
    ] {
        input
            .model_selection
            .as_object_mut()
            .ok_or("model selection missing")?
            .remove(key);
    }
    input.model_selection["o3TransferHandoffs"] = data["handoffs"].clone();
    input.model_selection["o3DeliveredHistoryItemIds"] = data["deliveredItemIds"].clone();
    input.model_selection["o3HandoffBudget"] = data["budget"].clone();
    input.model_selection["o3ForceNewNativeThread"] = json!(true);
    Ok(data["id"]
        .as_str()
        .ok_or("Recovery receipt missing")?
        .to_owned())
}
pub(crate) fn uncertain(input: &RunInput) -> bool {
    input.native_session_id.as_deref().is_some_and(|native| {
        (input.model_selection["o3HandoffBudget"]["nativeThreadId"] == native
            && input.model_selection["o3HandoffBudget"]["uncertainDelivery"] == true)
            || input.model_selection["o3TransferHandoffs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|h| {
                    h["delivery"]["nativeThreadId"] == native
                        && h["delivery"]["status"] == "pending"
                })
    })
}
pub(crate) fn thread_in_use(message: &str) -> bool {
    if serde_json::from_str::<Value>(message).is_ok_and(|e| e["code"] == -32600)
        && message.contains("already has an active writer")
    {
        return true;
    }
    let lower = message.to_lowercase();
    [
        "thread is in use",
        "thread in use",
        "already in use",
        "session is locked",
        "another client",
        "already running",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

pub(crate) fn release(
    s: &State,
    tid: &str,
    rid: &str,
    expected: Option<&str>,
    id: &str,
) -> Result<()> {
    let db = s.db.lock().unwrap();
    if !crate::run_restart::matches(&db, rid, expected) {
        return Err("Stale recovery release".into());
    }
    db.execute("UPDATE native_recoveries SET released_at=coalesce(released_at,?) WHERE id=? AND thread_id=?",params![iso(now_ms()),id,tid]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(crate) async fn released(output: &mpsc::Sender<ProviderEvent>, id: String) -> Result<()> {
    let (reply, ready) = oneshot::channel();
    output
        .send(ProviderEvent::NativeRecoveryReleased { id, reply })
        .await
        .map_err(|_| "Recovery supervisor closed")?;
    ready
        .await
        .map_err(|_| "Recovery release acknowledgement lost")?
}
pub(crate) async fn release_pending(
    s: &State,
    provider: &dyn crate::provider::Provider,
    tid: &str,
    rid: &str,
    expected: Option<&str>,
) -> Result<()> {
    let jobs = {
        let db = s.db.lock().unwrap();
        let mut q=db.prepare("SELECT id,provider_instance_id,pool_key,old_native FROM native_recoveries WHERE thread_id=? AND released_at IS NULL ORDER BY rowid").map_err(|e|e.to_string())?;
        let jobs = q
            .query_map([tid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        jobs
    };
    for (id, instance, key, native) in jobs {
        provider
            .detach(&instance, &key, native.as_deref(), true)
            .await?;
        release(s, tid, rid, expected, &id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_imported_handoff_lists_normalize_idempotently_without_hiding_corruption() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        for (id, v) in [
            ("missing", json!({"id":"missing"})),
            ("null", json!({"id":"null","handoffIds":null})),
            ("valid", json!({"id":"valid","handoffIds":["retained"]})),
            ("corrupt", json!({"id":"corrupt","handoffIds":"invalid"})),
        ] {
            put(&db, "providerThreads", id, "t", &v).unwrap();
        }
        for _ in 0..2 {
            context_transfer::initialize(&db).unwrap();
            for id in ["missing", "null"] {
                assert_eq!(
                    get(&db, "providerThreads", id).unwrap()["handoffIds"],
                    json!([])
                );
            }
            assert_eq!(
                get(&db, "providerThreads", "valid").unwrap()["handoffIds"],
                json!(["retained"])
            );
            assert_eq!(
                get(&db, "providerThreads", "corrupt").unwrap()["handoffIds"],
                "invalid"
            );
        }
    }
    #[test]
    fn recovery_journal_is_attempt_fenced_idempotent_and_gates_new_bindings_on_cleanup() {
        let dir = std::env::temp_dir().join(format!("native-recovery-{}", random_id().unwrap()));
        let s = State::open(&dir).unwrap();
        {
            let db = s.db.lock().unwrap();
            for (kind, id, v) in [
                (
                    "thread",
                    "t",
                    json!({"id":"t","providerInstanceId":"codex","activeProviderThreadId":"pt","codexNativeThreadId":"old","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"}),
                ),
                (
                    "runs",
                    "r",
                    json!({"id":"r","threadId":"t","ordinal":1,"status":"preparing","providerThreadId":"pt","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"activeAttemptId":"a"}),
                ),
                (
                    "providerThreads",
                    "pt",
                    json!({"id":"pt","driver":"codex","providerInstanceId":"codex","nativeThreadRef":{"nativeId":"old","strength":"strong","driver":"codex"}}),
                ),
            ] {
                put(&db, kind, id, "t", &v).unwrap();
            }
        }
        assert!(prepare(
            &s,
            "t",
            "r",
            Some("stale"),
            Some("old".into()),
            NativeRecoveryReason::UncertainHistory,
            "CURRENT".into()
        )
        .unwrap_err()
        .contains("Stale"));
        let first = prepare(
            &s,
            "t",
            "r",
            Some("a"),
            Some("old".into()),
            NativeRecoveryReason::UncertainHistory,
            "CURRENT".into(),
        )
        .unwrap();
        let second = prepare(
            &s,
            "t",
            "r",
            Some("a"),
            Some("old".into()),
            NativeRecoveryReason::UncertainHistory,
            "CURRENT".into(),
        )
        .unwrap();
        assert_eq!(first["id"], second["id"]);
        {
            let db = s.db.lock().unwrap();
            let r = get(&db, "runs", "r").unwrap();
            assert!(bound(&db, &r, "new").unwrap_err().contains("cleanup"));
            assert_eq!(
                get(&db, "thread", "t").unwrap()["activeProviderThreadId"],
                "pt"
            );
            assert_eq!(
                get(&db, "providerThreads", "pt").unwrap()["nativeThreadRef"],
                Value::Null
            );
            assert_eq!(list(&db, "contextHandoffs", "t").unwrap().len(), 1);
        }
        release(&s, "t", "r", Some("a"), first["id"].as_str().unwrap()).unwrap();
        {
            let db = s.db.lock().unwrap();
            let r = get(&db, "runs", "r").unwrap();
            assert!(bound(&db, &r, "old").unwrap_err().contains("reused"));
            bound(&db, &r, "new").unwrap();
            bound(&db, &r, "new").unwrap();
            assert!(bound(&db, &r, "other").unwrap_err().contains("changed"));
        }
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
