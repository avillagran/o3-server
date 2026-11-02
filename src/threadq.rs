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

const CREATE_INDEX: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS thread_search USING fts5(text, thread_id UNINDEXED, message_id UNINDEXED, role UNINDEXED, created_at UNINDEXED, tokenize='unicode61 remove_diacritics 2');";

/// Creates the message index and fills it in one transaction: a failed rebuild leaves no
/// index, so the next start retries it instead of serving a permanently incomplete one.
pub fn initialize(db: &Connection) -> Result<(), String> {
    let existed: bool = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='thread_search'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?
        > 0;
    if existed {
        return Ok(());
    }
    db.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let rebuilt = rebuild(db);
    let end = if rebuilt.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    };
    db.execute_batch(end).map_err(|e| e.to_string())?;
    rebuilt
}

fn rebuild(db: &Connection) -> Result<(), String> {
    db.execute_batch(CREATE_INDEX).map_err(|e| e.to_string())?;
    let mut q = db
        .prepare("SELECT thread_id, body FROM records WHERE kind='messages'")
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (tid, body) = row.map_err(|e| e.to_string())?;
        if let Ok(v) = serde_json::from_str::<Value>(&body) {
            index_message(db, &tid, &v)?;
        }
    }
    Ok(())
}

/// Keeps one row per message; called whenever a message record is written.
pub fn index_message(db: &Connection, thread_id: &str, m: &Value) -> Result<(), String> {
    let (Some(id), Some(text)) = (m["id"].as_str(), m["text"].as_str()) else {
        return Ok(());
    };
    db.execute("DELETE FROM thread_search WHERE message_id=?", [id])
        .map_err(|e| e.to_string())?;
    if !text.trim().is_empty() && matches!(m["role"].as_str(), Some("user" | "assistant")) {
        db.execute(
            "INSERT INTO thread_search(text,thread_id,message_id,role,created_at) VALUES(?,?,?,?,?)",
            params![text, thread_id, id, m["role"].as_str(), m["createdAt"].as_str()],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `"tok"*` terms, ANDed: user text can never become FTS5 syntax.
fn match_expression(query: &str) -> Option<String> {
    let terms = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .take(12)
            .map(|t| format!("\"{t}\"*"))
            .collect()
    };
    // The index stores raw text, so a folded query alone misses compatibility forms.
    let folded = terms(&fold_search(query));
    let raw = terms(query);
    if folded.is_empty() {
        return None;
    }
    Some(if raw.is_empty() || raw == folded {
        folded.join(" ")
    } else {
        format!("({}) OR ({})", folded.join(" "), raw.join(" "))
    })
}

/// Accent/case folding used by the palette, preserving compatibility characters
/// whose decomposition would expand the source character.
fn fold_search(text: &str) -> String {
    use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
    text.chars()
        .map(|c| {
            let folded: String = c
                .to_string()
                .nfkd()
                .filter(|c| !is_combining_mark(*c))
                .collect::<String>()
                .to_lowercase()
                .replace('ς', "σ")
                .replace(['‘', '’'], "'");
            if folded.chars().count() <= 1 {
                folded
            } else {
                c.to_lowercase().collect()
            }
        })
        .collect()
}

fn search_snippet(text: &str, needle: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 240 {
        return text;
    }
    let folded = fold_search(&text);
    let byte = folded.find(needle).unwrap_or(0);
    let folded_index = folded[..byte].chars().count();
    let mut emitted = 0;
    let index = chars
        .iter()
        .position(|c| {
            let n = fold_search(&c.to_string()).chars().count();
            let found = n > 0 && emitted >= folded_index;
            emitted += n;
            found
        })
        .unwrap_or(0);
    let start = index
        .saturating_sub(24)
        .min(chars.len().saturating_sub(238));
    let end = (start + 238).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
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
    let db = s.reader.lock().unwrap();
    let live = |tid: &str| -> Option<Value> {
        events::get(&db, "thread", tid).ok().filter(|t| {
            t["deletedAt"].is_null()
                && t["archivedAt"].is_null()
                && p["projectId"]
                    .as_str()
                    .is_none_or(|id| t["projectId"] == id)
                && t["lineage"]["relationshipToParent"] != "subagent"
                && projects
                    .as_ref()
                    .is_none_or(|ps| t["projectId"].as_str().is_some_and(|p| ps.contains(&p)))
        })
    };
    let mut matches: Vec<Value> = Vec::new();
    if p["titlesOnly"] != true {
        let expr = match_expression(query).unwrap_or_else(|| "\"\"".into());
        let needle = fold_search(query);
        // Independent FTS and substring phases each scan at most 8 MiB.
        let mut seen: std::collections::HashMap<String, usize> = Default::default();
        let mut message_ids = std::collections::HashSet::new();
        for sql in [
            "SELECT thread_id,message_id,role,created_at,text,bm25(thread_search) FROM thread_search WHERE thread_search MATCH ?1 AND length(CAST(text AS BLOB))<=8388608 ORDER BY bm25(thread_search) LIMIT 1000",
            "SELECT thread_id,message_id,role,created_at,text,0.0 FROM (SELECT * FROM thread_search ORDER BY rowid DESC LIMIT 4096) WHERE ?1 IS NOT NULL AND length(CAST(text AS BLOB))<=8388608",
        ] {
            let mut q = db.prepare(sql).map_err(|e|fail(&e.to_string()))?;
            let rows = q.query_map([&expr], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,String>(4)?,r.get::<_,f64>(5)?))).map_err(|e|fail(&e.to_string()))?;
            let mut scanned_bytes = 0usize;
        for (tid, mid, role, at, snippet, rank) in rows.flatten() {
            scanned_bytes += snippet.len();
            if scanned_bytes > 8 * 1024 * 1024 {
                break;
            }
            if !message_ids.insert(mid.clone()) || !fold_search(&snippet).contains(&needle) {
                continue;
            }
            let old = seen.get(&tid).copied();
            let hits = old.map_or(1, |i| matches[i]["hits"].as_i64().unwrap_or(1) + 1);
            if let Some(i) = old {
                let previous = &matches[i];
                let old_role = previous["source"].as_str().unwrap_or("");
                let better = (role != "user", std::cmp::Reverse(at.as_deref()), &mid)
                    < (
                        old_role != "user",
                        std::cmp::Reverse(previous["messageCreatedAt"].as_str()),
                        &previous["messageId"].as_str().unwrap_or("").to_owned(),
                    );
                if !better {
                    matches[i]["hits"] = json!(hits);
                    continue;
                }
            }
            let Some(thread) = live(&tid) else { continue };
            let i = old.unwrap_or(matches.len());
            seen.insert(tid.clone(), i);
            let found = json!({"threadId":tid,"projectId":thread["projectId"],"source":role,"snippet":search_snippet(&snippet, &needle),"messageCreatedAt":at,"quality":-rank,"hits":hits,"messageId":mid,"threadUpdatedAt":thread["updatedAt"]});
            if i == matches.len() {
                matches.push(found);
            } else {
                matches[i] = found;
            }
        }
    }
    }
    if p["titlesOnly"] == true {
        let needle = fold_search(query);
        let mut q = db
            .prepare("SELECT thread_id, body FROM records WHERE kind='thread' AND length(body)<=65536 ORDER BY rowid DESC LIMIT 4096")
            .map_err(|e| fail(&e.to_string()))?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| fail(&e.to_string()))?;
        let mut scanned_bytes = 0usize;
        for (tid, body) in rows.flatten() {
            scanned_bytes += body.len();
            if scanned_bytes > 8 * 1024 * 1024 {
                break;
            }
            let Ok(t) = serde_json::from_str::<Value>(&body) else {
                continue;
            };
            if t["title"]
                .as_str()
                .is_some_and(|title| fold_search(title).contains(&needle))
                && live(&tid).is_some()
            {
                // A title hit outranks a body hit for the same thread.
                matches.retain(|m| m["threadId"] != tid);
                matches.push(json!({"threadId":tid,"projectId":t["projectId"],"source":"user","snippet":t["title"].as_str().unwrap_or("").chars().take(240).collect::<String>(),"messageCreatedAt":null,"quality":if fold_search(t["title"].as_str().unwrap_or("")) == needle { 4.0 } else if fold_search(t["title"].as_str().unwrap_or(" ")).starts_with(&needle) { 3.0 } else { 2.0 },"hits":1}));
            }
        }
    }
    let mut threads = Vec::new();
    if let Some(before) = p["olderSettledBefore"].as_str() {
        let needle = query.to_lowercase();
        let mut q=db.prepare("SELECT id FROM records WHERE kind='thread' AND json_extract(body,'$.settledAt') < ? ORDER BY rowid DESC LIMIT 10000").map_err(|e|fail(&e.to_string()))?;
        let ids = q
            .query_map([before], |r| r.get::<_, String>(0))
            .map_err(|e| fail(&e.to_string()))?;
        for id in ids.flatten() {
            let Some(t) = live(&id) else { continue };
            let project = events::get(&db, "project", t["projectId"].as_str().unwrap_or(""))
                .unwrap_or(Value::Null);
            let title = [
                t["title"].as_str(),
                t["branch"].as_str(),
                project["title"].as_str(),
                p["environmentLabel"].as_str(),
            ]
            .into_iter()
            .flatten()
            .any(|v| v.to_lowercase().contains(&needle));
            if title && threads.len() < limit {
                threads.push(events::shell_thread(&db, &t).map_err(|e| fail(&e))?);
            }
        }
    }
    matches.sort_by(|a, b| {
        if p["titlesOnly"] == true {
            return b["quality"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["quality"].as_f64().unwrap_or(0.0));
        }
        (a["source"] != "user")
            .cmp(&(b["source"] != "user"))
            .then_with(|| {
                b["threadUpdatedAt"]
                    .as_str()
                    .cmp(&a["threadUpdatedAt"].as_str())
            })
            .then_with(|| a["threadId"].as_str().cmp(&b["threadId"].as_str()))
    });
    matches.truncate(limit);
    for m in &matches {
        if !threads.iter().any(|t| t["id"] == m["threadId"]) {
            if let Some(t) = m["threadId"].as_str().and_then(live) {
                threads.push(events::shell_thread(&db, &t).map_err(|e| fail(&e))?);
            }
        }
    }
    let seq = events::sequence(&db).unwrap_or(0);
    Ok(json!({"matches":matches,"snapshotSequence":seq,"threads":threads}))
}

