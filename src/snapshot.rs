//! Consistent disk snapshots, with backpressure at 32 KiB rather than a JSON tree.
use crate::{events, store::State};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::io::{self, Write};
use tokio::sync::mpsc;
pub type Receiver = mpsc::Receiver<Result<Vec<u8>, String>>;
#[derive(Clone, Default)]
pub struct Options {
    pub bounded: bool,
    pub dedupe: bool,
    pub compact_items: bool,
    pub compact_ids: bool,
    pub resolved_roots: Vec<String>,
}
const KINDS: &[&str] = &[
    "runs",
    "attempts",
    "nodes",
    "subagents",
    "providerSessions",
    "providerThreads",
    "providerTurns",
    "runtimeRequests",
    "messages",
    "plans",
    "turnItems",
    "checkpointScopes",
    "checkpoints",
    "contextHandoffs",
    "contextTransfers",
    "visibleTurnItems",
];
struct ChannelWriter(mpsc::Sender<Result<Vec<u8>, String>>);
impl Write for ChannelWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0
            .blocking_send(Ok(b[..b.len().min(16 * 1024)].to_vec()))
            .map_err(|_| io::Error::other("snapshot cancelled"))?;
        Ok(b.len().min(16 * 1024))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Establish the read transaction before returning the live replay boundary.
