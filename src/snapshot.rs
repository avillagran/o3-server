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
    let mut db =
        Connection::open_with_flags(s.dir.join("o3.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    crate::store::profiled(&mut db);
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
        let window = if opts.bounded {
            window(db, tid, i64::MAX, 10)?
        } else {
            Window::default()
        };
        let anchor = window.ids.last().copied().unwrap_or(0);
        let more = window.more;
        let selected = serde_json::to_string(&window.ids).map_err(|e| e.to_string())?;
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
        // Local turn items already present as visible items, read once per snapshot instead of
        // one EXISTS per item.
        let visible_ids: std::collections::HashSet<String> = if opts.compact_items {
            let mut q = db
                .prepare("SELECT json_extract(body,'$.item.id') FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid IN (SELECT value FROM json_each(?))")
                .map_err(|e| e.to_string())?;
            let ids = q
                .query_map(params![tid, selected], |r| r.get::<_, Option<String>>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok().flatten())
                .collect();
            ids
        } else {
            Default::default()
        };
        for kind in KINDS {
            raw(out, &format!(",\"{kind}\":["))?;
            let transcript = matches!(*kind, "messages" | "turnItems" | "visibleTurnItems");
            // Select by canonical visible positions, independently of physical row age.
            let sql = if opts.bounded && transcript {
                if *kind == "visibleTurnItems" {
                    "SELECT body FROM records WHERE kind=?1 AND thread_id=?2 AND rowid IN (SELECT value FROM json_each(?3)) ORDER BY coalesce(json_extract(body,'$.position'),json_extract(body,'$.item.ordinal'),0),rowid"
                } else if *kind == "messages" {
                    // Nightly keeps every message belonging to an active run,
                    // even when its original turn item is outside the tool page.
                    "SELECT body FROM records r WHERE kind=?1 AND thread_id=?2 AND (id IN (SELECT json_extract(body,'$.item.messageId') FROM records WHERE kind='visibleTurnItems' AND thread_id=?2 AND rowid IN (SELECT value FROM json_each(?3))) OR json_extract(r.body,'$.runId') IN (SELECT id FROM records WHERE kind='runs' AND thread_id=?2 AND json_extract(body,'$.status') IN ('queued','preparing','starting','running','waiting'))) ORDER BY json_extract(body,'$.createdAt'),id"
                } else {
                    "SELECT body FROM records r WHERE kind=?1 AND thread_id=?2 AND id IN (SELECT json_extract(body,CASE WHEN ?1='messages' THEN '$.item.messageId' ELSE '$.item.id' END) FROM records WHERE kind='visibleTurnItems' AND thread_id=?2 AND rowid IN (SELECT value FROM json_each(?3))) ORDER BY rowid"
                }
            } else {
                "SELECT body FROM records WHERE kind=?1 AND thread_id=?2 ORDER BY rowid"
            };
            let mut q = db.prepare(sql).map_err(|e| e.to_string())?;
            let mut rows = if opts.bounded && transcript {
                q.query(params![kind, tid, selected])
            } else {
                q.query(params![kind, tid])
            }
            .map_err(|e| e.to_string())?;
            let mut first = true;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let body: String = row.get(0).map_err(|e| e.to_string())?;
                let mut v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                if opts.compact_items
                    && *kind == "turnItems"
                    && v["id"].as_str().is_some_and(|id| visible_ids.contains(id))
                {
                    continue;
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
/// T3's `THREAD_HISTORY_PAGE_POLICY` (`threadHistoryPaging.ts`): a page that reaches a user
/// turn keeps up to `max_turns` user turns and 500 entries, so ordinary turns stay whole.
/// A page inside one turn longer than that (no user turn selected) keeps the newest 75
/// entries. Every page stops at 1 MiB and admits at least one entry. Returns the oldest
/// selected row IDs in reverse logical order and whether older entries remain.
#[derive(Default)]
pub(crate) struct Window {
    pub ids: Vec<i64>,
    pub more: bool,
}
pub(crate) fn window(
    db: &Connection,
    tid: &str,
    before: i64,
    max_turns: usize,
) -> Result<Window, String> {
    const MAX_ITEMS: usize = 75;
    const MAX_TURN_ITEMS: usize = 500;
    const MAX_BYTES: usize = 1024 * 1024;
    use rusqlite::OptionalExtension;
    let before_position = if before == i64::MAX {
        i64::MAX
    } else {
        db.query_row("SELECT coalesce(json_extract(body,'$.position'),json_extract(body,'$.item.ordinal'),0) FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND rowid=?",params![tid,before],|row|row.get::<_,i64>(0)).optional().map_err(|e|e.to_string())?.ok_or("invalid history cursor")?
    };
    // Sort IDs only; payloads are fetched after the fixed row limit.
    let mut q=db.prepare("SELECT rowid FROM records WHERE kind='visibleTurnItems' AND thread_id=?1 AND (coalesce(json_extract(body,'$.position'),json_extract(body,'$.item.ordinal'),0)<?2 OR (coalesce(json_extract(body,'$.position'),json_extract(body,'$.item.ordinal'),0)=?2 AND rowid<?3)) ORDER BY coalesce(json_extract(body,'$.position'),json_extract(body,'$.item.ordinal'),0) DESC,rowid DESC LIMIT 501").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![tid, before_position, before], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())?;
    let (mut ids, mut turns, mut bytes, mut more) = (Vec::new(), 0, 0, false);
    for row in rows {
        let id = row.map_err(|e| e.to_string())?;
        let b: String = db
            .query_row("SELECT body FROM records WHERE rowid=?", [id], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        if !ids.is_empty()
            && (ids.len() >= MAX_TURN_ITEMS || turns >= max_turns || bytes + b.len() > MAX_BYTES)
        {
            more = true;
            break;
        }
        let v: Value = serde_json::from_str(&b).map_err(|e| e.to_string())?;
        if v["item"]["type"] == "user_message" {
            turns += 1;
        }
        ids.push(id);
        bytes += b.len();
    }
    if turns == 0 && ids.len() > MAX_ITEMS {
        ids.truncate(MAX_ITEMS);
        more = true;
    }
    Ok(Window { ids, more })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db(items: &[&str]) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE records(kind TEXT NOT NULL,id TEXT NOT NULL,thread_id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(kind,id));").unwrap();
        for (n, kind) in items.iter().enumerate() {
            let body = json!({"item":{"type":kind}}).to_string();
            db.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('visibleTurnItems',?,'t',?)",
                params![n.to_string(), body],
            )
            .unwrap();
        }
        db
    }
    fn selected(db: &Connection, max_turns: usize) -> (usize, bool) {
        let window = window(db, "t", i64::MAX, max_turns).unwrap();
        (window.ids.len(), window.more)
    }

    #[test]
    fn a_tool_heavy_turn_stays_whole_when_its_user_message_is_in_reach() {
        let mut items = vec!["user_message"];
        items.extend(std::iter::repeat_n("command_execution", 300));
        items.push("assistant_message");
        assert_eq!(selected(&db(&items), 10), (302, false));
    }

    #[test]
    fn inside_a_turn_longer_than_the_window_the_newest_75_entries_show() {
        let mut items = vec!["user_message"];
        items.extend(std::iter::repeat_n("command_execution", 600));
        assert_eq!(selected(&db(&items), 10), (75, true));
    }

    #[test]
    fn pages_stop_at_the_user_turn_limit() {
        let items: Vec<&str> = (0..30)
            .flat_map(|_| ["user_message", "assistant_message"])
            .collect();
        assert_eq!(selected(&db(&items), 10), (20, true));
        assert_eq!(selected(&db(&items), 20), (40, true));
    }
}