// ------------------------------------------------------------------ simple reads

pub fn archived_shell(s: &State) -> Reply {
    let db = s.reader.lock().unwrap();
    let result = (|| -> Result<Value, String> {
        let mut q = db.prepare("SELECT body FROM records WHERE kind='thread' AND json_extract(body,'$.deletedAt') IS NULL AND json_extract(body,'$.archivedAt') IS NOT NULL ORDER BY rowid").map_err(|e| e.to_string())?;
        let mut bytes = 1024usize;
        let mut threads = Vec::new();
        for row in q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
        {
            let t: Value = serde_json::from_str(&row.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let t = events::shell_thread(&db, &t)?;
            bytes += serde_json::to_vec(&t).map_err(|e| e.to_string())?.len() + 1;
            if bytes > 7 * 1024 * 1024 {
                return Err("archive exceeds the client reader budget; this protocol has no archive pagination; export or reduce archived history".into());
            }
            threads.push(t);
        }
        let mut projects = Vec::new();
        let mut q = db.prepare("SELECT body FROM records WHERE kind='project' AND json_extract(body,'$.deletedAt') IS NULL ORDER BY rowid").map_err(|e|e.to_string())?;
        for row in q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
        {
            let project: Value = serde_json::from_str(&row.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            bytes += serde_json::to_vec(&project)
                .map_err(|e| e.to_string())?
                .len()
                + 1;
            if bytes > 7 * 1024 * 1024 {
                return Err("archive exceeds the client reader budget; this protocol has no archive pagination; export or reduce archived history".into());
            }
            projects.push(project);
        }
        Ok(
            json!({"schemaVersion":2,"snapshotSequence":events::sequence(&db)?,"projects":projects,"threads":threads}),
        )
    })();
    result.map_err(|m| json!({"_tag":"OrchestrationV2GetShellSnapshotError","message":m}))
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

/// Literal find in transcript messages/plans, ordered like the projected transcript.
/// Rich markdown/context search segmentation is tracked separately in the registry.
// Render block boundaries independently: phrases must not cross paragraphs, table
// cells, context chips or code blocks. Link destinations are never visible text.
fn rendered_segments(text: &str, user: bool, skills: &Value) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut hidden = 0usize;
    let mut literal = 0usize;
    let flush = |current: &mut String, segments: &mut Vec<String>| {
        if !current.trim().is_empty() {
            segments.push(std::mem::take(current));
        } else {
            current.clear();
        }
    };
    for event in Parser::new_ext(
        text,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(Tag::Link { dest_url, .. })
                if user && dest_url.starts_with("t3-context:") =>
            {
                flush(&mut current, &mut segments);
                hidden += 1;
            }
            Event::End(TagEnd::Link) if hidden > 0 => {
                hidden -= 1;
            }
            _ if hidden > 0 => {}
            Event::Start(Tag::Link { .. } | Tag::CodeBlock(_)) => {
                literal += 1;
            }
            Event::End(TagEnd::Link | TagEnd::CodeBlock) => {
                literal = literal.saturating_sub(1);
            }
            Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough)
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {}
            Event::Text(t) => {
                if literal > 0 {
                    current.push_str(&t);
                } else {
                    let mut cursor = 0;
                    for (at, word) in
                        t.split_inclusive(char::is_whitespace)
                            .scan(0, |offset, word| {
                                let at = *offset;
                                *offset += word.len();
                                Some((at, word))
                            })
                    {
                        let token = word.trim_end();
                        if let Some(skill) = token
                            .strip_prefix('$')
                            .and_then(|name| skills.as_array()?.iter().find(|s| s["name"] == name))
                        {
                            current.push_str(&t[cursor..at]);
                            let label = skill["displayName"]
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    skill["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .split(['-', '_', ':'])
                                        .map(|w| {
                                            let mut c = w.chars();
                                            c.next()
                                                .map(|a| {
                                                    a.to_uppercase().collect::<String>()
                                                        + c.as_str()
                                                })
                                                .unwrap_or_default()
                                        })
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                });
                            current.push_str(&label);
                            current.push_str(&word[token.len()..]);
                            cursor = at + word.len();
                        }
                    }
                    current.push_str(&t[cursor..]);
                }
            }
            Event::Code(t) => current.push_str(&t),
            Event::SoftBreak if !user && literal == 0 => current.push(' '),
            Event::SoftBreak | Event::HardBreak | Event::Rule => flush(&mut current, &mut segments),
            Event::Html(t) | Event::InlineHtml(t) if user => current.push_str(&t),
            _ => flush(&mut current, &mut segments),
        }
    }
    flush(&mut current, &mut segments);
    segments
}

