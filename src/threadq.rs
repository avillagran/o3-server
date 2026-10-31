//! Read-side orchestration queries: thread search (SQLite FTS5), archived shell, single turn items,
//! bounded thread projections and checkpoint diffs.
use crate::{
    events, paths,
    proc::{self, Opts},
    store::State,
    vcs,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;

type Reply = Result<Value, Value>;
const DIFF_MAX: usize = 10_000_000;
const TRUNCATED: &str = "\n\n[truncated]";

// ------------------------------------------------------------------ FTS5 index

/// Creates the message index; fills it from existing messages the first time it exists.
pub fn initialize(db: &Connection) -> Result<(), String> {
    let existed: bool = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='thread_search'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?
        > 0;
    db.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS thread_search USING fts5(text, thread_id UNINDEXED, message_id UNINDEXED, role UNINDEXED, created_at UNINDEXED, tokenize='unicode61 remove_diacritics 2');")
        .map_err(|e| e.to_string())?;
    if !existed {
        let mut q = db
            .prepare("SELECT thread_id, body FROM records WHERE kind='messages'")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        for (tid, body) in rows {
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                index_message(db, &tid, &v);
            }
        }
    }
    Ok(())
}

/// Keeps one row per message; called whenever a message record is written.
pub fn index_message(db: &Connection, thread_id: &str, m: &Value) {
    let (Some(id), Some(text)) = (m["id"].as_str(), m["text"].as_str()) else {
        return;
    };
    let _ = db.execute("DELETE FROM thread_search WHERE message_id=?", [id]);
    if !text.trim().is_empty() && matches!(m["role"].as_str(), Some("user" | "assistant")) {
        let _ = db.execute(
            "INSERT INTO thread_search(text,thread_id,message_id,role,created_at) VALUES(?,?,?,?,?)",
            params![text, thread_id, id, m["role"].as_str(), m["createdAt"].as_str()],
        );
    }
}

/// `"tok"*` terms, ANDed: user text can never become FTS5 syntax.
fn match_expression(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(12)
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub fn search_threads(s: &State, p: &Value) -> Reply {
    let fail = |m: &str| json!({"_tag":"OrchestrationSearchThreadsError","message":m});
    let query = p["query"]
        .as_str()
        .map(str::trim)
        .filter(|q| q.chars().count() >= 2 && q.chars().count() <= 200)
        .ok_or_else(|| fail("query must be 2-200 characters"))?;
    let limit = p["limit"].as_i64().unwrap_or(20).clamp(1, 50) as usize;
    let projects: Option<Vec<&str>> = p["projectIds"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect());
    let db = s.db.lock().unwrap();
    let live = |tid: &str| -> Option<Value> {
        events::get(&db, "thread", tid).ok().filter(|t| {
            t["deletedAt"].is_null()
                && t["archivedAt"].is_null()
                && projects
                    .as_ref()
                    .is_none_or(|ps| t["projectId"].as_str().is_some_and(|p| ps.contains(&p)))
        })
    };
    let mut matches: Vec<Value> = Vec::new();
    if p["titlesOnly"] != true {
        let Some(expr) = match_expression(query) else {
            return Ok(json!({"matches":[]}));
        };
        let mut q = db
            .prepare("SELECT thread_id, message_id, role, created_at, snippet(thread_search, 0, '', '', '…', 24), bm25(thread_search) FROM thread_search WHERE thread_search MATCH ?1 ORDER BY bm25(thread_search) LIMIT 1000")
            .map_err(|e| fail(&e.to_string()))?;
        let rows = q
            .query_map([expr], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })
            .map_err(|e| fail(&e.to_string()))?;
        // Best message per thread; `hits` counts the thread's matching messages.
        let mut seen: std::collections::HashMap<String, usize> = Default::default();
        for (tid, mid, role, at, snippet, rank) in rows.flatten() {
            if let Some(&i) = seen.get(&tid) {
                matches[i]["hits"] = json!(matches[i]["hits"].as_i64().unwrap_or(1) + 1);
                continue;
            }
            let Some(thread) = live(&tid) else { continue };
            seen.insert(tid.clone(), matches.len());
            matches.push(json!({"threadId":tid,"projectId":thread["projectId"],"source":role,"snippet":snippet.chars().take(240).collect::<String>(),"messageCreatedAt":at,"quality":-rank,"hits":1,"messageId":mid}));
        }
    } else {
        let needle = query.to_lowercase();
        let mut q = db
            .prepare("SELECT thread_id, body FROM records WHERE kind='thread' ORDER BY rowid DESC")
            .map_err(|e| fail(&e.to_string()))?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| fail(&e.to_string()))?;
        for (tid, body) in rows.flatten() {
            let Ok(t) = serde_json::from_str::<Value>(&body) else {
                continue;
            };
            if t["title"]
                .as_str()
                .is_some_and(|title| title.to_lowercase().contains(&needle))
                && live(&tid).is_some()
            {
                matches.push(json!({"threadId":tid,"projectId":t["projectId"],"source":"user","snippet":t["title"].as_str().unwrap_or("").chars().take(240).collect::<String>(),"messageCreatedAt":null}));
            }
        }
    }
    matches.truncate(limit);
    let seq = events::sequence(&db).unwrap_or(0);
    Ok(json!({"matches":matches,"snapshotSequence":seq}))
}

