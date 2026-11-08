//! Forward-going accepted creation/message admissions shared with native T3 receipts.
//! The journal proves admission, not execution. Only a proven unstarted root turn
//! is reconstructed as durable preparation; ambiguous starts stay explicitly held.
use crate::{
    events,
    store::{random_id, State},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, String>;
type NativeReceiptRow = (String, String, String, String, i64, String, Option<String>);
type NativeEffectRow = (String, String, String, String, String);
const TABLE: &str = "o3_command_admissions";

/// Release no provider/preparation effect until the interchangeable database
/// contains the accepted command, immutable creation events and projections.
pub(crate) fn ensure_exported(state: &State, command: Option<&str>) -> Result<()> {
    ensure_exported_inner(state, command).map_err(|error| {
        if error.starts_with("shared_admission_recovery_required:") {
            error
        } else {
            format!("shared_admission_recovery_required: {error}")
        }
    })
}

fn ensure_exported_inner(state: &State, command: Option<&str>) -> Result<()> {
    let Some(mut native) = crate::t3_shared::open(&state.dir)? else {
        return Ok(());
    };
    let head: i64 = native
        .query_row(
            "SELECT max(migration_id) FROM effect_sql_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if head != 80
        && state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scheduled_task_webhook_pending)",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?
    {
        return Err("shared_admission_recovery_required: Native webhook admission sharing requires T3 schema 80; upgrade the shared profile before accepting deliveries.".into());
    }
    crate::t3_shared::export(state, &mut native).map_err(|e| {
        format!("shared_admission_recovery_required: Shared admission could not be persisted: {e}")
    })?;
    if let Some(cid) = command {
        let entry = {
            let db = state.db.lock().unwrap();
            db.query_row("SELECT command_id,thread_id,command_type,accepted_at,run_id,message_id FROM o3_command_admissions WHERE command_id=?1",[cid],read).optional().map_err(|e|e.to_string())?
        };
        if let Some(entry) = entry {
            native_evidence(&native, &entry)?;
            let accepted:bool=native.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_command_receipts WHERE command_id=?1 AND aggregate_kind='thread' AND aggregate_id=?2 AND command_type=?3 AND status='accepted')",params![entry.cid,entry.thread,entry.kind],|r|r.get(0)).map_err(|e|e.to_string())?;
            if !accepted {
                return Err(format!("shared_admission_recovery_required: Shared command {cid} has no accepted receipt."));
            }
        }
    }
    Ok(())
}

