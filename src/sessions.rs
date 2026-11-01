//! `agentSessions.scan` / `agentSessions.import`: find the Claude Code and Codex transcripts that
//! already exist on this machine and turn the ones for a project into threads.
//!
//! Sources: `$CLAUDE_CONFIG_DIR` (default `~/.claude`) `/projects/*/*.jsonl` and `$CODEX_HOME`
//! (default `~/.codex`) `/sessions/**/rollout-*.jsonl`. Reading is bounded (files, bytes and line
//! length), streaming and read-only; nothing is cached between calls.
use crate::{
    events,
    p2::{invalid, str_field, Reply},
    store::{iso, State},
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

const MAX_TRANSCRIPTS: usize = 5000;
const HEAD_BYTES: usize = 32 * 1024;
const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MESSAGES: usize = 200;
const MAX_IMPORT: usize = 100;
const RECENT_MS: i64 = 30 * 24 * 3600 * 1000;
const MAX_TEXT: usize = 20_000;

#[derive(Default)]
pub struct Cache;

pub fn handles(tag: &str) -> bool {
    matches!(tag, "agentSessions.scan" | "agentSessions.import")
}

pub async fn call(s: &Arc<State>, tag: &str, p: &Value) -> Reply {
    if tag == "agentSessions.scan" {
        let s = s.clone();
        let (found, truncated) = tokio::task::spawn_blocking(discover)
            .await
            .map_err(|e| scan_error(&e.to_string()))?;
        return Ok(scan_result(&s, found, truncated));
    }
    let project_id = str_field(p, "projectId")?.to_owned();
    let project = events::get(&s.db.lock().unwrap(), "project", &project_id)
        .ok()
        .filter(|v| v["deletedAt"].is_null())
        .ok_or_else(|| json!({"_tag":"AgentSessionImportProjectNotFoundError","projectId":project_id}))?;
    let root = project["workspaceRoot"].as_str().unwrap_or("").to_owned();
    if p["expectedWorkspaceRoot"].as_str().is_some_and(|e| e != root) {
        return Err(json!({"_tag":"AgentSessionImportProjectChangedError","projectId":project_id}));
    }
    if root.is_empty() {
        return Err(invalid("project has no workspace root"));
    }
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || import(&s2, &project_id, &root))
        .await
        .map_err(|e| scan_error(&e.to_string()))?
}

fn scan_error(cause: &str) -> Value {
    json!({"_tag":"AgentSessionScanError","operation":"read-projects","cause":cause,"message":"Failed to scan agent sessions during read-projects."})
}

#[derive(Clone)]
struct Transcript {
    source: &'static str,
    instance: &'static str,
    path: PathBuf,
    cwd: String,
    session: String,
    mtime_ms: i64,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
fn claude_root() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".claude")))
        .map(|p| p.join("projects"))
}
fn codex_root() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".codex")))
        .map(|p| p.join("sessions"))
}

fn mtime_ms(p: &Path) -> Option<i64> {
    let m = p.metadata().ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// Lines of `path`, each at most `max_line` bytes (longer ones are skipped, not buffered), reading
/// at most `max_bytes`. `f` returns false to stop.
fn for_each_line(path: &Path, max_bytes: u64, max_line: usize, mut f: impl FnMut(&[u8]) -> bool) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let mut r = std::io::BufReader::with_capacity(64 * 1024, file.take(max_bytes));
    let (mut line, mut skipping) = (Vec::new(), false);
    loop {
        use std::io::BufRead;
        let (done, used) = {
            let Ok(buf) = r.fill_buf() else { return };
            if buf.is_empty() {
                if !line.is_empty() && !skipping {
                    f(&line);
                }
                return;
            }
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    if !skipping && line.len() + i <= max_line {
                        line.extend_from_slice(&buf[..i]);
                        if !f(&line) {
                            return;
                        }
                    }
                    line.clear();
                    skipping = false;
                    (true, i + 1)
                }
                None => {
                    if !skipping {
                        if line.len() + buf.len() > max_line {
                            skipping = true;
                            line.clear();
                        } else {
                            line.extend_from_slice(buf);
                        }
                    }
                    (false, buf.len())
                }
            }
        };
        r.consume(used);
        let _ = done;
    }
}

fn head_cwd(path: &Path, source: &str) -> Option<(String, Option<String>)> {
    let mut found = None;
    let mut bytes = 0;
    for_each_line(path, HEAD_BYTES as u64, HEAD_BYTES, |line| {
        bytes += line.len();
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return bytes < HEAD_BYTES };
        let (cwd, id) = if source == "codex" {
            if v["type"] != "session_meta" {
                return true;
            }
            (v["payload"]["cwd"].as_str(), v["payload"]["id"].as_str())
        } else {
            (v["cwd"].as_str(), v["sessionId"].as_str())
        };
        if let Some(cwd) = cwd.filter(|c| c.starts_with('/')) {
            found = Some((cwd.trim_end_matches('/').to_owned(), id.map(str::to_owned)));
            return false;
        }
        true
    });
    found
}