// ------------------------------------------------------------------ simple reads

pub fn archived_shell(s: &State) -> Reply {
    let shell = events::shell(s)
        .map_err(|m| json!({"_tag":"OrchestrationV2GetShellSnapshotError","message":m}))?;
    Ok(
        json!({"schemaVersion":shell["schemaVersion"],"snapshotSequence":shell["snapshotSequence"],"projects":shell["projects"],"threads":shell["archivedThreads"]}),
    )
}

pub fn turn_item(s: &State, p: &Value) -> Reply {
    let fail = |m: &str| json!({"_tag":"OrchestrationV2GetThreadProjectionError","threadId":p["threadId"],"message":m});
    let (Some(tid), Some(item)) = (p["threadId"].as_str(), p["itemId"].as_str()) else {
        return Err(fail("threadId and itemId are required"));
    };
    let db = s.db.lock().unwrap();
    let found = events::get(&db, "turnItems", item)
        .ok()
        .filter(|i| i["threadId"] == tid);
    Ok(json!({"item":found}))
}

pub fn thread_projection(s: &State, p: &Value) -> Reply {
    let tid = p["threadId"].as_str().unwrap_or("");
    events::projection(s, tid).map_err(|_| json!({"_tag":"OrchestrationV2GetThreadProjectionError","threadId":p["threadId"],"message":format!("Failed to load orchestration V2 thread {tid}")}))
}

// ------------------------------------------------------------------ checkpoint diffs

fn records(db: &Connection, kind: &str, tid: &str) -> Vec<Value> {
    let Ok(mut q) =
        db.prepare("SELECT body FROM records WHERE kind=? AND thread_id=? ORDER BY rowid")
    else {
        return vec![];
    };
    let Ok(rows) = q.query_map(params![kind, tid], |r| r.get::<_, String>(0)) else {
        return vec![];
    };
    rows.flatten()
        .filter_map(|b| serde_json::from_str(&b).ok())
        .collect()
}
fn diff_err(tag: &str, message: &str, cause: Option<Value>) -> Value {
    let mut e = json!({"_tag":tag,"message":message});
    if let Some(c) = cause {
        e["cause"] = c;
    }
    e
}

pub async fn turn_diff(s: &State, p: Value) -> Reply {
    let tag = "OrchestrationGetTurnDiffError";
    let message = "Failed to load turn diff";
    let (Some(tid), Some(from), Some(to)) = (
        p["threadId"].as_str(),
        p["fromTurnCount"].as_u64(),
        p["toTurnCount"].as_u64(),
    ) else {
        return Err(diff_err(
            tag,
            message,
            Some(json!(
                "threadId, fromTurnCount and toTurnCount are required"
            )),
        ));
    };
    if from > to {
        return Err(diff_err(
            tag,
            message,
            Some(json!("fromTurnCount must not exceed toTurnCount")),
        ));
    }
    diff_between(s, tid, from, to, p["ignoreWhitespace"] != false)
        .await
        .map_err(|c| diff_err(tag, message, Some(c)))
}

pub async fn full_thread_diff(s: &State, p: Value) -> Reply {
    let tag = "OrchestrationGetFullThreadDiffError";
    let message = "Failed to load full thread diff";
    let (Some(tid), Some(to)) = (p["threadId"].as_str(), p["toTurnCount"].as_u64()) else {
        return Err(diff_err(
            tag,
            message,
            Some(json!("threadId and toTurnCount are required")),
        ));
    };
    diff_between(s, tid, 0, to, p["ignoreWhitespace"] != false)
        .await
        .map_err(|c| diff_err(tag, message, Some(c)))
}