pub(crate) fn provider_fence(state: &State, run: &str) -> Result<()> {
    if crate::t3_shared::open(&state.dir)?.is_none() {
        return Ok(());
    }
    let command = {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let command: Option<String> = tx
            .query_row(
                "SELECT command_id FROM o3_command_admissions WHERE run_id=?1",
                [run],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(command) = &command {
            let mut record = events::get(&tx, "runs", run)?;
            if !matches!(record["status"].as_str(), Some("preparing" | "starting")) {
                return Err("shared_admission_recovery_required: Provider start is no longer an unstarted admitted turn.".into());
            }
            let thread = record["threadId"]
                .as_str()
                .ok_or("Admitted run has no thread")?
                .to_owned();
            let attempt = record["activeAttemptId"]
                .as_str()
                .ok_or("Admitted run has no attempt")?;
            let identity: String = tx
                .query_row("SELECT identity FROM o3_command_identity", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let id = format!("effect:o3-start:{identity}:{run}:{attempt}");
            let at = crate::store::iso(crate::store::now_ms());
            tx.execute("INSERT INTO o3_shared_effect_claims(effect_id,command_id,thread_id,run_id,claimed_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(effect_id) DO NOTHING",params![id,command,thread,run,at]).map_err(|e|e.to_string())?;
            if record["status"] == "preparing" {
                record["status"] = json!("starting");
                let projection = events::get(&tx, "thread", &thread)?;
                events::put(&tx, "runs", run, &thread, &record)?;
                events::append(&tx, &thread, "run.updated", record, &projection)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        command
    };
    ensure_exported(state, command.as_deref())
}

pub(crate) fn preparation_fence(state: &State, command: &str) -> Result<()> {
    let result = ensure_exported(state, Some(command));
    let db = state.db.lock().unwrap();
    match &result {
        Err(error) => {
            db.execute("INSERT INTO o3_shared_admission_recovery SELECT run_id,thread_id,'export_pending',?2 FROM thread_launches WHERE id=?1 AND run_id IS NOT NULL ON CONFLICT(run_id) DO UPDATE SET status='export_pending',reason=excluded.reason WHERE status!='blocked'",params![command,error]).map_err(|e|e.to_string())?;
        }
        Ok(()) => {
            db.execute("UPDATE o3_shared_admission_recovery SET status='resolved',reason=NULL WHERE status='export_pending' AND run_id=(SELECT run_id FROM thread_launches WHERE id=?1)",[command]).map_err(|e|e.to_string())?;
        }
    }
    result
}

pub(crate) fn defer_provider(state: &std::sync::Arc<State>, thread: &str, run: &str, error: &str) {
    let _=state.db.lock().unwrap().execute("INSERT INTO o3_shared_admission_recovery VALUES(?1,?2,'export_pending',?3) ON CONFLICT(run_id) DO UPDATE SET status='export_pending',reason=excluded.reason WHERE status!='blocked'",params![run,thread,error]);
    let weak = std::sync::Arc::downgrade(state);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Some(state) = weak.upgrade() {
            if let Err(error) = resume_deferred(&state) {
                eprintln!("Shared admission retry: {error}");
            }
        }
    });
}

pub(crate) fn resume_deferred(state: &std::sync::Arc<State>) -> Result<()> {
    let runs = {
        let db = state.db.lock().unwrap();
        let mut q=db.prepare("SELECT r.thread_id,r.run_id,m.body FROM o3_shared_admission_recovery r JOIN records v ON v.kind='runs' AND v.id=r.run_id JOIN records m ON m.kind='messages' AND m.id=json_extract(v.body,'$.userMessageId') WHERE r.status='export_pending' AND json_extract(v.body,'$.status') IN ('preparing','starting') AND json_extract(v.body,'$.startedAt') IS NULL AND NOT EXISTS(SELECT 1 FROM thread_launches l WHERE l.run_id=r.run_id AND l.completed=0 AND l.released=0)").map_err(|e|e.to_string())?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    for (thread, run, message) in runs {
        if !state.active.lock().unwrap().contains_key(&run) {
            let claimed=state.db.lock().unwrap().execute("UPDATE o3_shared_admission_recovery SET status='releasing' WHERE run_id=?1 AND status='export_pending'",[&run]).map_err(|e|e.to_string())?;
            if claimed == 0 {
                continue;
            }
            let message: Value = serde_json::from_str(&message).map_err(|e| e.to_string())?;
            crate::provider::start(
                state.clone(),
                thread,
                run,
                message["text"].as_str().unwrap_or("").into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS o3_command_admissions(command_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,command_type TEXT NOT NULL,accepted_at TEXT NOT NULL,creation_sequence INTEGER NOT NULL,run_id TEXT,message_id TEXT,origin TEXT NOT NULL); CREATE INDEX IF NOT EXISTS o3_command_run ON o3_command_admissions(run_id); CREATE INDEX IF NOT EXISTS o3_command_thread ON o3_command_admissions(thread_id,command_type); CREATE INDEX IF NOT EXISTS o3_command_message ON o3_command_admissions(message_id); CREATE TABLE IF NOT EXISTS o3_command_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),identity TEXT NOT NULL);").map_err(|e|e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS o3_shared_admission_recovery(run_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,status TEXT NOT NULL,reason TEXT);").map_err(|e|e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS o3_shared_effect_claims(effect_id TEXT PRIMARY KEY,command_id TEXT NOT NULL,thread_id TEXT NOT NULL,run_id TEXT NOT NULL,claimed_at TEXT NOT NULL,settled INTEGER NOT NULL DEFAULT 0); CREATE INDEX IF NOT EXISTS o3_shared_effect_unsettled ON o3_shared_effect_claims(settled) WHERE settled=0;").map_err(|e|e.to_string())?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM o3_command_identity)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        db.execute(
            "INSERT INTO o3_command_identity VALUES(1,?1)",
            [random_id()?],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn admit(
    db: &Connection,
    cid: &str,
    thread: &str,
    kind: &str,
    at: &str,
    sequence: i64,
    delivery: (Option<&str>, Option<&str>),
) -> Result<()> {
    let (run, message) = delivery;
    if !matches!(kind, "thread.create" | "message.dispatch") {
        return Ok(());
    }
    db.execute(
        "INSERT INTO o3_command_admissions VALUES(?1,?2,?3,?4,?5,?6,?7,'local')",
        params![cid, thread, kind, at, sequence, run, message],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Native receipt replay is scoped to its original thread, including when the
/// current command type differs (for example a launch replay through a bound send).
pub(crate) fn check_replay(db: &Connection, cid: &str, thread: Option<&str>) -> Result<()> {
    let owner: Option<String> = db
        .query_row(
            "SELECT thread_id FROM o3_command_admissions WHERE command_id=?1",
            [cid],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(owner) = owner {
        if Some(owner.as_str()) != thread {
            return Err(format!("Command {cid} was already handled for thread {owner} and cannot be replayed for {}.",thread.unwrap_or("")));
        }
    }
    if let Some(reason) = blocked(db, cid)? {
        return Err(format!("shared_admission_recovery_required: {reason}"));
    }
    Ok(())
}

pub(crate) fn blocked(db: &Connection, cid: &str) -> Result<Option<String>> {
    db.query_row("SELECT r.reason FROM o3_shared_admission_recovery r WHERE r.status IN ('blocked','export_pending','releasing') AND (r.run_id=(SELECT run_id FROM o3_command_admissions WHERE command_id=?1) OR r.run_id=(SELECT run_id FROM launch_claims WHERE command_id=?1)) AND EXISTS(SELECT 1 FROM records WHERE kind='runs' AND id=r.run_id AND json_extract(body,'$.status') IN ('preparing','starting','queued'))",[cid],|r|r.get(0)).optional().map_err(|e|e.to_string())
}

pub(crate) fn holds_unstarted(db: &Connection, run: &str) -> Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM o3_shared_admission_recovery r JOIN records v ON v.kind='runs' AND v.id=r.run_id WHERE r.run_id=?1 AND r.status IN ('blocked','export_pending','releasing') AND json_extract(v.body,'$.startedAt') IS NULL)",[run],|row|row.get(0)).map_err(|e|e.to_string())
}

fn recover_admission(
    db: &Connection,
    native: &Connection,
    entry: &Admission,
    thread: &Value,
) -> Result<()> {
    let Some(id) = entry.run.as_deref() else {
        return Ok(());
    };
    let mut run = events::get(db, "runs", id)?;
    if !matches!(
        run["status"].as_str(),
        Some("preparing" | "starting" | "queued")
    ) || !run["startedAt"].is_null()
    {
        db.execute(
            "UPDATE o3_shared_admission_recovery SET status='resolved',reason=NULL WHERE run_id=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let seen_turn:bool=native.query_row("SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_provider_turns WHERE json_extract(payload_json,'$.runAttemptId')=?1)",[run["activeAttemptId"].as_str()],|row|row.get(0)).map_err(|e|e.to_string())?;
    if seen_turn {
        db.execute(
            "UPDATE o3_shared_admission_recovery SET status='resolved',reason=NULL WHERE run_id=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let known: Option<String> = db
        .query_row(
            "SELECT status FROM o3_shared_admission_recovery WHERE run_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if known.as_deref().is_some_and(|status| status != "blocked") {
        return Ok(());
    }
    if run["status"] == "queued" && run["activeAttemptId"].is_null() {
        db.execute(
            "INSERT INTO o3_shared_admission_recovery VALUES(?1,?2,'queued',NULL) ON CONFLICT(run_id) DO UPDATE SET status='queued',reason=NULL",
            params![id, entry.thread],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let root_workspace = thread["worktreePath"].is_null()
        && run["workspacePreparation"].get("branch").is_none()
        && (run["workspacePreparation"].is_null() || run["workspacePreparation"]["type"] == "root");
    let (pending,claimed):(i64,i64)=native.query_row("SELECT coalesce(sum(CASE WHEN status='pending' AND attempt_count=0 THEN 1 ELSE 0 END),0),coalesce(sum(CASE WHEN status!='pending' OR attempt_count!=0 THEN 1 ELSE 0 END),0) FROM orchestration_v2_effect_outbox WHERE thread_id=?1 AND effect_type IN ('provider-turn.start','provider-turn.restart') AND json_extract(payload_json,'$.runId')=?2",params![entry.thread,id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    let proven = run["status"] == "preparing" && pending == 0 && claimed == 0
        || run["status"] == "starting" && pending == 1 && claimed == 0;
    if root_workspace && proven {
        let message = events::get(
            db,
            "messages",
            entry
                .message
                .as_deref()
                .ok_or("Imported admission lacks message")?,
        )?;
        let at = crate::store::iso(crate::store::now_ms());
        let item_id = format!("workspace-preparation:{id}");
        if events::get(db, "turnItems", &item_id).is_err() {
            let ordinal:i64=db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?1",[&entry.thread],|r|r.get(0)).map_err(|e|e.to_string())?;
            let item = json!({"id":item_id,"threadId":entry.thread,"runId":id,"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"running","title":"Recovering accepted turn","startedAt":at,"completedAt":null,"updatedAt":at,"type":"command_execution","input":"Preparing workspace"});
            crate::worktree_setup::save_item(db, &entry.thread, thread, item)?;
        }
        run["status"] = json!("preparing");
        run["executionRequested"] = json!(false);
        events::put(db, "runs", id, &entry.thread, &run)?;
        let plan = json!({"commandId":entry.cid,"projectId":thread["projectId"],"workspaceStrategy":{"type":"root"},"initialMessage":{"text":message["text"],"attachments":message["attachments"],"context":message["context"]}});
        crate::thread_launch::enqueue(db, &entry.thread, Some(id), &entry.cid, &plan)?;
        db.execute(
            "INSERT INTO o3_shared_admission_recovery VALUES(?1,?2,'prepared',NULL) ON CONFLICT(run_id) DO UPDATE SET status='prepared',reason=NULL",
            params![id, entry.thread],
        )
        .map_err(|e| e.to_string())?;
    } else {
        let reason = if !root_workspace {
            "Native workspace preparation may have run side effects. Inspect the thread workspace before explicitly retrying."
        } else {
            "Native provider start was already claimed or its delivery state is unknown. Inspect the native conversation before explicitly retrying."
        };
        db.execute(
            "INSERT INTO o3_shared_admission_recovery VALUES(?1,?2,'blocked',?3) ON CONFLICT(run_id) DO UPDATE SET status='blocked',reason=excluded.reason",
            params![id, entry.thread, reason],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn resume_queues(state: &std::sync::Arc<State>) -> Result<()> {
    let threads = {
        let db = state.db.lock().unwrap();
        let mut q = db
            .prepare(
                "SELECT DISTINCT thread_id FROM o3_shared_admission_recovery WHERE status='queued'",
            )
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    for thread in threads {
        crate::thread_commands::advance_checked(state, &thread)?;
    }
    resume_deferred(state)?;
    Ok(())
}

/// Only new native projection records receive admission events. Read their
/// immutable local event payload rather than relabelling a later terminal row.
pub(crate) fn creation(db: &Connection, kind: &str, id: &str) -> Result<Option<(String, Value)>> {
    let (field, command) = match kind {
        "thread" => ("thread_id", "thread.create"),
        "runs" => ("run_id", "message.dispatch"),
        "messages" => ("message_id", "message.dispatch"),
        _ => return Ok(None),
    };
    let row:Option<(String,i64)>=db.query_row(&format!("SELECT command_id,creation_sequence FROM {TABLE} WHERE {field}=?1 AND command_type=?2 AND origin='local'"),params![id,command],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?;
    let Some((cid, sequence)) = row else {
        return Ok(None);
    };
    let text:String=if kind=="messages" {
        db.query_row("SELECT body FROM events WHERE sequence<=?1 AND json_extract(body,'$.type')='message.updated' AND json_extract(body,'$.payload.id')=?2 ORDER BY sequence DESC LIMIT 1",params![sequence,id],|row|row.get(0))
    } else {db.query_row("SELECT body FROM events WHERE sequence=?1",[sequence],|row|row.get(0))}.map_err(|e|format!("Shared command {cid} lost its original admission event: {e}"))?;
    let event: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let expected = match kind {
        "thread" => "thread.created",
        "runs" => "run.created",
        _ => "message.updated",
    };
    if event["type"] != expected || event["payload"]["id"] != id {
        return Err(format!(
            "Shared command {cid} has inconsistent admission evidence"
        ));
    }
    Ok(Some((cid, event["payload"].clone())))
}

#[derive(Clone)]
struct Admission {
    cid: String,
    thread: String,
    kind: String,
    at: String,
    run: Option<String>,
    message: Option<String>,
}
fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Admission> {
    Ok(Admission {
        cid: row.get(0)?,
        thread: row.get(1)?,
        kind: row.get(2)?,
        at: row.get(3)?,
        run: row.get(4)?,
        message: row.get(5)?,
    })
}

fn native_evidence(native: &Connection, admission: &Admission) -> Result<i64> {
    let thread: bool = native
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads WHERE thread_id=?1)",
            [&admission.thread],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !thread {
        return Err(format!(
            "Shared command {} requires its thread projection",
            admission.cid
        ));
    }
    let event = if admission.kind == "thread.create" {
        "thread.created"
    } else {
        "run.created"
    };
    let linked:Option<String>=native.query_row("SELECT payload_json FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id=?1 AND command_id=?2 AND event_type=?3 AND application_event_version=2 ORDER BY sequence LIMIT 1",params![admission.thread,admission.cid,event],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let linked = linked.ok_or_else(|| {
        format!(
            "Shared command {} requires linked {event} admission evidence",
            admission.cid
        )
    })?;
    let payload: Value = serde_json::from_str(&linked).map_err(|e| e.to_string())?;
    if admission.kind == "thread.create" {
        if payload["id"] != admission.thread {
            return Err("Shared thread creation identity mismatch".into());
        }
    } else {
        let run = admission
            .run
            .as_deref()
            .ok_or("Shared message admission has no run")?;
        let message = admission
            .message
            .as_deref()
            .ok_or("Shared message admission has no message")?;
        if payload["id"] != run
            || payload["userMessageId"] != message
            || payload["threadId"] != admission.thread
        {
            return Err("Shared run creation identity mismatch".into());
        }
        for (table, id_field, id) in [
            ("orchestration_v2_projection_runs", "run_id", run),
            (
                "orchestration_v2_projection_messages",
                "message_id",
                message,
            ),
        ] {
            let body: Option<String> = native
                .query_row(
                    &format!(
                        "SELECT payload_json FROM {table} WHERE {id_field}=?1 AND thread_id=?2"
                    ),
                    params![id, admission.thread],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let body: Value = serde_json::from_str(&body.ok_or_else(|| {
                format!(
                    "Shared command {} requires its {table} projection",
                    admission.cid
                )
            })?)
            .map_err(|e| e.to_string())?;
            if table.ends_with("messages") && body["runId"] != run {
                return Err("Shared message/run identity mismatch".into());
            }
        }
    }
    native.query_row("SELECT max(sequence) FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id=?1 AND command_id=?2 AND application_event_version=2",params![admission.thread,admission.cid],|r|r.get(0)).map_err(|e|e.to_string())
}

struct EffectClaim {
    id: String,
    command: String,
    thread: String,
    run: String,
    at: String,
    status: String,
}

/// These process-bound effects are claimed before entering the provider adapter.
/// Native restart recovery cancels them; a receipt replay cannot prepare them again.
fn export_effects(state: &State, native: &Connection) -> Result<Vec<String>> {
    let mut after = String::new();
    let mut settled = vec![];
    loop {
        let claims = {
            let db = state.db.lock().unwrap();
            let mut query=db.prepare("SELECT c.effect_id,c.command_id,c.thread_id,c.run_id,c.claimed_at,coalesce(json_extract(r.body,'$.status'),'cancelled') FROM o3_shared_effect_claims c LEFT JOIN records r ON r.kind='runs' AND r.id=c.run_id WHERE c.settled=0 AND c.effect_id>?1 ORDER BY c.effect_id LIMIT 128").map_err(|e|e.to_string())?;
            let rows = query
                .query_map([&after], |r| {
                    Ok(EffectClaim {
                        id: r.get(0)?,
                        command: r.get(1)?,
                        thread: r.get(2)?,
                        run: r.get(3)?,
                        at: r.get(4)?,
                        status: r.get(5)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if claims.is_empty() {
            break;
        }
        after = claims.last().unwrap().id.clone();
        for claim in claims {
            let terminal = !matches!(
                claim.status.as_str(),
                "preparing" | "starting" | "running" | "waiting"
            );
            let status = match claim.status.as_str() {
                "completed" => "succeeded",
                "failed" => "failed",
                _ if terminal => "cancelled",
                _ => "running",
            };
            let existing:Option<NativeEffectRow>=native.query_row("SELECT command_id,thread_id,effect_type,payload_json,status FROM orchestration_v2_effect_outbox WHERE effect_id=?1",[&claim.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
            let payload = json!({"type":"provider-turn.start","runId":claim.run});
            if let Some((command, thread, kind, body, previous)) = existing {
                if command != claim.command
                    || thread != claim.thread
                    || kind != "provider-turn.start"
                    || serde_json::from_str::<Value>(&body).map_err(|e| e.to_string())? != payload
                {
                    return Err(format!(
                        "Shared provider effect identity conflict for {}",
                        claim.id
                    ));
                }
                if !terminal && previous != "running" {
                    return Err(format!("Shared provider effect {} was already {previous}; inspect the native conversation before retrying",claim.id));
                }
                if terminal {
                    native.execute("UPDATE orchestration_v2_effect_outbox SET status=?2,lease_owner=NULL,lease_expires_at=NULL,completed_at=coalesce(completed_at,?3),updated_at=?3 WHERE effect_id=?1",params![claim.id,status,crate::store::iso(crate::store::now_ms())]).map_err(|e|e.to_string())?;
                }
            } else {
                native.execute("INSERT INTO orchestration_v2_effect_outbox(effect_id,command_id,thread_id,effect_type,payload_json,status,attempt_count,available_at,lease_owner,created_at,updated_at,completed_at) VALUES(?1,?2,?3,'provider-turn.start',?4,?5,1,?6,'o3',?6,?6,?7)",params![claim.id,claim.command,claim.thread,payload.to_string(),status,claim.at,terminal.then_some(&claim.at)]).map_err(|e|e.to_string())?;
            }
            if terminal {
                settled.push(claim.id);
            }
        }
    }
    Ok(settled)
}

pub(crate) fn export(state: &State, native: &mut Connection) -> Result<usize> {
    let identity: String = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT identity FROM o3_command_identity", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let tx = native
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS o3_command_sync(singleton INTEGER PRIMARY KEY CHECK(singleton=1),local_identity TEXT NOT NULL,local_rowid INTEGER NOT NULL);").map_err(|e|e.to_string())?;
    let previous: Option<(String, i64)> = tx
        .query_row(
            "SELECT local_identity,local_rowid FROM o3_command_sync",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let mut cursor = previous
        .filter(|(saved, _)| saved == &identity)
        .map_or(0, |(_, cursor)| cursor);
    let mut written = 0;
    loop {
        let entries = {
            let db = state.db.lock().unwrap();
            let mut query=db.prepare("SELECT command_id,thread_id,command_type,accepted_at,run_id,message_id,rowid FROM o3_command_admissions WHERE rowid>?1 ORDER BY rowid LIMIT 128").map_err(|e|e.to_string())?;
            let rows = query
                .query_map([cursor], |row| Ok((read(row)?, row.get::<_, i64>(6)?)))
                .map_err(|e| e.to_string())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if entries.is_empty() {
            break;
        }
        for (entry, rowid) in entries {
            cursor = rowid;
            let local: bool = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT origin='local' FROM o3_command_admissions WHERE command_id=?1",
                    [&entry.cid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !local {
                continue;
            }
            let sequence = native_evidence(&tx, &entry)?;
            let existing:Option<NativeReceiptRow>=tx.query_row("SELECT aggregate_kind,aggregate_id,command_type,accepted_at,result_sequence,status,error FROM orchestration_command_receipts WHERE command_id=?1",[&entry.cid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(|e|e.to_string())?;
            let expected = (
                "thread".into(),
                entry.thread.clone(),
                entry.kind.clone(),
                entry.at.clone(),
                sequence,
                "accepted".into(),
                None,
            );
            if let Some(existing) = existing {
                if existing != expected {
                    return Err(format!(
                        "Shared command receipt conflict for {}; both admissions are retained",
                        entry.cid
                    ));
                }
            } else {
                tx.execute("INSERT INTO orchestration_command_receipts(command_id,aggregate_kind,aggregate_id,command_type,accepted_at,result_sequence,status,error) VALUES(?1,'thread',?2,?3,?4,?5,'accepted',NULL)",params![entry.cid,entry.thread,entry.kind,entry.at,sequence]).map_err(|e|e.to_string())?;
                written += 1;
            }
        }
    }
    tx.execute("INSERT INTO o3_command_sync VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET local_identity=excluded.local_identity,local_rowid=excluded.local_rowid",params![identity,cursor]).map_err(|e|e.to_string())?;
    let settled = export_effects(state, &tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    if !settled.is_empty() {
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        for id in settled {
            tx.execute(
                "UPDATE o3_shared_effect_claims SET settled=1 WHERE effect_id=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(written)
}

pub(crate) fn import(state: &State, native: &Connection) -> Result<usize> {
    let mut after = String::new();
    let mut count = 0;
    loop {
        let entries = {
            let mut query=native.prepare("SELECT command_id,aggregate_id,command_type,accepted_at,NULL,NULL FROM orchestration_command_receipts WHERE command_id>?1 AND aggregate_kind='thread' AND command_type IN ('thread.create','message.dispatch') AND status='accepted' ORDER BY command_id LIMIT 128").map_err(|e|e.to_string())?;
            let rows = query.query_map([&after], read).map_err(|e| e.to_string())?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if entries.is_empty() {
            break;
        }
        after = entries.last().unwrap().cid.clone();
        let mut db = state.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        for mut entry in entries {
            let local_thread = events::get(&tx, "thread", &entry.thread)?;
            if entry.kind == "message.dispatch" {
                let payload:Option<String>=native.query_row("SELECT payload_json FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id=?1 AND command_id=?2 AND event_type='run.created' AND application_event_version=2 ORDER BY sequence LIMIT 1",params![entry.thread,entry.cid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
                // Message steering and maintenance are not new-run admissions. They
                // require their own effect delivery model rather than a fake run receipt.
                let Some(payload) = payload else { continue };
                let run: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                entry.run = run["id"].as_str().map(str::to_owned);
                entry.message = run["userMessageId"].as_str().map(str::to_owned);
                native_evidence(native, &entry)?;
                let local_run = events::get(&tx, "runs", entry.run.as_deref().unwrap())?;
                let local_message =
                    events::get(&tx, "messages", entry.message.as_deref().unwrap())?;
                if local_run["threadId"] != entry.thread
                    || local_run["userMessageId"] != entry.message.as_deref().unwrap()
                    || local_message["runId"] != entry.run.as_deref().unwrap()
                {
                    return Err(
                        "Imported command admission does not match its local projections".into(),
                    );
                }
            } else {
                native_evidence(native, &entry)?;
            }
            let existing:Option<(String,String,Option<String>,Option<String>)>=tx.query_row("SELECT thread_id,command_type,run_id,message_id FROM o3_command_admissions WHERE command_id=?1",[&entry.cid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
            let expected = (
                entry.thread.clone(),
                entry.kind.clone(),
                entry.run.clone(),
                entry.message.clone(),
            );
            if let Some(existing) = existing {
                if existing != expected {
                    return Err(format!(
                        "Shared command receipt conflict for {}; both admissions are retained",
                        entry.cid
                    ));
                }
                let imported: bool = tx
                    .query_row(
                        "SELECT origin='native' FROM o3_command_admissions WHERE command_id=?1",
                        [&entry.cid],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if imported {
                    recover_admission(&tx, native, &entry, &local_thread)?;
                }
                continue;
            }
            let receipt: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM receipts WHERE command_id=?1)",
                    [&entry.cid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if receipt {
                return Err(format!("Shared command {} conflicts with an unjournaled local receipt; both copies are retained",entry.cid));
            }
            recover_admission(&tx, native, &entry, &local_thread)?;
            tx.execute(
                "INSERT INTO o3_command_admissions VALUES(?1,?2,?3,?4,0,?5,?6,'native')",
                params![
                    entry.cid,
                    entry.thread,
                    entry.kind,
                    entry.at,
                    entry.run,
                    entry.message
                ],
            )
            .map_err(|e| e.to_string())?;
            let sequence: i64 = tx
                .query_row("SELECT coalesce(max(sequence),0) FROM events", [], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            let mut result = json!({"sequence":sequence});
            if entry.kind == "thread.create" {
                result["threadId"] = json!(entry.thread);
                tx.execute("INSERT INTO launch_claims(command_id,thread_id,project_id) VALUES(?1,?2,?3) ON CONFLICT(command_id) DO NOTHING",params![entry.cid,entry.thread,local_thread["projectId"].as_str().ok_or("Imported launch has no project")?]).map_err(|e|e.to_string())?;
            }
            tx.execute(
                "INSERT INTO receipts VALUES(?1,?2)",
                params![entry.cid, result.to_string()],
            )
            .map_err(|e| e.to_string())?;
            count += 1;
        }
        // The base may sort before its accepted initial message; update claims only
        // from a proven journal entry, without creating a preparation or provider effect.
        tx.execute("UPDATE launch_claims SET run_id=(SELECT run_id FROM o3_command_admissions a WHERE a.command_id=launch_claims.command_id||':initial-message' AND a.thread_id=launch_claims.thread_id AND a.command_type='message.dispatch') WHERE run_id IS NULL AND EXISTS(SELECT 1 FROM o3_command_admissions a WHERE a.command_id=launch_claims.command_id||':initial-message' AND a.thread_id=launch_claims.thread_id AND a.run_id IS NOT NULL)",[]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn native_pending_worker_parks_shared_recovery_and_resumes_only_after_progress() {
        assert!(
            std::path::Path::new("/run/.containerenv").is_file()
                || std::path::Path::new("/.dockerenv").is_file(),
            "Run this regression only in the rootless container harness"
        );
        let path =
            std::env::temp_dir().join(format!("o3-pending-recovery-{}", random_id().unwrap()));
        let state = Arc::new(State::open(&path).unwrap());
        let mut ordinal = 0;
        for alias in [false, true] {
            for recovery in ["blocked", "export_pending", "releasing"] {
                for run_status in ["preparing", "starting", "queued"] {
                    ordinal += 1;
                    let delivery = format!("delivery-{ordinal}");
                    let run = format!("run-{ordinal}");
                    let base = format!("scheduled-task:task:webhook:{delivery}");
                    let command = if alias {
                        format!("{base}:initial-message")
                    } else {
                        base
                    };
                    {
                        let db = state.db.lock().unwrap();
                        // Deliberately invalid dispatch payloads make accidental worker
                        // admission observable as a removed row or released replay key.
                        events::put(
                            &db,
                            "scheduledTasks",
                            "task",
                            "",
                            &json!({"id":"task","runCount":0}),
                        )
                        .unwrap();
                        events::put(&db,"runs",&run,"thread",&json!({"id":run,"threadId":"thread","status":run_status,"startedAt":null})).unwrap();
                        admit(
                            &db,
                            &command,
                            "thread",
                            "message.dispatch",
                            "2026-11-06T00:00:00.000Z",
                            0,
                            (Some(&run), Some("message")),
                        )
                        .unwrap();
                        db.execute("INSERT INTO o3_shared_admission_recovery VALUES(?1,'thread',?2,'Synthetic recovery hold')",params![run,recovery]).unwrap();
                        db.execute("INSERT INTO scheduled_task_webhook_pending VALUES(?1,'task','',0,'running','{}','{}','[]')",[&delivery]).unwrap();
                        db.execute("INSERT INTO scheduled_task_webhook_replay_keys VALUES('task',?1,?1,9223372036854775807)",[&delivery]).unwrap();
                    }
                    assert_eq!(
                        crate::webhook_native::next_due(&state),
                        None,
                        "{command}/{recovery}/{run_status}"
                    );
                    crate::webhook_native::tick(&state);
                    {
                        let db = state.db.lock().unwrap();
                        let pending:i64=db.query_row("SELECT count(*) FROM scheduled_task_webhook_pending WHERE delivery_id=?1",[&delivery],|r|r.get(0)).unwrap();
                        let replay:i64=db.query_row("SELECT count(*) FROM scheduled_task_webhook_replay_keys WHERE delivery_id=?1",[&delivery],|r|r.get(0)).unwrap();
                        assert_eq!((pending, replay), (1, 1));
                        assert_eq!(
                            events::get(&db, "scheduledTasks", "task").unwrap()["runCount"],
                            0
                        );
                        assert!(check_replay(&db, &command, Some("thread"))
                            .unwrap_err()
                            .starts_with("shared_admission_recovery_required:"));
                        db.execute("UPDATE o3_shared_admission_recovery SET status='resolved' WHERE run_id=?1",[&run]).unwrap();
                    }
                    assert_eq!(crate::webhook_native::next_due(&state), Some(0));
                    for progressed in ["running", "completed"] {
                        let db = state.db.lock().unwrap();
                        db.execute(
                            "UPDATE o3_shared_admission_recovery SET status=?2 WHERE run_id=?1",
                            params![run, recovery],
                        )
                        .unwrap();
                        db.execute("UPDATE records SET body=json_set(body,'$.status',?2) WHERE kind='runs' AND id=?1",params![run,progressed]).unwrap();
                        drop(db);
                        assert_eq!(crate::webhook_native::next_due(&state), Some(0));
                    }
                    {
                        let db = state.db.lock().unwrap();
                        db.execute("UPDATE records SET body=json_set(body,'$.status','preparing') WHERE kind='runs' AND id=?1",[&run]).unwrap();
                        db.execute("UPDATE scheduled_task_webhook_pending SET state='completed' WHERE delivery_id=?1",[&delivery]).unwrap();
                    }
                    // Committed terminal delivery cleanup is never parked by an
                    // unrelated stale recovery marker and preserves accepted replay.
                    assert_eq!(crate::webhook_native::next_due(&state), Some(0));
                    crate::webhook_native::tick(&state);
                    let db = state.db.lock().unwrap();
                    assert_eq!(
                        db.query_row(
                            "SELECT count(*) FROM scheduled_task_webhook_pending",
                            [],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                        0
                    );
                    assert_eq!(db.query_row("SELECT count(*) FROM scheduled_task_webhook_replay_keys WHERE delivery_id=?1",[&delivery],|r|r.get::<_,i64>(0)).unwrap(),1);
                }
            }
        }
        assert_eq!(ordinal, 18);
    }
}