fn discover() -> (Vec<Transcript>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    if let Some(root) = claude_root() {
        'outer: for dir in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            if !dir.path().is_dir() {
                continue;
            }
            for f in std::fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                let path = f.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.ends_with(".jsonl") || name.starts_with("agent-") {
                    continue;
                }
                if out.len() >= MAX_TRANSCRIPTS {
                    truncated = true;
                    break 'outer;
                }
                if let (Some((cwd, id)), Some(m)) = (head_cwd(&path, "claude"), mtime_ms(&path)) {
                    let session = id.unwrap_or_else(|| name.trim_end_matches(".jsonl").to_owned());
                    out.push(Transcript { source: "claudeAgent", instance: "claudeAgent", path, cwd, session, mtime_ms: m });
                }
            }
        }
    }
    if let Some(root) = codex_root() {
        let mut stack = vec![(root, 0)];
        let mut count = 0;
        'walk: while let Some((dir, depth)) = stack.pop() {
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = f.path();
                if path.is_dir() {
                    if depth < 4 {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                    continue;
                }
                if count >= MAX_TRANSCRIPTS {
                    truncated = true;
                    break 'walk;
                }
                count += 1;
                if let (Some((cwd, id)), Some(m)) = (head_cwd(&path, "codex"), mtime_ms(&path)) {
                    let session = id.unwrap_or_else(|| name.trim_end_matches(".jsonl").to_owned());
                    out.push(Transcript { source: "codex", instance: "codex", path, cwd, session, mtime_ms: m });
                }
            }
        }
    }
    (out, truncated)
}

fn thread_id(t: &Transcript) -> String {
    format!("imported-{}-{}", t.instance, t.session)
}

