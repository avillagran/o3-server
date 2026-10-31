//! Import only from an SQLite backup. The source handle is always read-only.
use crate::{
    events,
    store::{random_id, State},
};
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde_json::{json, Value};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
struct CopyFile(PathBuf);
impl Drop for CopyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub fn from_t3(s: &State, path: &Path) -> Result<Value, String> {
    let copy = CopyFile(std::env::temp_dir().join(format!("o3-import-{}.sqlite", random_id()?)));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&copy.0)
        .map_err(|e| e.to_string())?;
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut dest = Connection::open(&copy.0).map_err(|e| e.to_string())?;
    Backup::new(&source, &mut dest)
        .and_then(|b| b.run_to_completion(128, std::time::Duration::from_millis(5), None))
        .map_err(|e| e.to_string())?;
    drop(dest);
    drop(source);
    let source = Connection::open_with_flags(&copy.0, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut counts = serde_json::Map::new();
    for (table, kind, idcol) in [
        ("projection_projects", "project", "project_id"),
        ("orchestration_v2_projection_threads", "thread", "thread_id"),
        (
            "orchestration_v2_projection_messages",
            "messages",
            "message_id",
        ),
        (
            "orchestration_v2_projection_turn_items",
            "turnItems",
            "turn_item_id",
        ),
    ] {
        let mut q = source
            .prepare(&format!("SELECT * FROM {table}"))
            .map_err(|e| format!("{table}: {e}"))?;
        let names = q
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut rows = q.query([]).map_err(|e| e.to_string())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let index = |name: &str| names.iter().position(|n| n == name);
            let text = |name: &str| -> Option<String> {
                index(name).and_then(|i| row.get::<_, Option<String>>(i).ok().flatten())
            };
            let id = text(idcol).ok_or("missing import id")?;
            let tid = text("thread_id").unwrap_or_default();
            let v = if kind == "project" {
                let mut v = json!({"id":id});
                for (i, name) in names.iter().enumerate() {
                    if name == idcol {
                        continue;
                    }
                    let mut camel = String::new();
                    let mut upper = false;
                    for c in name.trim_end_matches("_json").chars() {
                        if c == '_' {
                            upper = true;
                        } else if upper {
                            camel.push(c.to_ascii_uppercase());
                            upper = false;
                        } else {
                            camel.push(c);
                        }
                    }
                    let value = match row.get_ref(i).map_err(|e| e.to_string())? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Text(b) => {
                            let t = std::str::from_utf8(b).map_err(|e| e.to_string())?;
                            if name.ends_with("_json") {
                                serde_json::from_str(t).map_err(|e| e.to_string())?
                            } else {
                                json!(t)
                            }
                        }
                        _ => return Err(format!("unsupported project column {name}")),
                    };
                    v[camel] = value;
                }
                v
            } else {
                serde_json::from_str::<Value>(&text("payload_json").ok_or("payload_json missing")?)
                    .map_err(|e| e.to_string())?
            };
            if v["id"] != id {
                return Err(format!("payload id mismatch in {table}"));
            }
            // Existing local records win: reimport cannot roll back later local edits.
            if events::get(&tx, kind, &id).is_err() {
                events::put(&tx, kind, &id, &tid, &v)?;
                count += 1;
                if kind == "turnItems" {
                    let visible = json!({"position":v["ordinal"],"sourceItemId":id,"sourceThreadId":tid,"visibility":"local","item":v});
                    events::put(&tx, "visibleTurnItems", &id, &tid, &visible)?;
                }
            }
        }
        counts.insert(kind.into(), json!(count));
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(Value::Object(counts))
}
