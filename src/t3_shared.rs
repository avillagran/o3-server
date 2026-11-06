//! Shared base with T3. O3 and T3 run on one profile and one thread history: every record O3
//! changes is written to T3's `statev2.sqlite` the way T3's own event sink writes it (an event in
//! `orchestration_events` plus the projection rows its reducer would leave), and at start O3 takes
//! in whatever T3 wrote while it owned the profile. See `docs/SHARED-STATE.md`.
//!
//! Only one server owns a profile at a time (`profile::refuse_live` and the ownership lock), so the
//! two directions never race: T3 events past `o3SeenSequence` are T3's, everything else is ours.
use crate::events;
use crate::store::{iso, now_ms, random_id, State};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Result<T> = std::result::Result<T, String>;

/// The newest T3 migration whose tables and reducers this module writes.
pub const MIGRATION_HEAD: (i64, &str) = (73, "ActiveTurnItemIndexes");
/// T3's `ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION` at that migration.
pub const PROJECTION_SCHEMA_VERSION: i64 = 2;
/// Cursor in `statev2.sqlite`: the last event sequence O3 has seen or written.
const SEEN: &str = "o3SeenSequence";
/// Version of the O3-to-T3 value mapping (`shape_values`, `t3_shape`). Raising it makes the next
/// start compare every record again, so rows written under an older mapping are corrected.
const SHAPE_VERSION: i64 = 2;
const SHAPED: &str = "o3ShapeVersion";
/// Bodies read per export transaction, bounded so a run of large tool outputs stays small.
const BATCH_ROWS: usize = 256;
const BATCH_BYTES: usize = 8 << 20;

/// One O3 record kind and the T3 projection table that holds it.
struct Kind {
    kind: &'static str,
    table: &'static str,
    id: &'static str,
    created: Option<&'static str>,
    updated: &'static str,
}

const KINDS: &[Kind] = &[
    Kind {
        kind: "thread",
        table: "orchestration_v2_projection_threads",
        id: "thread_id",
        created: Some("thread.created"),
        updated: "thread.metadata-updated",
    },
    Kind {
        kind: "runs",
        table: "orchestration_v2_projection_runs",
        id: "run_id",
        created: Some("run.created"),
        updated: "run.updated",
    },
    Kind {
        kind: "attempts",
        table: "orchestration_v2_projection_run_attempts",
        id: "attempt_id",
        created: Some("run-attempt.created"),
        updated: "run-attempt.updated",
    },
    Kind {
        kind: "nodes",
        table: "orchestration_v2_projection_nodes",
        id: "node_id",
        created: None,
        updated: "node.updated",
    },
    Kind {
        kind: "subagents",
        table: "orchestration_v2_projection_subagents",
        id: "subagent_id",
        created: None,
        updated: "subagent.updated",
    },
    Kind {
        kind: "providerSessions",
        table: "orchestration_v2_projection_provider_sessions",
        id: "provider_session_id",
        created: Some("provider-session.attached"),
        updated: "provider-session.updated",
    },
    Kind {
        kind: "providerThreads",
        table: "orchestration_v2_projection_provider_threads",
        id: "provider_thread_id",
        created: None,
        updated: "provider-thread.updated",
    },
    Kind {
        kind: "providerTurns",
        table: "orchestration_v2_projection_provider_turns",
        id: "provider_turn_id",
        created: None,
        updated: "provider-turn.updated",
    },
    Kind {
        kind: "runtimeRequests",
        table: "orchestration_v2_projection_runtime_requests",
        id: "runtime_request_id",
        created: None,
        updated: "runtime-request.updated",
    },
    Kind {
        kind: "messages",
        table: "orchestration_v2_projection_messages",
        id: "message_id",
        created: None,
        updated: "message.updated",
    },
    Kind {
        kind: "plans",
        table: "orchestration_v2_projection_plans",
        id: "plan_id",
        created: None,
        updated: "plan.updated",
    },
    Kind {
        kind: "turnItems",
        table: "orchestration_v2_projection_turn_items",
        id: "turn_item_id",
        created: None,
        updated: "turn-item.updated",
    },
    Kind {
        kind: "checkpointScopes",
        table: "orchestration_v2_projection_checkpoint_scopes",
        id: "scope_id",
        created: None,
        updated: "checkpoint-scope.created",
    },
    Kind {
        kind: "checkpoints",
        table: "orchestration_v2_projection_checkpoints",
        id: "checkpoint_id",
        created: None,
        updated: "checkpoint.captured",
    },
    Kind {
        kind: "contextHandoffs",
        table: "orchestration_v2_projection_context_handoffs",
        id: "context_handoff_id",
        created: None,
        updated: "context-handoff.updated",
    },
    Kind {
        kind: "contextTransfers",
        table: "orchestration_v2_projection_context_transfers",
        id: "context_transfer_id",
        created: Some("context-transfer.created"),
        updated: "context-transfer.updated",
    },
];

const PROJECTS: Kind = Kind {
    kind: "project",
    table: "projection_projects",
    id: "project_id",
    created: None,
    updated: "",
};

fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.kind == name)
}

/// The record kind and id a T3 thread event changes. `None` for events without a projection row.
fn event_target(event_type: &str, payload: &Value) -> Option<(&'static str, String)> {
    let id = |field: &str| payload[field].as_str().map(str::to_owned);
    let kind = match event_type {
        t if t.starts_with("thread.") => "thread",
        "run.background-work-cancelled" => return id("runId").map(|id| ("runs", id)),
        "provider-session.detached" | "checkpoint.rollback-requested" => return None,
        t => {
            KINDS
                .iter()
                .find(|k| k.updated == t || k.created == Some(t))?
                .kind
        }
    };
    id("id").map(|id| (kind, id))
}

pub fn database(dir: &Path) -> PathBuf {
    dir.join("statev2.sqlite")
}

fn kinds_sql() -> String {
    let mut names: Vec<String> = KINDS.iter().map(|k| format!("'{}'", k.kind)).collect();
    names.push("'project'".into());
    names.join(",")
}

/// O3 side: every write to a shared kind marks the record for export.
pub fn initialize(db: &Connection) -> Result<()> {
    let kinds = kinds_sql();
    // Conflict-free on purpose: an outer statement's conflict policy overrides a trigger's own.
    let mark = "UPDATE t3_dirty SET version=version+1,thread_id=new.thread_id WHERE kind=new.kind AND id=new.id;
           INSERT INTO t3_dirty SELECT new.kind,new.id,new.thread_id,1 WHERE NOT EXISTS(SELECT 1 FROM t3_dirty WHERE kind=new.kind AND id=new.id);";
    // Sessions are shared too: a client paired with either server is accepted by the other.
    let session = "UPDATE t3_dirty SET version=version+1 WHERE kind='authSession' AND id=new.session_id;
           INSERT INTO t3_dirty SELECT 'authSession',new.session_id,'',1 WHERE NOT EXISTS(SELECT 1 FROM t3_dirty WHERE kind='authSession' AND id=new.session_id);";
    db.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS t3_dirty(kind TEXT NOT NULL,id TEXT NOT NULL,thread_id TEXT NOT NULL,version INTEGER NOT NULL,PRIMARY KEY(kind,id));
         CREATE TABLE IF NOT EXISTS t3_rejects(kind TEXT NOT NULL,id TEXT NOT NULL,error TEXT NOT NULL,at INTEGER NOT NULL,PRIMARY KEY(kind,id));
         DROP TRIGGER IF EXISTS t3_dirty_insert;
         DROP TRIGGER IF EXISTS t3_dirty_update;
         CREATE TRIGGER t3_dirty_insert AFTER INSERT ON records WHEN new.kind IN ({kinds}) BEGIN {mark} END;
         CREATE TRIGGER t3_dirty_update AFTER UPDATE OF body ON records WHEN new.kind IN ({kinds}) BEGIN {mark} END;"
    ))
    .map_err(|e| e.to_string())?;
    let sessions: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='auth_sessions')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !sessions {
        return Ok(());
    }
    db.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS t3_session_insert;
         DROP TRIGGER IF EXISTS t3_session_update;
         CREATE TRIGGER t3_session_insert AFTER INSERT ON auth_sessions BEGIN {session} END;
         CREATE TRIGGER t3_session_update AFTER UPDATE ON auth_sessions BEGIN {session} END;"
    ))
    .map_err(|e| e.to_string())
}