async fn diff_between(
    s: &State,
    tid: &str,
    from: u64,
    to: u64,
    ignore_ws: bool,
) -> Result<Value, Value> {
    let result =
        |diff: String| json!({"threadId":tid,"fromTurnCount":from,"toTurnCount":to,"diff":diff});
    if from == to {
        return Ok(result(String::new()));
    }
    let (from_ref, to_ref, cwd) = {
        let db = s.db.lock().unwrap();
        if events::get(&db, "thread", tid).is_err() {
            return Err(json!({"_tag":"CheckpointThreadNotFoundError","threadId":tid}));
        }
        let completed: std::collections::HashSet<String> = records(&db, "runs", tid)
            .iter()
            .filter(|r| r["status"] == "completed")
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        let checkpoints = records(&db, "checkpoints", tid);
        let scopes = records(&db, "checkpointScopes", tid);
        let ready: Vec<&Value> = checkpoints
            .iter()
            .filter(|c| {
                c["status"] == "ready"
                    && c["appRunOrdinal"].is_u64()
                    && c["runId"].as_str().is_some_and(|r| completed.contains(r))
            })
            .collect();
        let max = ready
            .iter()
            .filter_map(|c| c["appRunOrdinal"].as_u64())
            .max()
            .unwrap_or(0);
        if to > max {
            return Err(
                json!({"_tag":"CheckpointTurnRangeUnavailableError","threadId":tid,"requestedTurnCount":to,"availableTurnCount":max}),
            );
        }
        let to_cp = ready.iter().find(|c| c["appRunOrdinal"].as_u64() == Some(to)).ok_or_else(|| json!({"_tag":"CheckpointRefUnavailableError","threadId":tid,"turnCount":to,"checkpoint":"to"}))?;
        let scope = scopes
            .iter()
            .find(|sc| sc["id"] == to_cp["scopeId"])
            .ok_or_else(|| json!({"_tag":"CheckpointWorkspacePathMissingError","threadId":tid}))?;
        let unavailable = || json!({"_tag":"CheckpointRefUnavailableError","threadId":tid,"turnCount":from,"checkpoint":"from"});
        let from_ref = if from == 0 {
            // The root run's baseline is its scope's first checkpoint.
            let root = scopes
                .iter()
                .find(|sc| sc["kind"] == "root_run")
                .ok_or_else(unavailable)?;
            checkpoints
                .iter()
                .find(|c| c["scopeId"] == root["id"] && c["ordinalWithinScope"].as_u64() == Some(0))
                .and_then(|c| c["ref"].as_str())
                .ok_or_else(unavailable)?
                .to_owned()
        } else {
            let parent = (from + 1 == to)
                .then(|| {
                    checkpoints.iter().find(|c| {
                        c["status"] == "ready"
                            && c["scopeId"] == to_cp["scopeId"]
                            && to_cp["ordinalWithinScope"].as_u64().is_some_and(|o| {
                                o > 0 && c["ordinalWithinScope"].as_u64() == Some(o - 1)
                            })
                    })
                })
                .flatten();
            parent
                .or_else(|| {
                    ready
                        .iter()
                        .find(|c| c["appRunOrdinal"].as_u64() == Some(from))
                        .copied()
                })
                .and_then(|c| c["ref"].as_str())
                .ok_or_else(unavailable)?
                .to_owned()
        };
        (
            from_ref,
            to_cp["ref"].as_str().unwrap_or("").to_owned(),
            scope["cwd"].as_str().unwrap_or("").to_owned(),
        )
    };
    for r in [&from_ref, &to_ref] {
        if !r.starts_with("refs/") || !vcs::valid_ref_name(r) {
            return Err(
                json!({"_tag":"CheckpointRefUnavailableError","threadId":tid,"detail":"checkpoint ref is not a valid git ref"}),
            );
        }
    }
    let cwd_path = Path::new(&cwd);
    if !cwd_path.is_absolute() || !paths::is_known_workspace(s, cwd_path) {
        return Err(json!({"_tag":"CheckpointWorkspacePathMissingError","threadId":tid}));
    }
    let mut args = vec![
        "diff",
        "--find-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    if ignore_ws {
        args.push("--ignore-all-space");
    }
    args.extend([from_ref.as_str(), to_ref.as_str(), "--"]);
    let o = proc::git(
        cwd_path,
        &args,
        Opts {
            timeout: proc::READ_TIMEOUT,
            cap: DIFF_MAX,
            allow_truncate: true,
        },
    )
    .await
    .map_err(|e| vcs::run_err("CheckpointStore.diffCheckpoints", cwd_path, e))?;
    if !o.ok() {
        return Err(vcs::git_fail(
            "CheckpointStore.diffCheckpoints",
            cwd_path,
            &args,
            &o,
        ));
    }
    let mut diff = String::from_utf8_lossy(&o.stdout).into_owned();
    if o.truncated {
        diff.push_str(TRUNCATED);
    }
    Ok(result(diff))
}