fn scan_result(s: &State, found: Vec<Transcript>, truncated: bool) -> Value {
    let projects: Vec<Value> = {
        let db = s.db.lock().unwrap();
        db.prepare("SELECT body FROM records WHERE kind='project'")
            .ok()
            .map(|mut q| {
                q.query_map([], |r| r.get::<_, String>(0))
                    .map(|rows| rows.flatten().filter_map(|b| serde_json::from_str(&b).ok()).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    };
    let mut groups: BTreeMap<String, Vec<&Transcript>> = BTreeMap::new();
    for t in &found {
        groups.entry(t.cwd.clone()).or_default().push(t);
    }
    let mut candidates: Vec<Value> = groups
        .into_iter()
        .map(|(cwd, list)| {
            let mut sources: Vec<&str> = list.iter().map(|t| t.source).collect();
            sources.sort();
            sources.dedup();
            let last = list.iter().map(|t| t.mtime_ms).max();
            let project = projects.iter().find(|p| p["deletedAt"].is_null() && p["workspaceRoot"] == cwd.as_str());
            let imported = project.is_some() && {
                let db = s.db.lock().unwrap();
                list.iter().all(|t| events::get(&db, "thread", &thread_id(t)).is_ok())
            };
            let title = cwd.rsplit('/').find(|c| !c.is_empty()).unwrap_or(&cwd).to_owned();
            let mut c = json!({"path":cwd,"title":title,"sources":sources,"threadCount":list.len(),
                "lastActiveAt":last.map(iso),"alreadyImported":imported});
            if let Some(id) = project.and_then(|p| p["id"].as_str()) {
                c["projectId"] = json!(id);
            }
            c
        })
        .collect();
    candidates.sort_by(|a, b| b["lastActiveAt"].as_str().cmp(&a["lastActiveAt"].as_str()));
    let mut out = json!({"candidates":candidates,"scannedAt":iso(crate::store::now_ms())});
    if truncated {
        out["truncated"] = json!(true);
    }
    out
}

struct Parsed {
    title: Option<String>,
    model: Option<String>,
    messages: Vec<(bool, String, String)>,
}

fn text_of(content: &Value, kinds: &[&str]) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter(|b| b["type"].as_str().is_some_and(|t| kinds.contains(&t)))
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse(t: &Transcript) -> Parsed {
    let mut out = Parsed { title: None, model: None, messages: Vec::new() };
    let mut ring = std::collections::VecDeque::new();
    for_each_line(&t.path, MAX_FILE_BYTES, MAX_LINE, |line| {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return true };
        let at = v["timestamp"].as_str().unwrap_or("").to_owned();
        let (user, text) = if t.source == "codex" {
            if v["type"] == "turn_context" && out.model.is_none() {
                out.model = v["payload"]["model"].as_str().map(str::to_owned);
            }
            if v["type"] != "response_item" || v["payload"]["type"] != "message" {
                return true;
            }
            let role = v["payload"]["role"].as_str().unwrap_or("");
            if !matches!(role, "user" | "assistant") {
                return true;
            }
            let text = text_of(&v["payload"]["content"], &["input_text", "output_text"]);
            // Codex prepends environment and instruction blocks as user messages.
            if role == "user" && (text.starts_with('<') || text.starts_with("# AGENTS.md")) {
                return true;
            }
            (role == "user", text)
        } else {
            if let Some(title) = v["customTitle"].as_str().or_else(|| v["aiTitle"].as_str()) {
                out.title = Some(title.to_owned());
            }
            if v["isSidechain"] == true || v["isMeta"] == true || v["isCompactSummary"] == true {
                return true;
            }
            let role = v["message"]["role"].as_str().or_else(|| v["type"].as_str()).unwrap_or("");
            if !matches!(role, "user" | "assistant") {
                return true;
            }
            if role == "assistant" && out.model.is_none() {
                out.model = v["message"]["model"].as_str().filter(|m| !m.starts_with('<')).map(str::to_owned);
            }
            (role == "user", text_of(&v["message"]["content"], &["text"]))
        };
        let text: String = text.trim().chars().take(MAX_TEXT).collect();
        if text.is_empty() || (user && text.starts_with("<command-")) {
            return true;
        }
        if ring.len() == MAX_MESSAGES {
            ring.pop_front();
        }
        ring.push_back((user, text, at));
        true
    });
    out.messages = ring.into();
    out
}

fn import(s: &Arc<State>, project_id: &str, root: &str) -> Reply {
    let (mut found, _) = discover();
    let now = crate::store::now_ms();
    found.retain(|t| t.cwd == root.trim_end_matches('/') && now - t.mtime_ms <= RECENT_MS);
    found.sort_by_key(|t| std::cmp::Reverse(t.mtime_ms));
    found.truncate(MAX_IMPORT);
    let (mut imported, mut skipped) = (0u64, 0u64);
    for t in found {
        let tid = thread_id(&t);
        if events::get(&s.db.lock().unwrap(), "thread", &tid).is_ok() {
            skipped += 1;
            continue;
        }
        let parsed = parse(&t);
        let Some(first_user) = parsed.messages.iter().find(|m| m.0) else {
            skipped += 1;
            continue;
        };
        let title: String = parsed
            .title
            .clone()
            .unwrap_or_else(|| first_user.1.lines().next().unwrap_or("").to_owned())
            .chars()
            .take(80)
            .collect();
        let model = parsed.model.clone().unwrap_or_else(|| {
            if t.source == "codex" { "gpt-5-codex".into() } else { "haiku".into() }
        });
        let created = events::dispatch(
            s,
            &json!({"type":"thread.create","commandId":format!("import-{tid}"),"threadId":tid,"projectId":project_id,
                "title":if title.trim().is_empty(){"Imported session"}else{title.trim()},
                "modelSelection":{"instanceId":t.instance,"model":model,"options":[]},
                "runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null}),
        );
        if let Err(e) = created {
            return Err(json!({"_tag":"AgentSessionScanError","operation":"read-projects","cause":e,"message":e}));
        }
        write_messages(s, &tid, &t, &parsed)?;
        imported += 1;
    }
    s.notify();
    Ok(json!({"importedCount":imported,"skippedCount":skipped}))
}

fn write_messages(s: &State, tid: &str, t: &Transcript, parsed: &Parsed) -> Result<(), Value> {
    let fail = |e: String| json!({"_tag":"AgentSessionScanError","operation":"read-projects","cause":e,"message":e});
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| fail(e.to_string()))?;
    let mut thread = events::get(&tx, "thread", tid).map_err(fail)?;
    let key = if t.source == "codex" { "codexNativeThreadId" } else { "activeProviderThreadId" };
    thread[key] = json!(t.session);
    thread["createdAt"] = json!(iso(t.mtime_ms));
    thread["updatedAt"] = json!(iso(t.mtime_ms));
    events::put(&tx, "thread", tid, tid, &thread).map_err(fail)?;
    for (i, (user, text, at)) in parsed.messages.iter().enumerate() {
        let at = if at.is_empty() { iso(t.mtime_ms) } else { at.clone() };
        let id = format!("{tid}-m{i}");
        let m = json!({"id":id,"threadId":tid,"runId":null,"nodeId":null,"role":if *user{"user"}else{"assistant"},"text":text,
            "attachments":[],"streaming":false,"createdBy":if *user{"user"}else{"agent"},"creationSource":"web","createdAt":at,"updatedAt":at});
        events::put(&tx, "messages", &id, tid, &m).map_err(fail)?;
        events::append(&tx, tid, "message.updated", m.clone(), &thread).map_err(fail)?;
        let item = events::message_item(&tx, tid, &m).map_err(fail)?;
        events::append(&tx, tid, "turn-item.updated", item, &thread).map_err(fail)?;
    }
    tx.commit().map_err(|e| fail(e.to_string()))?;
    let _ = db.query_row("SELECT 1", [], |_| Ok(())).optional();
    Ok(())
}