/// Opens T3's database for writing, or `None` when the profile has none yet (T3 creates it on its
/// first start; until then the dirty marks wait). Refuses a schema this module was not written for.
pub fn open(dir: &Path) -> Result<Option<Connection>> {
    let path = database(dir);
    if !path.is_file() {
        return Ok(None);
    }
    let db = Connection::open(&path).map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON; PRAGMA cache_size=-1024; PRAGMA mmap_size=0;")
        .map_err(|e| e.to_string())?;
    let head: Option<(i64, String)> = db
        .query_row(
            "SELECT migration_id,name FROM effect_sql_migrations ORDER BY migration_id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if head.as_ref().map(|(id, name)| (*id, name.as_str())) != Some(MIGRATION_HEAD) {
        return Err(format!(
            "{} is at T3 migration {:?}; O3 writes only {:?}",
            path.display(),
            head,
            MIGRATION_HEAD
        ));
    }
    let version: Option<i64> = db
        .query_row(
            "SELECT schema_version FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    // A base T3 just created says 1 until its first event writes the current version, as O3's
    // first export then does.
    let empty: bool = db
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM orchestration_v2_projection_threads)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if version.is_some_and(|v| v != PROJECTION_SCHEMA_VERSION && !(v == 1 && empty)) {
        return Err(format!(
            "T3 projection schema {version:?}, O3 writes {PROJECTION_SCHEMA_VERSION}"
        ));
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS o3_sync(name TEXT PRIMARY KEY,value INTEGER NOT NULL)",
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(db))
}

fn seen(t3: &Connection) -> Result<Option<i64>> {
    t3.query_row("SELECT value FROM o3_sync WHERE name=?", [SEEN], |r| {
        r.get(0)
    })
    .optional()
    .map_err(|e| e.to_string())
}

