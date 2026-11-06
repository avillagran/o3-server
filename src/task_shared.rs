//! Task handover has no orchestration events. A native-side baseline records the last
//! synchronized row hashes, independently of the shared thread-event cursor.
use crate::{events, store::State};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, String>;

struct Table {
    name: &'static str,
    columns: &'static [&'static str],
    key: &'static [&'static str],
}
const TASK_FIELDS: &[(&str, &str)] = &[
    ("task_id", "id"),
    ("title", "title"),
    ("prompt", "prompt"),
    ("enabled", "enabled"),
    ("schedule_json", "schedule"),
    ("project_id", "projectId"),
    ("thread_id", "threadId"),
    ("workspace_strategy_json", "workspaceStrategy"),
    ("model_selection_json", "modelSelection"),
    ("runtime_mode", "runtimeMode"),
    ("interaction_mode", "interactionMode"),
    ("created_by", "createdBy"),
    ("creation_source", "creationSource"),
    ("created_at", "createdAt"),
    ("updated_at", "updatedAt"),
    ("next_run_at", "nextRunAt"),
    ("last_run_at", "lastRunAt"),
    ("last_run_status", "lastRunStatus"),
    ("last_run_error", "lastRunError"),
    ("run_count", "runCount"),
    ("thread_mode", "threadMode"),
];
const TABLES: &[Table] = &[
    Table {
        name: "scheduled_tasks",
        key: &["task_id"],
        columns: &[
            "task_id",
            "title",
            "prompt",
            "enabled",
            "schedule_json",
            "project_id",
            "thread_id",
            "workspace_strategy_json",
            "model_selection_json",
            "runtime_mode",
            "interaction_mode",
            "created_by",
            "creation_source",
            "created_at",
            "updated_at",
            "next_run_at",
            "last_run_at",
            "last_run_status",
            "last_run_error",
            "run_count",
            "webhook_token",
            "webhook_secret",
            "last_run_webhook_delivery_id",
            "thread_mode",
        ],
    },
    Table {
        name: "scheduled_task_webhook_deliveries",
        key: &["delivery_id"],
        columns: &[
            "delivery_id",
            "task_id",
            "received_at",
            "method",
            "query",
            "headers_json",
            "body",
            "body_bytes",
            "body_truncated",
            "outcome",
            "signature_verified",
            "missing_fields_json",
            "rendered_prompt",
            "error",
        ],
    },
    Table {
        name: "scheduled_task_webhook_pending",
        key: &["delivery_id"],
        columns: &[
            "delivery_id",
            "task_id",
            "coalesce_key",
            "due_at",
            "state",
            "task_json",
            "request_json",
            "delivery_ids_json",
        ],
    },
    Table {
        name: "scheduled_task_webhook_replay_keys",
        key: &["task_id", "replay_key"],
        columns: &["task_id", "replay_key", "delivery_id", "expires_at"],
    },
    Table {
        name: "scheduled_task_webhook_relay_deliveries",
        key: &["relay_delivery_id"],
        columns: &["relay_delivery_id", "task_id", "seen_at"],
    },
];