pub fn search_thread(s: &State, p: &Value) -> Reply {
    let fail = |message: &str| json!({"_tag":"OrchestrationV2SearchThreadError","cause":message});
    let tid = p["threadId"]
        .as_str()
        .ok_or_else(|| fail("threadId required"))?;
    let query = p["query"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty() && q.chars().count() <= 200)
        .ok_or_else(|| fail("query must be 1-200 characters"))?
        .to_lowercase()
        .replace('ς', "σ");
    let db = s.reader.lock().unwrap();
    events::get(&db, "thread", tid).map_err(|e| fail(&e))?;
    let mut navigation = Vec::new();
    let mut total = 0usize;
    let mut q = db.prepare("SELECT body FROM records WHERE kind='visibleTurnItems' AND thread_id=? AND length(body)<=262144 ORDER BY rowid DESC LIMIT 4096").map_err(|e| fail(&e.to_string()))?;
    let mut budget = 0usize;
    let mut rows: Vec<Value> = q
        .query_map([tid], |r| r.get::<_, String>(0))
        .map_err(|e| fail(&e.to_string()))?
        .flatten()
        .take_while(|body| {
            budget += body.len();
            budget <= 8 * 1024 * 1024
        })
        .filter_map(|body| serde_json::from_str(&body).ok())
        .collect();
    rows.sort_by_key(|r| r["position"].as_i64().unwrap_or(0));
    let folded: std::collections::HashSet<String> = rows
        .iter()
        .filter(|r| r["item"]["type"] == "user_input_request")
        .filter_map(|r| {
            r["item"]["questionAnswer"]["requestId"]
                .as_str()
                .map(|id| format!("async-answer:{id}"))
        })
        .collect();
    for row in rows {
        if row["item"]["type"] == "user_message"
            && row["item"]["messageId"]
                .as_str()
                .is_some_and(|id| folded.contains(id))
        {
            continue;
        }
        let item = &row["item"];
        let text = match item["type"].as_str() {
            Some("user_message" | "assistant_message") => item["text"].as_str(),
            Some("proposed_plan") => item["markdown"].as_str(),
            _ => None,
        };
        let skills = if item["type"] == "proposed_plan" {
            &Value::Null
        } else {
            &p["skills"]
        };
        let count: usize =
            rendered_segments(text.unwrap_or(""), item["type"] == "user_message", skills)
                .iter()
                .map(|s| s.to_lowercase().replace('ς', "σ").matches(&query).count())
                .sum();
        if count == 0 {
            continue;
        }
        let entry = if item["type"] == "proposed_plan" {
            &item["id"]
        } else {
            &item["messageId"]
        };
        navigation
            .push(json!({"entryId":entry,"runId":item["runId"],"startIndex":total,"count":count}));
        total += count;
    }
    let requested = p["index"].as_u64().map(|v| v as usize).unwrap_or_else(|| {
        navigation
            .iter()
            .find(|n| n["entryId"] == p["start"]["entryId"])
            .map(|n| {
                n["startIndex"].as_u64().unwrap() as usize
                    + (p["start"]["occurrence"].as_u64().unwrap_or(0) as usize)
                        .min(n["count"].as_u64().unwrap() as usize)
            })
            .filter(|n| *n < total)
            .unwrap_or(0)
    });
    let active = if p.get("index").is_some() {
        requested.min(total.saturating_sub(1))
    } else if total > 0 {
        (requested as i128 + p["offset"].as_i64().unwrap_or(0) as i128).rem_euclid(total as i128)
            as usize
    } else {
        0
    };
    let selected = navigation.iter().position(|n| {
        active >= n["startIndex"].as_u64().unwrap() as usize
            && active < (n["startIndex"].as_u64().unwrap() + n["count"].as_u64().unwrap()) as usize
    });
    let found = selected.map(|i| {let n=&navigation[i]; json!({"entryId":n["entryId"],"runId":n["runId"],"occurrence":active-n["startIndex"].as_u64().unwrap() as usize})});
    if navigation.len() > 17 {
        let len = navigation.len();
        let center = selected.unwrap_or(0);
        navigation = (0..17)
            .map(|i| navigation[(center + len + i - 8) % len].clone())
            .collect();
    }
    Ok(
        json!({"snapshotSequence":events::sequence(&db).map_err(|e|fail(&e))?,"totalMatches":total,"activeIndex":active,"navigation":navigation,"match":found}),
    )
}

#[cfg(test)]
mod reader_search_tests {
    #[test]
    fn fts_queries_also_keep_the_raw_form_the_index_stores() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE VIRTUAL TABLE t USING fts5(text)")
            .unwrap();
        db.execute("INSERT INTO t(text) VALUES('ＣＡＦＥ con leche')", [])
            .unwrap();
        for query in ["ＣＡＦＥ", "ＣＡＦＥ leche"] {
            let hits: i64 = db
                .query_row(
                    "SELECT count(*) FROM t WHERE t MATCH ?",
                    [super::match_expression(query).unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hits, 1, "{query}");
        }
    }

    use super::*;
    #[test]
    fn search_does_not_take_the_writer_mutex() {
        let root = std::env::temp_dir().join(crate::store::random_id().unwrap());
        let state = State::open(&root).unwrap();
        let writer = state.db.lock().unwrap();
        assert!(search_threads(&state, &json!({"query":"missing"})).is_ok());
        assert!(search_thread(&state, &json!({"threadId":"missing", "query":"x"})).is_err());
        drop(writer);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