fn latest(t3: &Connection) -> Result<i64> {
    t3.query_row(
        "SELECT coalesce(max(sequence),0) FROM orchestration_events",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn set_seen(t3: &Connection, sequence: i64) -> Result<()> {
    t3.execute(
        "INSERT INTO o3_sync VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=max(value,excluded.value)",
        params![SEEN, sequence],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn text(v: &Value, field: &str) -> Option<String> {
    v.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn int(v: &Value, field: &str) -> Option<i64> {
    v.get(field).and_then(Value::as_i64)
}

fn bit(v: &Value, field: &str) -> i64 {
    i64::from(v.get(field).and_then(Value::as_bool).unwrap_or(false))
}

fn required(v: &Value, field: &str) -> Result<String> {
    text(v, field).ok_or_else(|| format!("{field} missing"))
}

fn payload_row(t3: &Connection, k: &Kind, id: &str) -> Result<Option<String>> {
    t3.query_row(
        &format!("SELECT payload_json FROM {} WHERE {}=?", k.table, k.id),
        [id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// The ISO time a record's change happened at, for the event T3 stores with it.
fn occurred_at(v: &Value) -> String {
    [
        "updatedAt",
        "completedAt",
        "capturedAt",
        "resolvedAt",
        "createdAt",
        "requestedAt",
        "startedAt",
    ]
    .iter()
    .find_map(|f| text(v, f))
    .unwrap_or_else(|| iso(now_ms()))
}

/// Appends one agent event the way T3's `appendAgentEvents` does and returns its sequence.
fn append_thread_event(
    tx: &Transaction,
    tid: &str,
    event_type: &str,
    v: &Value,
    at: &str,
) -> Result<i64> {
    let mut metadata = Map::new();
    for field in ["runId", "nodeId", "driver", "providerInstanceId"] {
        if let Some(value) = v.get(field).filter(|value| value.is_string()) {
            metadata.insert(field.into(), value.clone());
        }
    }
    tx.query_row(
        "INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,command_id,causation_event_id,correlation_id,actor_kind,payload_json,metadata_json,application_event_version)
         VALUES(?1,'thread',?2,coalesce((SELECT max(stream_version)+1 FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id=?2),0),?3,?4,NULL,NULL,NULL,'server',?5,?6,2)
         RETURNING sequence",
        params![random_id()?, tid, event_type, at, v.to_string(), Value::Object(metadata).to_string()],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn append_project_event(
    tx: &Transaction,
    id: &str,
    event_type: &str,
    payload: &Value,
    at: &str,
) -> Result<i64> {
    tx.query_row(
        "INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,command_id,causation_event_id,correlation_id,actor_kind,payload_json,metadata_json,application_event_version)
         VALUES(?1,'project',?2,coalesce((SELECT stream_version+1 FROM orchestration_events WHERE aggregate_kind='project' AND stream_id=?2 ORDER BY stream_version DESC LIMIT 1),0),?3,?4,NULL,NULL,NULL,'server',?5,'{}',2)
         RETURNING sequence",
        params![random_id()?, id, event_type, at, payload.to_string()],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn thread_event_type(existing: Option<&Value>, v: &Value, created: bool) -> &'static str {
    let Some(old) = existing else {
        return if created {
            "thread.created"
        } else {
            "thread.metadata-updated"
        };
    };
    if old["deletedAt"].is_null() && !v["deletedAt"].is_null() {
        "thread.deleted"
    } else if old["archivedAt"].is_null() && !v["archivedAt"].is_null() {
        "thread.archived"
    } else if !old["archivedAt"].is_null() && v["archivedAt"].is_null() {
        "thread.unarchived"
    } else {
        "thread.metadata-updated"
    }
}

/// T3's `ProjectionStore.apply` for one upsert event: the projection row its reducer leaves.
fn project_row(
    tx: &Transaction,
    k: &Kind,
    tid: &str,
    v: &Value,
    existing: Option<&Value>,
    created: bool,
) -> Result<()> {
    let id = required(v, "id")?;
    let body = v.to_string();
    let run = |sql: &str, values: &[&dyn rusqlite::ToSql]| -> Result<()> {
        tx.execute(sql, values)
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    match k.kind {
        "thread" => run(
            "INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,interaction_mode,active_provider_thread_id,created_at,updated_at,archived_at,deleted_at,payload_json)
             VALUES(?1,?2,?3,?4,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(thread_id) DO UPDATE SET project_id=excluded.project_id,title=excluded.title,default_provider=excluded.default_provider,provider_instance_id=excluded.provider_instance_id,runtime_mode=excluded.runtime_mode,interaction_mode=excluded.interaction_mode,active_provider_thread_id=excluded.active_provider_thread_id,created_at=excluded.created_at,updated_at=excluded.updated_at,archived_at=excluded.archived_at,deleted_at=excluded.deleted_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "projectId")?, &required(v, "title")?, &required(v, "providerInstanceId")?, &required(v, "runtimeMode")?, &required(v, "interactionMode")?, &text(v, "activeProviderThreadId"), &required(v, "createdAt")?, &required(v, "updatedAt")?, &text(v, "archivedAt"), &text(v, "deletedAt"), &body],
        ),
        "runs" => {
            let keep = |inner: &str, path: &str| {
                format!("CASE WHEN json_type(excluded.payload_json,'{path}') IS NULL AND json_type(orchestration_v2_projection_runs.payload_json,'{path}') IS NOT NULL THEN json_set({inner},'{path}',json_extract(orchestration_v2_projection_runs.payload_json,'{path}')) ELSE {inner} END")
            };
            let payload = keep(&keep("excluded.payload_json", "$.delegatedCompletion"), "$.restartCancelledBackgroundWork");
            run(
                &format!("INSERT INTO orchestration_v2_projection_runs(run_id,thread_id,ordinal,provider,provider_instance_id,provider_thread_id,status,requested_at,completed_at,payload_json)
                 VALUES(?1,?2,?3,?4,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(run_id) DO UPDATE SET thread_id=excluded.thread_id,ordinal=excluded.ordinal,provider=excluded.provider,provider_instance_id=excluded.provider_instance_id,provider_thread_id=excluded.provider_thread_id,status=excluded.status,requested_at=excluded.requested_at,completed_at=excluded.completed_at,payload_json={payload}"),
                &[&id, &required(v, "threadId")?, &int(v, "ordinal").ok_or("ordinal missing")?, &required(v, "providerInstanceId")?, &text(v, "providerThreadId"), &required(v, "status")?, &required(v, "requestedAt")?, &text(v, "completedAt"), &body],
            )
        }
        "attempts" => run(
            "INSERT INTO orchestration_v2_projection_run_attempts(attempt_id,thread_id,run_id,attempt_ordinal,root_node_id,provider,provider_instance_id,provider_thread_id,provider_turn_id,status,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?6,?7,?8,?9,?10)
             ON CONFLICT(attempt_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,attempt_ordinal=excluded.attempt_ordinal,root_node_id=excluded.root_node_id,provider=excluded.provider,provider_instance_id=excluded.provider_instance_id,provider_thread_id=excluded.provider_thread_id,provider_turn_id=excluded.provider_turn_id,status=excluded.status,payload_json=excluded.payload_json",
            &[&id, &tid, &required(v, "runId")?, &int(v, "attemptOrdinal").ok_or("attemptOrdinal missing")?, &required(v, "rootNodeId")?, &required(v, "providerInstanceId")?, &required(v, "providerThreadId")?, &text(v, "providerTurnId"), &required(v, "status")?, &body],
        ),
        "nodes" => run(
            "INSERT INTO orchestration_v2_projection_nodes(node_id,thread_id,run_id,parent_node_id,root_node_id,kind,status,provider_thread_id,provider_turn_id,started_at,completed_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(node_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,parent_node_id=excluded.parent_node_id,root_node_id=excluded.root_node_id,kind=excluded.kind,status=excluded.status,provider_thread_id=excluded.provider_thread_id,provider_turn_id=excluded.provider_turn_id,started_at=excluded.started_at,completed_at=excluded.completed_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &text(v, "runId"), &text(v, "parentNodeId"), &required(v, "rootNodeId")?, &required(v, "kind")?, &required(v, "status")?, &text(v, "providerThreadId"), &text(v, "providerTurnId"), &text(v, "startedAt"), &text(v, "completedAt"), &body],
        ),
        "subagents" => run(
            "INSERT INTO orchestration_v2_projection_subagents(subagent_id,thread_id,run_id,parent_node_id,provider,driver,provider_instance_id,provider_thread_id,child_thread_id,origin,status,started_at,completed_at,updated_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?5,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(subagent_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,parent_node_id=excluded.parent_node_id,provider=excluded.provider,driver=excluded.driver,provider_instance_id=excluded.provider_instance_id,provider_thread_id=excluded.provider_thread_id,child_thread_id=excluded.child_thread_id,origin=excluded.origin,status=excluded.status,started_at=excluded.started_at,completed_at=excluded.completed_at,updated_at=excluded.updated_at,
               payload_json=CASE WHEN json_type(excluded.payload_json,'$.completionDelivery') IS NULL AND json_type(orchestration_v2_projection_subagents.payload_json,'$.completionDelivery') IS NOT NULL THEN json_set(excluded.payload_json,'$.completionDelivery',json_extract(orchestration_v2_projection_subagents.payload_json,'$.completionDelivery')) ELSE excluded.payload_json END",
            &[&id, &required(v, "threadId")?, &text(v, "runId"), &required(v, "parentNodeId")?, &required(v, "providerInstanceId")?, &text(v, "driver"), &text(v, "providerThreadId"), &text(v, "childThreadId"), &required(v, "origin")?, &required(v, "status")?, &text(v, "startedAt"), &text(v, "completedAt"), &required(v, "updatedAt")?, &body],
        ),
        "providerSessions" => {
            run(
                "INSERT INTO orchestration_v2_projection_provider_sessions(provider_session_id,thread_id,provider,driver,provider_instance_id,status,updated_at,payload_json)
                 VALUES(?1,?2,?3,?4,?3,?5,?6,?7)
                 ON CONFLICT(provider_session_id) DO UPDATE SET thread_id=excluded.thread_id,provider=excluded.provider,driver=excluded.driver,provider_instance_id=excluded.provider_instance_id,status=excluded.status,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
                &[&id, &tid, &required(v, "providerInstanceId")?, &text(v, "driver"), &required(v, "status")?, &required(v, "updatedAt")?, &body],
            )?;
            if created {
                run(
                    "INSERT OR IGNORE INTO orchestration_v2_projection_provider_session_bindings(provider_session_id,thread_id) VALUES(?1,?2)",
                    &[&id, &tid],
                )?;
            }
            Ok(())
        }
        "providerThreads" => {
            run(
                "INSERT INTO orchestration_v2_projection_provider_threads(provider_thread_id,thread_id,owner_node_id,provider,driver,provider_instance_id,provider_session_id,status,first_run_ordinal,last_run_ordinal,updated_at,payload_json)
                 VALUES(?1,?2,?3,?4,?5,?4,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(provider_thread_id) DO UPDATE SET thread_id=excluded.thread_id,owner_node_id=excluded.owner_node_id,provider=excluded.provider,driver=excluded.driver,provider_instance_id=excluded.provider_instance_id,provider_session_id=excluded.provider_session_id,status=excluded.status,first_run_ordinal=excluded.first_run_ordinal,last_run_ordinal=excluded.last_run_ordinal,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
                &[&id, &text(v, "appThreadId"), &text(v, "ownerNodeId"), &required(v, "providerInstanceId")?, &text(v, "driver"), &text(v, "providerSessionId"), &required(v, "status")?, &int(v, "firstRunOrdinal"), &int(v, "lastRunOrdinal"), &required(v, "updatedAt")?, &body],
            )?;
            let placeholder = v["status"] == "not_loaded"
                && v["firstRunOrdinal"].is_null()
                && v["nativeThreadRef"].is_null()
                && v["providerSessionId"].is_null();
            if let (Some(app), false) = (text(v, "appThreadId"), placeholder) {
                let updated = required(v, "updatedAt")?;
                run(
                    "UPDATE orchestration_v2_projection_threads SET active_provider_thread_id=?1,updated_at=?2,payload_json=json_set(payload_json,'$.activeProviderThreadId',?1,'$.updatedAt',?2) WHERE thread_id=?3",
                    &[&id, &updated, &app],
                )?;
            }
            Ok(())
        }
        "providerTurns" => {
            // T3 keeps the last token usage a later update leaves out.
            let mut turn = v.clone();
            if v.get("tokenUsage").is_none() {
                if let Some(usage) = existing.and_then(|old| old.get("tokenUsage")) {
                    turn["tokenUsage"] = usage.clone();
                }
            }
            run(
                "INSERT INTO orchestration_v2_projection_provider_turns(provider_turn_id,thread_id,provider_thread_id,node_id,run_attempt_id,ordinal,status,started_at,completed_at,payload_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(provider_turn_id) DO UPDATE SET thread_id=excluded.thread_id,provider_thread_id=excluded.provider_thread_id,node_id=excluded.node_id,run_attempt_id=excluded.run_attempt_id,ordinal=excluded.ordinal,status=excluded.status,started_at=excluded.started_at,completed_at=excluded.completed_at,payload_json=excluded.payload_json",
                &[&id, &tid, &required(&turn, "providerThreadId")?, &required(&turn, "nodeId")?, &text(&turn, "runAttemptId"), &int(&turn, "ordinal").ok_or("ordinal missing")?, &required(&turn, "status")?, &text(&turn, "startedAt"), &text(&turn, "completedAt"), &turn.to_string()],
            )
        }
        "runtimeRequests" => run(
            "INSERT INTO orchestration_v2_projection_runtime_requests(runtime_request_id,thread_id,node_id,provider_turn_id,kind,status,created_at,resolved_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(runtime_request_id) DO UPDATE SET thread_id=excluded.thread_id,node_id=excluded.node_id,provider_turn_id=excluded.provider_turn_id,kind=excluded.kind,status=excluded.status,created_at=excluded.created_at,resolved_at=excluded.resolved_at,payload_json=excluded.payload_json",
            &[&id, &tid, &required(v, "nodeId")?, &text(v, "providerTurnId"), &required(v, "kind")?, &required(v, "status")?, &required(v, "createdAt")?, &text(v, "resolvedAt"), &body],
        ),
        "messages" => run(
            "INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,run_id,node_id,role,streaming,created_at,updated_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(message_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,node_id=excluded.node_id,role=excluded.role,streaming=excluded.streaming,created_at=excluded.created_at,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &text(v, "runId"), &text(v, "nodeId"), &required(v, "role")?, &bit(v, "streaming"), &required(v, "createdAt")?, &required(v, "updatedAt")?, &body],
        ),
        "plans" => run(
            "INSERT INTO orchestration_v2_projection_plans(plan_id,thread_id,run_id,node_id,kind,status,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(plan_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,node_id=excluded.node_id,kind=excluded.kind,status=excluded.status,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &text(v, "runId"), &required(v, "nodeId")?, &required(v, "kind")?, &required(v, "status")?, &body],
        ),
        "turnItems" => {
            let ordinal = int(v, "ordinal").ok_or("ordinal missing")?;
            let thread = required(v, "threadId")?;
            run(
                "INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,run_id,node_id,provider_thread_id,provider_turn_id,parent_item_id,ordinal,type,status,updated_at,payload_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(turn_item_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,node_id=excluded.node_id,provider_thread_id=excluded.provider_thread_id,provider_turn_id=excluded.provider_turn_id,parent_item_id=excluded.parent_item_id,ordinal=excluded.ordinal,type=excluded.type,status=excluded.status,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
                &[&id, &thread, &text(v, "runId"), &text(v, "nodeId"), &text(v, "providerThreadId"), &text(v, "providerTurnId"), &text(v, "parentItemId"), &ordinal, &required(v, "type")?, &required(v, "status")?, &required(v, "updatedAt")?, &body],
            )?;
            // Positions are unique per thread; a taken ordinal keeps its first owner, as T3 does.
            run(
                "INSERT OR IGNORE INTO orchestration_v2_turn_item_positions(thread_id,turn_item_id,ordinal) VALUES(?1,?2,?3)",
                &[&thread, &id, &ordinal],
            )?;
            run(
                "UPDATE OR IGNORE orchestration_v2_turn_item_positions SET ordinal=?3 WHERE thread_id=?1 AND turn_item_id=?2",
                &[&thread, &id, &ordinal],
            )
        }
        "checkpointScopes" => run(
            "INSERT INTO orchestration_v2_projection_checkpoint_scopes(scope_id,thread_id,run_id,node_id,provider_thread_id,kind,ordinal_within_parent,advances_app_run_count,created_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(scope_id) DO UPDATE SET thread_id=excluded.thread_id,run_id=excluded.run_id,node_id=excluded.node_id,provider_thread_id=excluded.provider_thread_id,kind=excluded.kind,ordinal_within_parent=excluded.ordinal_within_parent,advances_app_run_count=excluded.advances_app_run_count,created_at=excluded.created_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &text(v, "runId"), &required(v, "nodeId")?, &text(v, "providerThreadId"), &required(v, "kind")?, &int(v, "ordinalWithinParent").ok_or("ordinalWithinParent missing")?, &bit(v, "advancesAppRunCount"), &required(v, "createdAt")?, &body],
        ),
        "checkpoints" => run(
            "INSERT INTO orchestration_v2_projection_checkpoints(checkpoint_id,thread_id,scope_id,run_id,node_id,ordinal_within_scope,app_run_ordinal,status,captured_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(checkpoint_id) DO UPDATE SET thread_id=excluded.thread_id,scope_id=excluded.scope_id,run_id=excluded.run_id,node_id=excluded.node_id,ordinal_within_scope=excluded.ordinal_within_scope,app_run_ordinal=excluded.app_run_ordinal,status=excluded.status,captured_at=excluded.captured_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &required(v, "scopeId")?, &text(v, "runId"), &required(v, "nodeId")?, &int(v, "ordinalWithinScope").ok_or("ordinalWithinScope missing")?, &int(v, "appRunOrdinal"), &required(v, "status")?, &required(v, "capturedAt")?, &body],
        ),
        "contextHandoffs" => run(
            "INSERT INTO orchestration_v2_projection_context_handoffs(context_handoff_id,thread_id,target_run_id,to_provider_thread_id,strategy,status,updated_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(context_handoff_id) DO UPDATE SET thread_id=excluded.thread_id,target_run_id=excluded.target_run_id,to_provider_thread_id=excluded.to_provider_thread_id,strategy=excluded.strategy,status=excluded.status,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "threadId")?, &required(v, "targetRunId")?, &required(v, "toProviderThreadId")?, &required(v, "strategy")?, &required(v, "status")?, &required(v, "updatedAt")?, &body],
        ),
        "contextTransfers" => run(
            "INSERT INTO orchestration_v2_projection_context_transfers(context_transfer_id,source_thread_id,target_thread_id,target_run_id,type,status,updated_at,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(context_transfer_id) DO UPDATE SET source_thread_id=excluded.source_thread_id,target_thread_id=excluded.target_thread_id,target_run_id=excluded.target_run_id,type=excluded.type,status=excluded.status,updated_at=excluded.updated_at,payload_json=excluded.payload_json",
            &[&id, &required(v, "sourceThreadId")?, &required(v, "targetThreadId")?, &text(v, "targetRunId"), &required(v, "type")?, &required(v, "status")?, &required(v, "updatedAt")?, &body],
        ),
        other => Err(format!("no T3 projection for {other}")),
    }
}

/// Writes one O3 record to T3 when T3's row differs. Returns the event sequence it appended.
fn export_record(tx: &Transaction, k: &Kind, tid: &str, v: &Value) -> Result<Option<i64>> {
    let id = required(v, "id")?;
    let existing = payload_row(tx, k, &id)?
        .map(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
        .transpose()?;
    if existing.as_ref() == Some(v) {
        return Ok(None);
    }
    if k.kind == "checkpoints" && existing.is_none() {
        // O3 keeps a baseline per run at position 0 of its scope; T3 holds one checkpoint per
        // position, and the state before a run is the previous run's checkpoint.
        let taken: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_checkpoints WHERE scope_id=?1 AND ordinal_within_scope=?2 AND checkpoint_id<>?3)",
                params![text(v, "scopeId"), int(v, "ordinalWithinScope"), id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if taken {
            return Ok(None);
        }
    }
    let stream = if k.kind == "thread" { id.as_str() } else { tid };
    if stream.is_empty() {
        return Err("record has no thread".into());
    }
    let created = existing.is_none();
    let event_type = if k.kind == "thread" {
        let anchored: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread' AND event_type='thread.created' AND stream_id=?)",
                [&id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        thread_event_type(existing.as_ref(), v, !anchored)
    } else if created {
        k.created.unwrap_or(k.updated)
    } else {
        k.updated
    };
    let at = occurred_at(v);
    let sequence = append_thread_event(tx, stream, event_type, v, &at)?;
    project_row(tx, k, stream, v, existing.as_ref(), created)?;
    if k.kind != "thread" {
        // T3 stamps the thread with every non-thread event; never move it backwards.
        tx.execute(
            "UPDATE orchestration_v2_projection_threads SET updated_at=max(updated_at,?1),payload_json=json_set(payload_json,'$.updatedAt',max(updated_at,?1)) WHERE thread_id=?2",
            params![at, stream],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(Some(sequence))
}

/// T3's `projection_projects` row as the O3 project record it corresponds to.
fn project_from_row(t3: &Connection, id: &str) -> Result<Option<Value>> {
    t3.query_row(
        "SELECT title,workspace_root,default_model_selection_json,default_thread_env_mode,auto_pull,favicon_path,project_icon_json,scripts_json,created_at,updated_at,deleted_at FROM projection_projects WHERE project_id=?",
        [id],
        |r| {
            let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
            Ok(json!({
                "id": id,
                "title": r.get::<_, String>(0)?,
                "workspaceRoot": r.get::<_, String>(1)?,
                "defaultModelSelection": parse(r.get(2)?),
                "defaultThreadEnvMode": r.get::<_, Option<String>>(3)?,
                "autoPull": r.get::<_, i64>(4)? != 0,
                "faviconPath": r.get::<_, Option<String>>(5)?,
                "projectIcon": parse(r.get(6)?),
                "scripts": parse(r.get(7)?),
                "createdAt": r.get::<_, String>(8)?,
                "updatedAt": r.get::<_, String>(9)?,
                "deletedAt": r.get::<_, Option<String>>(10)?,
            }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

const PROJECT_FIELDS: [&str; 11] = [
    "title",
    "workspaceRoot",
    "defaultModelSelection",
    "defaultThreadEnvMode",
    "autoPull",
    "faviconPath",
    "projectIcon",
    "scripts",
    "createdAt",
    "updatedAt",
    "deletedAt",
];

/// An O3 project record normalized to the fields T3 stores (`autoPull` a boolean, absent = null).
fn project_view(v: &Value) -> Value {
    let mut out = json!({"id": v["id"]});
    for field in PROJECT_FIELDS {
        out[field] = v.get(field).cloned().unwrap_or(Value::Null);
    }
    out["autoPull"] = json!(match &v["autoPull"] {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64() != Some(0),
        _ => false,
    });
    if out["scripts"].is_null() {
        out["scripts"] = json!([]);
    }
    out
}

fn export_project(tx: &Transaction, v: &Value) -> Result<Option<i64>> {
    let id = required(v, "id")?;
    let view = project_view(v);
    let existing = project_from_row(tx, &id)?;
    if existing.as_ref() == Some(&view) {
        return Ok(None);
    }
    let at = text(&view, "updatedAt").unwrap_or_else(|| iso(now_ms()));
    let mut fields = Map::new();
    fields.insert("projectId".into(), json!(id));
    for field in [
        "title",
        "workspaceRoot",
        "defaultModelSelection",
        "defaultThreadEnvMode",
        "faviconPath",
        "projectIcon",
        "scripts",
        "updatedAt",
    ] {
        fields.insert(field.into(), view[field].clone());
    }
    let mut sequence;
    if existing.is_none() {
        let mut created = fields.clone();
        created.insert("createdAt".into(), view["createdAt"].clone());
        sequence = Some(append_project_event(
            tx,
            &id,
            "project.created",
            &Value::Object(created),
            &at,
        )?);
    } else {
        let mut updated = fields.clone();
        updated.insert("autoPull".into(), view["autoPull"].clone());
        sequence = Some(append_project_event(
            tx,
            &id,
            "project.meta-updated",
            &Value::Object(updated),
            &at,
        )?);
    }
    if let Some(deleted) = text(&view, "deletedAt") {
        if existing
            .as_ref()
            .is_none_or(|old| old["deletedAt"].is_null())
        {
            sequence = Some(append_project_event(
                tx,
                &id,
                "project.deleted",
                &json!({"projectId": id, "deletedAt": deleted}),
                &deleted,
            )?);
        }
    }
    let encode = |field: &str| (!view[field].is_null()).then(|| view[field].to_string());
    tx.execute(
        "INSERT INTO projection_projects(project_id,title,workspace_root,default_model_selection_json,default_thread_env_mode,auto_pull,favicon_path,project_icon_json,scripts_json,created_at,updated_at,deleted_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(project_id) DO UPDATE SET title=excluded.title,workspace_root=excluded.workspace_root,default_model_selection_json=excluded.default_model_selection_json,default_thread_env_mode=excluded.default_thread_env_mode,auto_pull=excluded.auto_pull,favicon_path=excluded.favicon_path,project_icon_json=excluded.project_icon_json,scripts_json=excluded.scripts_json,created_at=excluded.created_at,updated_at=excluded.updated_at,deleted_at=excluded.deleted_at",
        params![id, required(&view, "title")?, required(&view, "workspaceRoot")?, encode("defaultModelSelection"), text(&view, "defaultThreadEnvMode"), bit(&view, "autoPull"), text(&view, "faviconPath"), encode("projectIcon"), view["scripts"].to_string(), required(&view, "createdAt")?, required(&view, "updatedAt")?, text(&view, "deletedAt")],
    )
    .map_err(|e| e.to_string())?;
    Ok(sequence)
}

/// Values O3 uses that T3's schema has no room for, mapped to T3's nearest: a session O3 calls
/// `disconnected` is `stopped`, a bare cost is an amount in USD, a null message context is absent,
/// and a runtime mode T3 lacks asks for approval.
fn shape_values(kind_name: &str, v: &mut Value) {
    const SESSION: [&str; 6] = [
        "starting", "ready", "running", "waiting", "stopped", "error",
    ];
    const RUNTIME: [&str; 4] = [
        "approval-required",
        "auto-accept-edits",
        "auto",
        "full-access",
    ];
    match kind_name {
        "providerSessions" => {
            if !v["status"].as_str().is_some_and(|s| SESSION.contains(&s)) {
                v["status"] = json!("stopped");
            }
        }
        "messages" => {
            if v.get("context").is_some_and(Value::is_null) {
                v.as_object_mut().map(|o| o.remove("context"));
            }
        }
        "thread"
            if !v["runtimeMode"]
                .as_str()
                .is_some_and(|s| RUNTIME.contains(&s)) =>
        {
            v["runtimeMode"] = json!("approval-required");
        }
        _ => {}
    }
    // T3 marks only awaited background work; `awaited: false` is spelled by leaving it out.
    if let Some(tasks) = v
        .get_mut("pendingBackgroundTasks")
        .and_then(Value::as_array_mut)
    {
        for task in tasks.iter_mut().filter_map(Value::as_object_mut) {
            if task.get("awaited").is_some_and(|a| a != &Value::Bool(true)) {
                task.remove("awaited");
            }
        }
    }
    if let Some(usage) = v.get_mut("contextUsage").and_then(Value::as_object_mut) {
        match usage.get("cost") {
            Some(Value::Number(n)) => {
                let amount = n.as_f64().unwrap_or(0.0);
                usage.insert("cost".into(), json!({"amount": amount, "currency": "USD"}));
            }
            Some(Value::Null) => {
                usage.remove("cost");
            }
            _ => {}
        }
    }
}

/// The reverse for fields O3 reads in its own form: a T3 cost object back to O3's USD amount.
fn o3_shape(v: &mut Value) {
    if let Some(usage) = v.get_mut("contextUsage").and_then(Value::as_object_mut) {
        if let Some(amount) = usage.get("cost").and_then(|c| c.get("amount")).cloned() {
            usage.insert("cost".into(), amount);
        }
    }
}

fn iso_ms(text: &str) -> Option<i64> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|at| (at.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// T3's per-client columns, which O3 keeps together in its `client` JSON.
const CLIENT_COLUMNS: [&str; 8] = [
    "client_label",
    "client_ip_address",
    "client_user_agent",
    "client_device_type",
    "client_os",
    "client_browser",
    "client_surface",
    "client_app_version",
];

fn o3_session(db: &Connection, id: &str) -> Result<Option<Value>> {
    db.query_row(
        "SELECT subject,scopes,method,client,issued_at,expires_at,revoked_at FROM auth_sessions WHERE session_id=?",
        [id],
        |r| {
            Ok(json!({
                "sessionId": id,
                "subject": r.get::<_, String>(0)?,
                "scopes": r.get::<_, String>(1)?,
                "method": r.get::<_, String>(2)?,
                "client": r.get::<_, String>(3)?,
                "issuedAt": r.get::<_, i64>(4)?,
                "expiresAt": r.get::<_, i64>(5)?,
                "revokedAt": r.get::<_, Option<i64>>(6)?,
            }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Writes an O3 session into T3's `auth_sessions`: scopes as T3's JSON array, times as ISO.
fn export_session(tx: &Transaction, v: &Value) -> Result<bool> {
    let id = required(v, "sessionId")?;
    let expires = iso(v["expiresAt"].as_i64().ok_or("expiresAt missing")?);
    let revoked = v["revokedAt"].as_i64().map(iso);
    let existing: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT expires_at,revoked_at FROM auth_sessions WHERE session_id=?",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let same = |at: &str, ours: &str| iso_ms(at) == iso_ms(ours);
    if existing.as_ref().is_some_and(|(e, r)| {
        same(e, &expires)
            && match (r, &revoked) {
                (None, None) => true,
                (Some(a), Some(b)) => same(a, b),
                _ => false,
            }
    }) {
        return Ok(false);
    }
    let scopes: Vec<&str> = v["scopes"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let client: Value =
        serde_json::from_str(v["client"].as_str().unwrap_or("{}")).unwrap_or_default();
    let column = |name: &str| client.get(name).and_then(Value::as_str).map(str::to_owned);
    tx.execute(
        "INSERT INTO auth_sessions(session_id,subject,scopes,method,client_label,client_ip_address,client_user_agent,client_device_type,client_os,client_browser,client_surface,client_app_version,issued_at,expires_at,revoked_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(session_id) DO UPDATE SET expires_at=excluded.expires_at,revoked_at=excluded.revoked_at",
        params![
            id,
            required(v, "subject")?,
            json!(scopes).to_string(),
            required(v, "method")?,
            column("client_label"),
            column("client_ip_address"),
            column("client_user_agent"),
            column("client_device_type").unwrap_or_else(|| "unknown".into()),
            column("client_os"),
            column("client_browser"),
            column("client_surface"),
            column("client_app_version"),
            iso(v["issuedAt"].as_i64().unwrap_or_else(now_ms)),
            expires,
            revoked,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// A T3 session row: id, subject, scopes, method, client, issued, expires, revoked.
type T3Session = (
    String,
    String,
    String,
    String,
    Value,
    String,
    String,
    Option<String>,
);

/// Takes in the live sessions T3 issued or revoked, so its paired clients reach O3 too.
fn take_sessions(s: &State, t3: &Connection) -> Result<usize> {
    let now = iso(now_ms());
    let rows: Vec<T3Session> = {
        let mut q = t3
            .prepare(&format!(
                "SELECT session_id,subject,scopes,method,{},issued_at,expires_at,revoked_at FROM auth_sessions WHERE expires_at>?",
                CLIENT_COLUMNS.join(",")
            ))
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map([&now], |r| {
                let mut client = Map::new();
                for (i, name) in CLIENT_COLUMNS.iter().enumerate() {
                    if let Some(value) = r.get::<_, Option<String>>(4 + i)? {
                        client.insert((*name).into(), json!(value));
                    }
                }
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    Value::Object(client),
                    r.get(12)?,
                    r.get(13)?,
                    r.get(14)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut taken = 0;
    for (id, subject, scopes, method, client, issued, expires, revoked) in rows {
        let scopes = serde_json::from_str::<Vec<String>>(&scopes)
            .map(|s| s.join(" "))
            .unwrap_or(scopes);
        let (Some(issued), Some(expires)) = (iso_ms(&issued), iso_ms(&expires)) else {
            continue;
        };
        let revoked = revoked.as_deref().and_then(iso_ms);
        let changed = tx
            .execute(
                "INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at,revoked_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(session_id) DO UPDATE SET revoked_at=coalesce(auth_sessions.revoked_at,excluded.revoked_at)
                 WHERE auth_sessions.revoked_at IS NULL AND excluded.revoked_at IS NOT NULL",
                params![id, subject, scopes, method, client.to_string(), issued, expires, revoked],
            )
            .map_err(|e| e.to_string())?;
        if changed > 0 {
            taken += 1;
            tx.execute(
                "DELETE FROM t3_dirty WHERE kind='authSession' AND id=?",
                [&id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(taken)
}

struct Dirty {
    kind: String,
    id: String,
    thread_id: String,
    version: i64,
    body: Option<Value>,
}

/// O3's own checkpoints predate the shared base and lack fields T3's schema requires. Fill them from
/// the run and thread they belong to; fields O3 already has are never changed.
fn t3_shape(db: &Connection, kind_name: &str, mut v: Value) -> Value {
    if !v.is_object() {
        return v;
    }
    shape_values(kind_name, &mut v);
    if !matches!(kind_name, "checkpoints" | "checkpointScopes") {
        return v;
    }
    let run = v["runId"]
        .as_str()
        .and_then(|id| events::get(db, "runs", id).ok());
    let thread = v["threadId"]
        .as_str()
        .and_then(|id| events::get(db, "thread", id).ok());
    let id = v["id"].as_str().unwrap_or_default().to_owned();
    let fill = |v: &mut Value, field: &str, value: Value| {
        if v.get(field).is_none_or(Value::is_null) {
            v[field] = value;
        }
    };
    let node = run
        .as_ref()
        .and_then(|r| r["rootNodeId"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("node:o3:{id}"));
    if v.get("nodeId").is_none_or(Value::is_null) {
        v["nodeId"] = json!(node);
    }
    let when = |r: Option<&Value>, fields: &[&str]| {
        r.and_then(|r| {
            fields
                .iter()
                .find_map(|f| r[*f].as_str().map(str::to_owned))
        })
    };
    if kind_name == "checkpoints" {
        let at = when(run.as_ref(), &["completedAt", "requestedAt"])
            .or_else(|| when(thread.as_ref(), &["updatedAt"]))
            .unwrap_or_else(|| iso(now_ms()));
        fill(&mut v, "capturedAt", json!(at));
        fill(&mut v, "parentCheckpointId", Value::Null);
        fill(&mut v, "files", json!([]));
        fill(&mut v, "ordinalWithinScope", json!(0));
        if !v["appRunOrdinal"].as_i64().is_some_and(|n| n > 0) {
            v["appRunOrdinal"] = Value::Null;
        }
    } else {
        let at = when(thread.as_ref(), &["createdAt"]).unwrap_or_else(|| iso(now_ms()));
        fill(&mut v, "createdAt", json!(at));
        fill(&mut v, "runId", Value::Null);
        fill(&mut v, "parentScopeId", Value::Null);
        fill(&mut v, "providerThreadId", Value::Null);
        fill(&mut v, "ordinalWithinParent", json!(0));
        let root = v["kind"] == "root_run";
        fill(&mut v, "advancesAppRunCount", json!(root));
    }
    v
}

/// Exports every marked record. Returns how many were written to T3.
pub fn export(s: &State, t3: &mut Connection) -> Result<usize> {
    let mut written = 0;
    let mut after = 0i64;
    loop {
        let batch = {
            let db = s.db.lock().unwrap();
            let mut q = db
                .prepare("SELECT d.rowid,d.kind,d.id,d.thread_id,d.version,r.body FROM t3_dirty d LEFT JOIN records r ON r.kind=d.kind AND r.id=d.id WHERE d.rowid>? ORDER BY d.rowid LIMIT ?")
                .map_err(|e| e.to_string())?;
            let mut rows = q
                .query(params![after, BATCH_ROWS as i64])
                .map_err(|e| e.to_string())?;
            let (mut raw, mut bytes) = (Vec::new(), 0usize);
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                after = row.get(0).map_err(|e| e.to_string())?;
                let body: Option<String> = row.get(5).map_err(|e| e.to_string())?;
                bytes += body.as_ref().map_or(0, String::len);
                raw.push((
                    row.get::<_, String>(1).map_err(|e| e.to_string())?,
                    row.get::<_, String>(2).map_err(|e| e.to_string())?,
                    row.get::<_, String>(3).map_err(|e| e.to_string())?,
                    row.get::<_, i64>(4).map_err(|e| e.to_string())?,
                    body,
                ));
                if bytes > BATCH_BYTES {
                    break;
                }
            }
            drop(rows);
            drop(q);
            let mut batch = Vec::with_capacity(raw.len());
            for (kind, id, thread_id, version, body) in raw {
                if kind == "authSession" {
                    let body = o3_session(&db, &id)?;
                    batch.push(Dirty {
                        kind,
                        id,
                        thread_id,
                        version,
                        body,
                    });
                    continue;
                }
                let body = match body.map(|b| serde_json::from_str::<Value>(&b)) {
                    Some(Ok(v)) => Some(t3_shape(&db, &kind, v)),
                    Some(Err(e)) => {
                        eprintln!("T3 export skipped unreadable {kind} {id}: {e}");
                        None
                    }
                    None => None,
                };
                batch.push(Dirty {
                    kind,
                    id,
                    thread_id,
                    version,
                    body,
                });
            }
            batch
        };
        if batch.is_empty() {
            return Ok(written);
        }
        let mut rejects = Vec::new();
        let tx = t3
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut last = None;
        for item in &batch {
            // A deleted record has no T3 counterpart to remove: T3 never deletes projection rows.
            let Some(v) = &item.body else { continue };
            let result = (|| -> Result<Option<Option<i64>>> {
                tx.execute_batch("SAVEPOINT o3_record")
                    .map_err(|e| e.to_string())?;
                // Project events do not move the thread projections' high-water mark.
                let result = if item.kind == "project" {
                    export_project(&tx, v).map(|written| written.map(|_| None))
                } else if item.kind == "authSession" {
                    export_session(&tx, v).map(|written| written.then_some(None))
                } else {
                    let k = kind(&item.kind).ok_or("unknown kind")?;
                    export_record(&tx, k, &item.thread_id, v).map(|written| written.map(Some))
                };
                let end = if result.is_ok() {
                    "RELEASE o3_record"
                } else {
                    "ROLLBACK TO o3_record; RELEASE o3_record"
                };
                tx.execute_batch(end).map_err(|e| e.to_string())?;
                result
            })();
            match result {
                Ok(Some(sequence)) => {
                    last = sequence.or(last);
                    written += 1;
                }
                Ok(None) => {}
                Err(error) => rejects.push((item.kind.clone(), item.id.clone(), error)),
            }
        }
        if let Some(sequence) = last {
            tx.execute(
                "INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('thread-projections',?1,?2,?3)
                 ON CONFLICT(projection_name) DO UPDATE SET schema_version=excluded.schema_version,last_sequence=excluded.last_sequence,updated_at=excluded.updated_at",
                params![PROJECTION_SCHEMA_VERSION, sequence, iso(now_ms())],
            )
            .map_err(|e| e.to_string())?;
        }
        set_seen(&tx, latest(&tx)?)?;
        tx.commit().map_err(|e| e.to_string())?;
        let db = s.db.lock().unwrap();
        for item in &batch {
            // A record rewritten since it was read stays marked for the next pass.
            db.execute(
                "DELETE FROM t3_dirty WHERE kind=? AND id=? AND version=?",
                params![item.kind, item.id, item.version],
            )
            .map_err(|e| e.to_string())?;
        }
        for (kind, id, error) in rejects {
            eprintln!("T3 export skipped {kind} {id}: {error}");
            db.execute(
                "INSERT OR REPLACE INTO t3_rejects VALUES(?,?,?,?)",
                params![kind, id, error, now_ms()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
}

/// A record just taken from T3 is T3's state already; the fields only O3 keeps were dropped by
/// T3's encoder on purpose, so writing it back would only churn events.
fn taken_from_t3(db: &Connection, kind_name: &str, id: &str) -> Result<()> {
    db.execute(
        "DELETE FROM t3_dirty WHERE kind=? AND id=?",
        params![kind_name, id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Puts a T3 row into O3 the way O3's own producers do, and tells subscribed clients.
fn take_in(db: &Connection, kind_name: &str, tid: &str, v: &Value) -> Result<bool> {
    let id = required(v, "id")?;
    let mut v = v.clone();
    if kind_name == "thread" {
        events::normalize_settled_override(&mut v);
    }
    o3_shape(&mut v);
    let current = events::get(db, kind_name, &id).ok();
    // T3's row wins field by field; fields only O3 keeps (run bookkeeping, checkpoint identity)
    // stay.
    if let (Some(Value::Object(old)), Value::Object(new)) = (&current, &v) {
        let mut merged = old.clone();
        merged.extend(new.clone());
        v = Value::Object(merged);
    }
    if current.as_ref() == Some(&v) {
        return Ok(false);
    }
    events::put(db, kind_name, &id, tid, &v)?;
    taken_from_t3(db, kind_name, &id)?;
    if kind_name == "turnItems" {
        let mut visible = events::get(db, "visibleTurnItems", &id).unwrap_or_else(|_| {
            json!({"position":v["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id})
        });
        visible["item"] = v.clone();
        events::put(db, "visibleTurnItems", &id, tid, &visible)?;
    }
    let k = kind(kind_name).ok_or("unknown kind")?;
    let thread = if kind_name == "thread" {
        v.clone()
    } else {
        events::get(db, "thread", tid)?
    };
    let event_type = if kind_name == "thread" {
        thread_event_type(current.as_ref(), &v, current.is_none())
    } else if current.is_none() {
        k.created.unwrap_or(k.updated)
    } else {
        k.updated
    };
    events::append(db, tid, event_type, v, &thread)?;
    Ok(true)
}

fn take_in_project(db: &Connection, v: &Value) -> Result<bool> {
    let id = required(v, "id")?;
    let current = events::get(db, "project", &id).ok();
    let mut merged = current.clone().unwrap_or_else(|| json!({"id": id}));
    for field in PROJECT_FIELDS {
        merged[field] = v[field].clone();
    }
    if current.as_ref().map(project_view) == Some(project_view(&merged)) {
        return Ok(false);
    }
    events::put(db, "project", &id, "", &merged)?;
    taken_from_t3(db, "project", &id)?;
    let body = json!({"id":random_id()?,"type":"project.update","payload":merged,"occurredAt":merged["updatedAt"]});
    db.execute(
        "INSERT INTO events(thread_id,body) VALUES('',?)",
        [body.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let sequence = db.last_insert_rowid();
    let shell = if merged["deletedAt"].is_null() {
        json!({"kind":"project.updated","sequence":sequence,"project":merged})
    } else {
        json!({"kind":"project.removed","sequence":sequence,"projectId":id})
    };
    db.execute(
        "UPDATE events SET shell=? WHERE sequence=?",
        params![shell.to_string(), sequence],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Takes in what T3 wrote after `after`: the current T3 row of every record its events touched.
fn catch_up(s: &State, t3: &Connection, after: i64) -> Result<usize> {
    let mut touched: Vec<(String, String, String)> = Vec::new();
    let mut known = std::collections::HashSet::new();
    {
        let mut q = t3
            .prepare("SELECT aggregate_kind,stream_id,event_type,payload_json FROM orchestration_events WHERE sequence>? AND (aggregate_kind='project' OR application_event_version=2) ORDER BY sequence")
            .map_err(|e| e.to_string())?;
        let mut rows = q.query([after]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let aggregate: String = row.get(0).map_err(|e| e.to_string())?;
            let stream: String = row.get(1).map_err(|e| e.to_string())?;
            let mut targets = Vec::new();
            if aggregate == "project" {
                targets.push(("project".to_owned(), stream.clone()));
            } else {
                let event_type: String = row.get(2).map_err(|e| e.to_string())?;
                let payload: Value =
                    serde_json::from_str(&row.get::<_, String>(3).map_err(|e| e.to_string())?)
                        .unwrap_or(Value::Null);
                if let Some((kind, id)) = event_target(&event_type, &payload) {
                    targets.push((kind.to_owned(), id));
                }
                // Every T3 thread event stamps the thread.
                targets.push(("thread".to_owned(), stream.clone()));
            }
            for (kind, id) in targets {
                if known.insert((kind.clone(), id.clone())) {
                    touched.push((kind, id, stream.clone()));
                }
            }
        }
    }
    // Threads first, so every other record finds the thread its client event names.
    touched.sort_by_key(|(kind, _, _)| kind != "project" && kind != "thread");
    let mut taken = 0;
    for chunk in touched.chunks(BATCH_ROWS) {
        let mut rows = Vec::new();
        for (kind_name, id, stream) in chunk {
            let row = if kind_name == "project" {
                project_from_row(t3, id)?
            } else {
                let k = kind(kind_name).ok_or("unknown kind")?;
                payload_row(t3, k, id)?
                    .map(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
                    .transpose()?
            };
            if let Some(row) = row {
                rows.push((kind_name.clone(), stream.clone(), row));
            }
        }
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        for (kind_name, stream, row) in &rows {
            let tid = if kind_name == "thread" {
                row["id"].as_str().unwrap_or(stream)
            } else {
                stream
            };
            let changed = if kind_name == "project" {
                take_in_project(&tx, row)?
            } else {
                take_in(&tx, kind_name, tid, row)?
            };
            taken += usize::from(changed);
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(taken)
}

/// One numbered record: ordinal, id, time, whether T3 has it, and its thread.
type Numbered = (i64, String, String, bool, String);

/// Before sharing T3's base, O3 numbered runs (and provider turns) without seeing T3's: its first
/// import carried no runs. Where an O3-only record takes an ordinal T3 already uses in the same
/// group, every O3-only record of that group moves after T3's last ordinal, in time order; where
/// only O3's own records share an ordinal, the later ones move to the end. Turn items keep their
/// ordinals: O3 placed them after T3's already. Returns how many moved.
fn renumber_forks(
    tx: &Transaction,
    t3: &Connection,
    kind_name: &str,
    group: &str,
    time: &str,
) -> Result<usize> {
    let k = kind(kind_name).ok_or("unknown kind")?;
    let mut groups: std::collections::BTreeMap<String, Vec<Numbered>> = Default::default();
    {
        let mut q = tx
            .prepare("SELECT id,thread_id,body FROM records WHERE kind=?")
            .map_err(|e| e.to_string())?;
        let mut rows = q.query([kind_name]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let tid: String = row.get(1).map_err(|e| e.to_string())?;
            let body: Value =
                serde_json::from_str(&row.get::<_, String>(2).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let (Some(key), Some(ordinal)) = (text(&body, group), int(&body, "ordinal")) else {
                continue;
            };
            let shared = payload_row(t3, k, &id)?.is_some();
            let at = text(&body, time).unwrap_or_default();
            groups
                .entry(key)
                .or_default()
                .push((ordinal, id, at, shared, tid));
        }
    }
    let mut moved = 0;
    for records in groups.values() {
        let taken: std::collections::HashSet<i64> =
            records.iter().filter(|r| r.3).map(|r| r.0).collect();
        let mut own: Vec<_> = records.iter().filter(|r| !r.3).collect();
        own.sort_by(|a, b| (&a.2, a.0).cmp(&(&b.2, b.0)));
        let (first, own) = if own.iter().any(|r| taken.contains(&r.0)) {
            // Numbered without seeing T3's: all of O3's move after T3's last.
            (taken.iter().max().copied().unwrap_or(0) + 1, own)
        } else {
            // Only O3's own duplicates: the earliest keeps its ordinal, later ones move to the end.
            let mut seen = std::collections::HashSet::new();
            let later: Vec<_> = own.iter().copied().filter(|r| !seen.insert(r.0)).collect();
            if later.is_empty() {
                continue;
            }
            (records.iter().map(|r| r.0).max().unwrap_or(0) + 1, later)
        };
        for (next, (old, id, _, _, tid)) in (first..).zip(own) {
            let mut body = events::get(tx, kind_name, id)?;
            body["ordinal"] = json!(next);
            events::put(tx, kind_name, id, tid, &body)?;
            if kind_name == "runs" {
                // A checkpoint names its run's ordinal.
                let mut q = tx
                    .prepare("SELECT id,thread_id,body FROM records WHERE kind='checkpoints' AND json_extract(body,'$.runId')=?")
                    .map_err(|e| e.to_string())?;
                let checkpoints: Vec<(String, String, String)> = q
                    .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .map_err(|e| e.to_string())?
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                for (cid, ctid, cbody) in checkpoints {
                    let mut cp: Value = serde_json::from_str(&cbody).map_err(|e| e.to_string())?;
                    if cp["appRunOrdinal"] == json!(old) {
                        cp["appRunOrdinal"] = json!(next);
                        events::put(tx, "checkpoints", &cid, &ctid, &cp)?;
                    }
                }
            }
            moved += 1;
        }
    }
    Ok(moved)
}

/// First sync on a profile: take in T3's history O3 lacks (O3's own records win), then mark every
/// O3 record so the next export writes whatever T3 lacks or holds differently.
fn adopt(s: &State, t3: &Connection) -> Result<usize> {
    let mut taken = 0;
    for k in KINDS.iter().chain(std::iter::once(&PROJECTS)) {
        let mut cursor = String::new();
        loop {
            let ids: Vec<(String, Option<String>)> = {
                let thread_col = match k.kind {
                    "thread" => "thread_id",
                    "project" => "''",
                    "contextTransfers" => "target_thread_id",
                    _ => "thread_id",
                };
                let mut q = t3
                    .prepare(&format!(
                        "SELECT {id},{thread_col} FROM {table} WHERE {id}>? ORDER BY {id} LIMIT ?",
                        id = k.id,
                        table = k.table
                    ))
                    .map_err(|e| e.to_string())?;
                let rows = q
                    .query_map(params![cursor, BATCH_ROWS as i64], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .map_err(|e| e.to_string())?;
                rows.collect::<std::result::Result<_, _>>()
                    .map_err(|e| e.to_string())?
            };
            let Some((last, _)) = ids.last() else { break };
            cursor = last.clone();
            let mut db = s.db.lock().unwrap();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            for (id, tid) in &ids {
                if events::get(&tx, k.kind, id).is_ok() {
                    continue;
                }
                let row = if k.kind == "project" {
                    project_from_row(t3, id)?
                } else {
                    payload_row(t3, k, id)?
                        .map(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
                        .transpose()?
                };
                let Some(mut row) = row else { continue };
                if k.kind == "project" {
                    events::put(&tx, "project", id, "", &row)?;
                } else {
                    if k.kind == "thread" {
                        events::normalize_settled_override(&mut row);
                    }
                    o3_shape(&mut row);
                    let tid = tid.clone().unwrap_or_default();
                    events::put(&tx, k.kind, id, &tid, &row)?;
                    if k.kind == "turnItems" {
                        let visible = json!({"position":row["ordinal"],"sourceItemId":id,"sourceThreadId":tid,"visibility":"local","item":row});
                        events::put(&tx, "visibleTurnItems", id, &tid, &visible)?;
                    }
                }
                taken += 1;
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
    }
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let renumbered = renumber_forks(&tx, t3, "runs", "threadId", "requestedAt")?
        + renumber_forks(&tx, t3, "providerTurns", "providerThreadId", "startedAt")?;
    if renumbered > 0 {
        eprintln!("T3 shared base: renumbered {renumbered} O3 records that collided with T3's");
    }
    tx.commit().map_err(|e| e.to_string())?;
    db.execute(
        &format!("INSERT OR IGNORE INTO t3_dirty SELECT kind,id,thread_id,0 FROM records WHERE kind IN ({})", kinds_sql()),
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(taken)
}

/// Brings O3 up to date with T3's base and returns the open connection for the exporter, which
/// writes the backlog in the background.
pub fn start(s: &State) -> Result<Option<Connection>> {
    let Some(t3) = open(&s.dir)? else {
        return Ok(None);
    };
    let latest_sequence = latest(&t3)?;
    let mut report = match seen(&t3)? {
        None => json!({"adopted": adopt(s, &t3)?}),
        Some(after) if after < latest_sequence => json!({"tookIn": catch_up(s, &t3, after)?}),
        Some(_) => json!({}),
    };
    report["sessions"] = json!(take_sessions(s, &t3)?);
    let shaped: i64 = t3
        .query_row("SELECT value FROM o3_sync WHERE name=?", [SHAPED], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(1);
    if shaped < SHAPE_VERSION {
        let marked = s
            .db
            .lock()
            .unwrap()
            .execute(
                &format!("INSERT OR IGNORE INTO t3_dirty SELECT kind,id,thread_id,0 FROM records WHERE kind IN ({})", kinds_sql()),
                [],
            )
            .map_err(|e| e.to_string())?;
        report["reshaped"] = json!(marked);
        t3.execute(
            "INSERT INTO o3_sync VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",
            params![SHAPED, SHAPE_VERSION],
        )
        .map_err(|e| e.to_string())?;
    }
    // Sessions are few; the exporter skips the ones T3 already holds as O3 does.
    s.db.lock()
        .unwrap()
        .execute(
            "INSERT OR IGNORE INTO t3_dirty SELECT 'authSession',session_id,'',0 FROM auth_sessions",
            [],
        )
        .map_err(|e| e.to_string())?;
    set_seen(&t3, latest_sequence)?;
    s.notify();
    eprintln!("T3 shared base {}: {report}", database(&s.dir).display());
    Ok(Some(t3))
}

/// The exporter: writes marked records to T3 shortly after they change, and once more on stop.
pub struct Exporter {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Exporter {
    pub fn spawn(s: Arc<State>, mut t3: Connection) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let mut changes = s.changed.subscribe();
        let worker = std::thread::Builder::new()
            .name("t3-export".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut first = true;
                loop {
                    let stopping = flag.load(Ordering::Acquire);
                    if first || stopping || changes.has_changed().unwrap_or(true) {
                        first = false;
                        changes.borrow_and_update();
                        if let Err(e) = export(&s, &mut t3) {
                            eprintln!("T3 export failed: {e}");
                        }
                    }
                    if stopping {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            })
            .expect("spawn t3 exporter");
        Self {
            stop,
            worker: Some(worker),
        }
    }

    /// Stops after a last export, so T3 started next finds everything O3 did.
    pub fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