pub fn start(
    s: &State,
    tid: Option<String>,
    opts: Options,
    rpc: Option<Value>,
) -> Result<(i64, Receiver), String> {
    let db = Connection::open_with_flags(s.dir.join("o3.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA cache_size=-512; PRAGMA mmap_size=0; BEGIN")
        .map_err(|e| e.to_string())?;
    let seq = events::sequence(&db)?;
    if let Some(tid) = &tid {
        events::get(&db, "thread", tid)?;
    }
    let (tx, rx) = mpsc::channel(2);
    tokio::task::spawn_blocking(move || {
        let mut out = io::BufWriter::with_capacity(16 * 1024, ChannelWriter(tx.clone()));
        let result = (|| -> Result<(), String> {
            if let Some(id) = &rpc {
                write!(out, "{{\"_tag\":\"Chunk\",\"requestId\":{id},\"values\":[")
                    .map_err(|e| e.to_string())?;
            }
            write_snapshot(&db, &mut out, tid.as_deref(), &opts, seq, rpc.is_some())?;
            if rpc.is_some() {
                out.write_all(b"]}").map_err(|e| e.to_string())?;
            }
            out.flush().map_err(|e| e.to_string())
        })();
        if let Err(e) = result {
            let _ = tx.blocking_send(Err(e));
        }
        drop(out);
        drop(db);
        // Return freed libc page/JSON/sort buffers after completion or cancellation.
        crate::heap::collect();
    });
    Ok((seq, rx))
}
fn emit(out: &mut impl Write, v: &Value) -> Result<(), String> {
    serde_json::to_writer(out, v).map_err(|e| e.to_string())
}
fn raw(out: &mut impl Write, s: &str) -> Result<(), String> {
    out.write_all(s.as_bytes()).map_err(|e| e.to_string())
}
fn write_snapshot(
    db: &Connection,
    out: &mut impl Write,
    tid: Option<&str>,
    opts: &Options,
    seq: i64,
    rpc: bool,
) -> Result<(), String> {
    if let Some(tid) = tid {
        let mut thread = events::get(db, "thread", tid)?;
        let mut ids = std::collections::BTreeMap::new();
        let compact_ids = opts.compact_ids;
        if compact_ids {
            compact_record(&mut thread, &mut ids);
        }
        // Bound all transcript representations to the same visible window.
        let anchor = if opts.bounded {
            boundary(db, tid, i64::MAX)?.0
        } else {
            0
        };
        let more=anchor>0 && db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid<?)",params![tid,anchor],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
        raw(out, "{")?;
        if rpc {
            raw(out, "\"kind\":\"snapshot\",")?;
        }
        if opts.compact_items {
            raw(out, "\"turnItemsOmitLocalVisible\":true,")?;
        }
        let latest = json!(events::latest_local_ordinal(db, tid)?);
        raw(out,&format!("\"snapshotSequence\":{seq},\"historyCursor\":{},\"hasMoreHistory\":{more},\"latestLocalTurnOrdinal\":{latest},\"projection\":{{\"thread\":",if more{json!(anchor.to_string()).to_string()}else{"null".into()}))?;
        emit(out, &thread)?;
        raw(out, ",\"updatedAt\":")?;
        emit(out, &thread["updatedAt"])?;
        for kind in KINDS {
            raw(out, &format!(",\"{kind}\":["))?;
            let transcript = matches!(*kind, "messages" | "turnItems" | "visibleTurnItems");
            // Raw records corresponding to visible items are selected by their IDs, not rowid.
            let sql = if opts.bounded && transcript && *kind != "visibleTurnItems" {
                "SELECT body FROM records r WHERE kind=?1 AND thread_id=?2 AND (id IN (SELECT json_extract(body,CASE WHEN ?1='messages' THEN '$.item.messageId' ELSE '$.item.id' END) FROM records WHERE kind='visibleTurnItems' AND thread_id=?2 AND rowid>=?3)) ORDER BY rowid"
            } else {
                "SELECT body FROM records WHERE kind=?1 AND thread_id=?2 AND (?4=0 OR rowid>=?3) ORDER BY rowid"
            };
            let mut q = db.prepare(sql).map_err(|e| e.to_string())?;
            let mut rows = if sql.contains("?4") {
                q.query(params![kind, tid, anchor, opts.bounded && transcript])
            } else {
                q.query(params![kind, tid, anchor])
            }
            .map_err(|e| e.to_string())?;
            let mut first = true;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let body: String = row.get(0).map_err(|e| e.to_string())?;
                let mut v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                if opts.compact_items && *kind == "turnItems" {
                    let present=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid>=? AND json_extract(body,'$.item.id')=?)",params![tid,anchor,v["id"].as_str()],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?;
                    if present {
                        continue;
                    }
                }
                if opts.dedupe
                    && (*kind != "visibleTurnItems"
                        || v["visibility"] == "local"
                        || v["sourceThreadId"] == tid)
                {
                    let item = if *kind == "visibleTurnItems" {
                        &mut v["item"]
                    } else {
                        &mut v
                    };
                    if let Some(mid) = item["messageId"].as_str() {
                        if let Ok(m) = events::get(db, "messages", mid) {
                            if item.get("text") == m.get("text") && item.get("text").is_some() {
                                item.as_object_mut().unwrap().remove("text");
                                item["textOmitted"] = json!(true);
                            }
                        }
                    }
                }
                if compact_ids {
                    compact_record(&mut v, &mut ids);
                }
                if !first {
                    raw(out, ",")?;
                }
                first = false;
                emit(out, &v)?;
            }
            raw(out, "]")?;
        }
        raw(out, "}")?;
        if compact_ids {
            let mut table = vec![Value::Null; ids.len()];
            for (id, n) in ids {
                table[n] = json!(id);
            }
            raw(out, ",\"ids\":")?;
            emit(out, &json!(table))?;
        }
        raw(out, "}")?;
    } else {
        if rpc {
            raw(out, "{\"kind\":\"snapshot\",")?;
            if !opts.resolved_roots.is_empty() {
                raw(out, "\"resolvedRepositoryIdentityRoots\":")?;
                emit(out, &json!(opts.resolved_roots))?;
                raw(out, ",")?;
            }
            raw(out, "\"snapshot\":")?;
        }
        raw(
            out,
            &format!("{{\"schemaVersion\":2,\"snapshotSequence\":{seq}"),
        )?;
        for (field, kind, archive) in [
            ("projects", "project", false),
            ("threads", "thread", false),
            ("archivedThreads", "thread", true),
        ] {
            raw(out, &format!(",\"{field}\":["))?;
            let mut q = db
                .prepare("SELECT body FROM records WHERE kind=? ORDER BY rowid")
                .map_err(|e| e.to_string())?;
            let mut rows = q.query([kind]).map_err(|e| e.to_string())?;
            let mut first = true;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let b: String = row.get(0).map_err(|e| e.to_string())?;
                let mut v: Value = serde_json::from_str(&b).map_err(|e| e.to_string())?;
                if !v["deletedAt"].is_null()
                    || kind == "thread" && (!v["archivedAt"].is_null()) != archive
                {
                    continue;
                }
                if kind == "thread" {
                    v = events::shell_thread(db, &v)?;
                }
                if !first {
                    raw(out, ",")?;
                }
                first = false;
                emit(out, &v)?;
            }
            raw(out, "]")?;
        }
        raw(out, "}")?;
        if rpc {
            raw(out, "}")?;
        }
    }
    Ok(())
}
/// Latest page: at most ten user turns, 75 generic entries, 500 entries and 1 MiB.
pub(crate) fn boundary(db: &Connection, tid: &str, before: i64) -> Result<(i64, bool), String> {
    let mut q=db.prepare("SELECT rowid,body FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid<? ORDER BY rowid DESC").map_err(|e|e.to_string())?;
    let mut rows = q.query(params![tid, before]).map_err(|e| e.to_string())?;
    let (mut anchor, mut turns, mut generic, mut count, mut bytes) = (0, 0, 0, 0, 0);
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let id: i64 = row.get(0).map_err(|e| e.to_string())?;
        let b: String = row.get(1).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&b).map_err(|e| e.to_string())?;
        let user = v["item"]["type"] == "user_message";
        if count > 0
            && (count >= 500 || generic >= 75 || turns >= 10 || bytes + b.len() > 1024 * 1024)
        {
            return Ok((anchor, true));
        }
        anchor = id;
        count += 1;
        bytes += b.len();
        if user {
            turns += 1;
        } else {
            generic += 1;
        }
    }
    Ok((anchor, false))
}

fn compact_record(v: &mut Value, ids: &mut std::collections::BTreeMap<String, usize>) {
    if let Some(fields) = v.as_object_mut() {
        for (key, value) in fields {
            if key == "id" || key.ends_with("Id") {
                compact_id(value, ids);
            } else if key.ends_with("Ids") {
                if let Some(a) = value.as_array_mut() {
                    for id in a {
                        compact_id(id, ids);
                    }
                }
            } else if key == "item" {
                compact_record(value, ids);
            }
        }
    }
}
fn compact_id(v: &mut Value, ids: &mut std::collections::BTreeMap<String, usize>) {
    if let Some(id) = v.as_str() {
        if let Some(n) = ids.get(id) {
            *v = json!(n);
        } else if ids.len() < 8192 {
            let n = ids.len();
            ids.insert(id.to_owned(), n);
            *v = json!(n);
        }
    }
}