/// The local ledger retains native payloads verbatim in meaning. Task definitions remain in
/// `records`; only the native database owns its task foreign keys. Local task deletion explicitly
/// clears the ledger in the same transaction as the definition.
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS scheduled_task_webhook_deliveries(
          delivery_id TEXT PRIMARY KEY,task_id TEXT NOT NULL,received_at TEXT NOT NULL,
          method TEXT NOT NULL,query TEXT NOT NULL,headers_json TEXT NOT NULL,body TEXT NOT NULL,
          body_bytes INTEGER NOT NULL,body_truncated INTEGER NOT NULL,outcome TEXT NOT NULL,
          signature_verified INTEGER NOT NULL,missing_fields_json TEXT NOT NULL,rendered_prompt TEXT,error TEXT);
         CREATE INDEX IF NOT EXISTS o3_webhook_deliveries_task ON scheduled_task_webhook_deliveries(task_id,received_at);
         CREATE TABLE IF NOT EXISTS scheduled_task_webhook_pending(
          delivery_id TEXT PRIMARY KEY,task_id TEXT NOT NULL,coalesce_key TEXT NOT NULL,due_at INTEGER NOT NULL,
          state TEXT NOT NULL DEFAULT 'pending',task_json TEXT NOT NULL,request_json TEXT NOT NULL,delivery_ids_json TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS o3_webhook_pending_due ON scheduled_task_webhook_pending(state,due_at);
         CREATE UNIQUE INDEX IF NOT EXISTS o3_webhook_pending_key ON scheduled_task_webhook_pending(task_id,coalesce_key) WHERE state='pending';
         CREATE TABLE IF NOT EXISTS scheduled_task_webhook_replay_keys(
          task_id TEXT NOT NULL,replay_key TEXT NOT NULL,delivery_id TEXT NOT NULL,expires_at INTEGER NOT NULL,
          PRIMARY KEY(task_id,replay_key));
         CREATE INDEX IF NOT EXISTS o3_webhook_replay_expiry ON scheduled_task_webhook_replay_keys(expires_at);
         CREATE INDEX IF NOT EXISTS o3_webhook_replay_delivery ON scheduled_task_webhook_replay_keys(delivery_id);
         CREATE TABLE IF NOT EXISTS scheduled_task_webhook_relay_deliveries(
          relay_delivery_id TEXT PRIMARY KEY,task_id TEXT NOT NULL,seen_at TEXT NOT NULL);",
    ).map_err(|e|e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS o3_task_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1),identity TEXT NOT NULL,generation INTEGER NOT NULL);").map_err(|e|e.to_string())?;
    db.execute(
        "INSERT OR IGNORE INTO o3_task_state VALUES(1,?1,0)",
        [crate::store::random_id()?],
    )
    .map_err(|e| e.to_string())?;
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let reference = if operation == "DELETE" { "old" } else { "new" };
        let trigger = format!("o3_task_records_{}", operation.to_lowercase());
        db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {trigger} AFTER {operation} ON records WHEN {reference}.kind IN ('scheduledTasks','hookCredentials','scheduledTaskNative') BEGIN UPDATE o3_task_state SET generation=generation+1 WHERE singleton=1; END;")).map_err(|e|e.to_string())?;
        for table in &TABLES[1..] {
            let trigger = format!("o3_task_{}_{}", table.name, operation.to_lowercase());
            db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {trigger} AFTER {operation} ON {} BEGIN UPDATE o3_task_state SET generation=generation+1 WHERE singleton=1; END;",table.name)).map_err(|e|e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn delete_ledger(db: &Connection, id: &str) -> Result<()> {
    for table in &TABLES[1..] {
        db.execute(
            &format!("DELETE FROM {} WHERE task_id=?1", table.name),
            [id],
        )
        .map_err(|e| e.to_string())?;
    }
    db.execute(
        "DELETE FROM records WHERE kind='scheduledTaskNative' AND id=?1",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn validate(t3: &Connection) -> Result<bool> {
    let head: i64 = t3
        .query_row(
            "SELECT max(migration_id) FROM effect_sql_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if head != 80 {
        return Ok(false);
    }
    // Validate the whole boundary before creating even the private native-side baseline.
    for table in TABLES {
        for column in table.columns {
            let exists: bool = t3
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
                    params![table.name, column],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                return Err(format!("T3 task schema missing {}.{column}", table.name));
            }
        }
    }
    Ok(true)
}

fn baseline_schema(t3: &Connection) -> Result<()> {
    t3.execute_batch("CREATE TABLE IF NOT EXISTS o3_task_baseline(table_name TEXT NOT NULL,row_key TEXT NOT NULL,row_hash TEXT NOT NULL,PRIMARY KEY(table_name,row_key));
        CREATE TABLE IF NOT EXISTS o3_task_sync(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),local_identity TEXT NOT NULL,local_generation INTEGER NOT NULL);").map_err(|e|e.to_string())
}
fn generation(local: &Connection) -> Result<(String, i64)> {
    local
        .query_row(
            "SELECT identity,generation FROM o3_task_state WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())
}
fn acknowledged(t3: &Connection) -> Result<Option<(String, i64)>> {
    t3.query_row(
        "SELECT local_identity,local_generation FROM o3_task_sync WHERE singleton=1 AND version=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}
fn key_values(key: &str) -> Result<Vec<String>> {
    serde_json::from_str(key).map_err(|e| e.to_string())
}
fn predicate(table: &Table) -> String {
    table
        .key
        .iter()
        .map(|c| format!("{c}=?"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn keys(db: &Connection, table: &Table, local: bool) -> Result<BTreeSet<String>> {
    let sql = if local && table.name == "scheduled_tasks" {
        "SELECT id FROM records WHERE kind='scheduledTasks'".to_owned()
    } else {
        format!("SELECT {} FROM {}", table.key.join(","), table.name)
    };
    let mut query = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |r| {
            let values = (0..table.key.len())
                .map(|i| r.get::<_, String>(i))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(serde_json::to_string(&values).unwrap())
        })
        .map_err(|e| e.to_string())?;
    rows.map(|r| r.map_err(|e| e.to_string())).collect()
}

/// Canonicalize JSON columns so property order or whitespace does not invent task edits.
fn normalized(table: &Table, mut row: Vec<Value>) -> Result<Vec<Value>> {
    for (column, field) in table.columns.iter().zip(&mut row) {
        if column.ends_with("_json") {
            let text = field.as_str().ok_or("Task JSON column is not text")?;
            let canonical = match *column {
                "request_json" => serde_json::to_string(
                    &serde_json::from_str::<crate::webhook_payload::Request>(text)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
                "headers_json" => serde_json::to_string(
                    &serde_json::from_str::<indexmap::IndexMap<String, String>>(text)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
                _ => serde_json::from_str::<Value>(text)
                    .map_err(|e| e.to_string())?
                    .to_string(),
            };
            *field = json!(canonical);
        }
    }
    Ok(row)
}

fn local_task(db: &Connection, key: &str) -> Result<Option<Vec<Value>>> {
    let values = key_values(key)?;
    let id = values.first().ok_or("Task key is empty")?;
    let body: Option<String> = db
        .query_row(
            "SELECT body FROM records WHERE kind='scheduledTasks' AND id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(body) = body else { return Ok(None) };
    let task: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let credentials = events::get(db, "hookCredentials", id).unwrap_or(Value::Null);
    let webhook = task["schedule"]["type"] == "webhook";
    let native = events::get(db, "scheduledTaskNative", id).unwrap_or(Value::Null);
    let row = TABLES[0]
        .columns
        .iter()
        .map(|column| {
            let value = match *column {
                "webhook_token" => {
                    if webhook {
                        credentials["token"].clone()
                    } else {
                        Value::Null
                    }
                }
                "webhook_secret" => {
                    if webhook {
                        credentials["secret"].clone()
                    } else {
                        Value::Null
                    }
                }
                "last_run_webhook_delivery_id" => native["lastRunWebhookDeliveryId"].clone(),
                column => {
                    let field = TASK_FIELDS.iter().find(|(c, _)| *c == column).unwrap().1;
                    if column == "enabled" {
                        json!(i64::from(task[field] == true))
                    } else if column == "thread_mode" {
                        task.get(field).cloned().unwrap_or_else(|| {
                            json!(if task["threadId"].is_null() {
                                "new_thread"
                            } else {
                                "continue"
                            })
                        })
                    } else if column.ends_with("_json") {
                        json!(task[field].to_string())
                    } else {
                        task[field].clone()
                    }
                }
            };
            value
        })
        .collect();
    Ok(Some(row))
}

fn row(db: &Connection, table: &Table, key: &str, local: bool) -> Result<Option<Vec<Value>>> {
    if local && table.name == "scheduled_tasks" {
        return local_task(db, key);
    }
    let sql = format!(
        "SELECT {} FROM {} WHERE {}",
        table.columns.join(","),
        table.name,
        predicate(table)
    );
    let values = key_values(key)?;
    let found = db
        .query_row(&sql, params_from_iter(values), |r| {
            (0..table.columns.len())
                .map(|i| {
                    Ok(match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Text(text) => json!(std::str::from_utf8(text)
                            .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                i,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?),
                        _ => {
                            return Err(rusqlite::Error::InvalidColumnType(
                                i,
                                table.columns[i].into(),
                                r.get_ref(i)?.data_type(),
                            ))
                        }
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(found)
}
fn fingerprint(table: &Table, row: Option<&Vec<Value>>) -> Result<Option<String>> {
    row.map(|row| {
        normalized(table, row.clone())
            .map(|row| format!("{:x}", Sha256::digest(serde_json::to_vec(&row).unwrap())))
    })
    .transpose()
}

fn baseline(db: &Connection, table: &Table, key: &str) -> Result<Option<String>> {
    db.query_row(
        "SELECT row_hash FROM o3_task_baseline WHERE table_name=?1 AND row_key=?2",
        params![table.name, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}
fn baseline_keys(db: &Connection, table: &Table) -> Result<BTreeSet<String>> {
    let mut query = db
        .prepare("SELECT row_key FROM o3_task_baseline WHERE table_name=?1")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([table.name], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    rows.map(|r| r.map_err(|e| e.to_string())).collect()
}

fn put_task(db: &Connection, values: &[Value]) -> Result<()> {
    let mut task = serde_json::Map::new();
    let column = |name: &str| &values[TABLES[0].columns.iter().position(|c| *c == name).unwrap()];
    for (sql, field) in TASK_FIELDS {
        let value = column(sql);
        let value = if *sql == "enabled" {
            json!(value.as_i64() != Some(0))
        } else if sql.ends_with("_json") {
            serde_json::from_str(value.as_str().ok_or("Task JSON column is not text")?)
                .map_err(|e| e.to_string())?
        } else {
            value.clone()
        };
        task.insert((*field).to_owned(), value);
    }
    let id = column("task_id").as_str().ok_or("Task id is not text")?;
    if let Some(token) = column("webhook_token").as_str() {
        task.insert("webhook".into(),json!({"path":format!("/api/hooks/{}/{token}",crate::webhook::encode_component(id)),"url":null,"hasSecret":!column("webhook_secret").is_null()}));
        events::put(
            db,
            "hookCredentials",
            id,
            "",
            &json!({"token":token,"secret":column("webhook_secret")}),
        )?;
    } else {
        db.execute(
            "DELETE FROM records WHERE kind='hookCredentials' AND id=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
    }
    events::put(
        db,
        "scheduledTaskNative",
        id,
        "",
        &json!({"lastRunWebhookDeliveryId":column("last_run_webhook_delivery_id")}),
    )?;
    events::put(db, "scheduledTasks", id, "", &Value::Object(task))
}

fn write_row(
    db: &Connection,
    table: &Table,
    key: &str,
    values: Option<&Vec<Value>>,
    local: bool,
) -> Result<()> {
    if local && table.name == "scheduled_tasks" {
        return match values {
            Some(values) => put_task(db, values),
            None => {
                crate::scheduled::remove_task(db, key_values(key)?.first().ok_or("Empty task key")?)
                    .map_err(|e| e.to_string())
            }
        };
    }
    let Some(values) = values else {
        db.execute(
            &format!("DELETE FROM {} WHERE {}", table.name, predicate(table)),
            params_from_iter(key_values(key)?),
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    };
    // UPDATE instead of REPLACE preserves the task's dependent pending/replay rows.
    let updates = table
        .columns
        .iter()
        .map(|c| format!("{c}=excluded.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) DO UPDATE SET {updates}",
        table.name,
        table.columns.join(","),
        vec!["?"; values.len()].join(","),
        table.key.join(",")
    );
    let values = values
        .iter()
        .map(|v| match v {
            Value::Null => Ok(rusqlite::types::Value::Null),
            Value::Number(n) => n
                .as_i64()
                .map(rusqlite::types::Value::Integer)
                .ok_or("Task integer out of range"),
            Value::String(s) => Ok(rusqlite::types::Value::Text(s.clone())),
            _ => Err("Unsupported task SQL value"),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    db.execute(&sql, params_from_iter(values))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Validate every planned import before changing any local row. Only row keys and hashes are
/// retained; large bodies are read one at a time and never duplicated in the baseline.
fn import_plan(local: &Connection, native: &Connection) -> Result<Vec<(usize, String)>> {
    let identity = generation(local)?.0;
    let first = acknowledged(native)?.is_none_or(|saved| saved.0 != identity);
    let mut plan = Vec::new();
    for (index, table) in TABLES.iter().enumerate() {
        let mut all = keys(native, table, false)?;
        all.extend(baseline_keys(native, table)?);
        for key in all {
            let remote = fingerprint(table, row(native, table, &key, false)?.as_ref())?;
            let old = baseline(native, table, &key)?;
            let ours = fingerprint(table, row(local, table, &key, true)?.as_ref())?;
            if remote == old && !first {
                continue;
            }
            if ours == remote {
                continue;
            }
            if ours != old && !(first && ours.is_none()) {
                return Err(format!(
                    "Shared task conflict in {} at {key}; both copies are retained",
                    table.name
                ));
            }
            plan.push((index, key));
        }
    }
    Ok(plan)
}

fn export_rows(local: &Connection, native: &Connection) -> Result<usize> {
    let mut written = 0;
    let mut plan = Vec::new();
    for (index, table) in TABLES.iter().enumerate() {
        let mut all = keys(local, table, true)?;
        all.extend(keys(native, table, false)?);
        all.extend(baseline_keys(native, table)?);
        for key in all {
            let ours = row(local, table, &key, true)?;
            let hash = fingerprint(table, ours.as_ref())?;
            let remote = fingerprint(table, row(native, table, &key, false)?.as_ref())?;
            let old = baseline(native, table, &key)?;
            if remote != old && remote != hash {
                return Err(format!(
                    "Shared task changed outside its profile owner: {} at {key}",
                    table.name
                ));
            }
            if remote != hash || old != hash {
                plan.push((index, key, remote != hash, hash));
            }
        }
    }
    // Validate the whole export before writes, then release pending unique keys. Parent
    // upserts use UPDATE rather than REPLACE, preserving the dependent rows and their IDs.
    for (index, key, changed, _) in &plan {
        let table = &TABLES[*index];
        if *changed && table.name == "scheduled_task_webhook_pending" {
            write_row(native, table, key, None, false)?;
        }
    }
    for (index, key, changed, hash) in plan {
        let table = &TABLES[index];
        if changed {
            write_row(
                native,
                table,
                &key,
                row(local, table, &key, true)?.as_ref(),
                false,
            )?;
            written += 1;
        }
        if let Some(hash) = hash {
            native.execute("INSERT INTO o3_task_baseline VALUES(?1,?2,?3) ON CONFLICT(table_name,row_key) DO UPDATE SET row_hash=excluded.row_hash",params![table.name,key,hash]).map_err(|e|e.to_string())?;
        } else {
            native
                .execute(
                    "DELETE FROM o3_task_baseline WHERE table_name=?1 AND row_key=?2",
                    params![table.name, key],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    let (identity, generation) = generation(local)?;
    native.execute("INSERT INTO o3_task_sync VALUES(1,1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET local_identity=excluded.local_identity,local_generation=excluded.local_generation",params![identity,generation]).map_err(|e|e.to_string())?;
    Ok(written)
}

pub(crate) fn start(s: &State, native: &mut Connection) -> Result<usize> {
    if !validate(native)? {
        return Ok(0);
    }
    let mut local = s.db.lock().unwrap();
    let tx = native
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    baseline_schema(&tx)?;
    let imports = import_plan(&local, &tx)?;
    let local_tx = local.transaction().map_err(|e| e.to_string())?;
    // Remove changed pending rows before reinserting them: two valid snapshots may exchange
    // coalescing keys and would otherwise violate the partial unique index mid-transaction.
    for (index, key) in imports.iter().rev() {
        let table = &TABLES[*index];
        if table.name == "scheduled_task_webhook_pending" || row(&tx, table, key, false)?.is_none()
        {
            write_row(&local_tx, table, key, None, true)?;
        }
    }
    for (index, key) in &imports {
        let table = &TABLES[*index];
        if let Some(value) = row(&tx, table, key, false)? {
            write_row(&local_tx, table, key, Some(&value), true)?;
        }
    }
    // Local imports commit before their native baseline. A crash before the native commit
    // repeats the import idempotently; no acknowledged baseline can hide an uncommitted import.
    let written = export_rows(&local_tx, &tx)?;
    local_tx.commit().map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(imports.len() + written)
}

pub(crate) fn export(s: &State, native: &mut Connection) -> Result<usize> {
    let head: i64 = native
        .query_row(
            "SELECT max(migration_id) FROM effect_sql_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if head != 80 {
        return Ok(0);
    }
    let local = s.db.lock().unwrap();
    let tx = native
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    baseline_schema(&tx)?;
    let saved = acknowledged(&tx)?;
    if saved.is_none()
        && TABLES
            .iter()
            .any(|table| keys(&tx, table, false).map_or(true, |rows| !rows.is_empty()))
    {
        return Err("Shared tasks require startup reconciliation before export".into());
    }
    if saved.as_ref() == Some(&generation(&local)?) {
        return Ok(0);
    }
    let written = export_rows(&local, &tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(written)
}
