//! `agentSessions.scan` / `agentSessions.import` over synthetic Claude and Codex transcripts. The
//! directories come from CLAUDE_CONFIG_DIR / CODEX_HOME; the user's real history is never read.
mod common;
use common::*;
use serde_json::{json, Value};

fn lines(items: &[Value]) -> String {
    items.iter().map(|v| v.to_string() + "\n").collect()
}

#[test]
fn scan_groups_transcripts_by_directory_and_import_creates_resumable_threads() {
    let history = std::env::temp_dir().join(format!("o3-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&history);
    let (claude, codex) = (history.join("claude"), history.join("codex"));
    // SAFETY: set before the test server (and its worker threads) exists; this file has one test.
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        std::env::set_var("CODEX_HOME", &codex);
    }
    let e = setup("sessions");
    let work = e.dir("work");
    let other = e.dir("other");
    let w = work.to_str().unwrap();

    write(
        &claude.join("projects/-work/aaaa.jsonl"),
        lines(&[
            json!({"type":"summary","aiTitle":"Fix the parser"}),
            json!({"type":"user","cwd":w,"sessionId":"sess-claude","timestamp":"2026-10-30T10:00:00.000Z","message":{"role":"user","content":"Please fix the parser"}}),
            json!({"type":"user","cwd":w,"sessionId":"sess-claude","isMeta":true,"timestamp":"2026-10-30T10:00:01.000Z","message":{"role":"user","content":"meta noise"}}),
            json!({"type":"assistant","cwd":w,"sessionId":"sess-claude","timestamp":"2026-10-30T10:00:02.000Z","message":{"role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Fixed it."},{"type":"tool_use","name":"Bash"}]}}),
        ]),
    );
    write(
        &claude.join("projects/-work/agent-sub.jsonl"),
        lines(&[json!({"cwd":w,"sessionId":"sub"})]),
    );
    write(
        &codex.join("sessions/2026/10/30/rollout-1.jsonl"),
        lines(&[
            json!({"type":"session_meta","payload":{"id":"sess-codex","cwd":w}}),
            json!({"type":"turn_context","payload":{"model":"gpt-5-codex"}}),
            json!({"type":"response_item","timestamp":"2026-10-30T11:00:00.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}),
            json!({"type":"response_item","timestamp":"2026-10-30T11:00:01.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Add logging"}]}}),
            json!({"type":"response_item","timestamp":"2026-10-30T11:00:02.000Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Logging added."}]}}),
        ]),
    );
    write(
        &codex.join("sessions/2026/10/30/rollout-2.jsonl"),
        lines(&[
            json!({"type":"session_meta","payload":{"id":"sess-other","cwd":other.to_str().unwrap()}}),
        ]),
    );
    write(&codex.join("sessions/2026/10/30/notes.jsonl"), "{}\n");
    // An old transcript is scanned but not imported (30-day window).
    let old = codex.join("sessions/2020/01/01/rollout-old.jsonl");
    write(
        &old,
        lines(&[
            json!({"type":"session_meta","payload":{"id":"sess-old","cwd":w}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ancient"}]}}),
        ]),
    );
    std::process::Command::new("touch")
        .args(["-d", "2020-01-01", old.to_str().unwrap()])
        .status()
        .unwrap();

    let c = &e.conn;
    e.project("pw", &work);
    let scan = c.rpc("agentSessions.scan", json!({})).unwrap();
    let cands = scan["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 2, "two directories: {scan}");
    let mine = cands.iter().find(|c| c["path"] == w).unwrap();
    assert_eq!(mine["projectId"], "pw");
    assert_eq!(
        mine["threadCount"], 3,
        "agent-* sidechains and non-rollout files are not sessions"
    );
    assert_eq!(mine["sources"], json!(["claudeAgent", "codex"]));
    assert_eq!(mine["alreadyImported"], false);
    let unknown = cands
        .iter()
        .find(|c| c["path"] == other.to_str().unwrap())
        .unwrap();
    assert!(unknown.get("projectId").is_none() && unknown["title"] == "other");
    assert!(scan["scannedAt"].is_string());

    // Guard rails.
    assert!(c
        .rpc("agentSessions.import", json!({"projectId":"nope"}))
        .is_err());
    assert!(c
        .rpc(
            "agentSessions.import",
            json!({"projectId":"pw","expectedWorkspaceRoot":"/elsewhere"})
        )
        .is_err());

    let result = c
        .rpc(
            "agentSessions.import",
            json!({"projectId":"pw","expectedWorkspaceRoot":w}),
        )
        .unwrap();
    assert_eq!(
        result,
        json!({"importedCount":2,"skippedCount":0}),
        "the old session is outside the window"
    );
    let claude_thread = c
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"imported-claudeAgent-sess-claude"}),
        )
        .unwrap();
    assert_eq!(claude_thread["thread"]["title"], "Fix the parser");
    assert_eq!(
        claude_thread["thread"]["activeProviderThreadId"], "sess-claude",
        "the native id lets the provider resume"
    );
    assert_eq!(claude_thread["thread"]["providerInstanceId"], "claudeAgent");
    let texts: Vec<_> = claude_thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["role"].as_str().unwrap(), m["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        texts,
        [
            ("user", "Please fix the parser"),
            ("assistant", "Fixed it.")
        ]
    );
    let codex_thread = c
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"imported-codex-sess-codex"}),
        )
        .unwrap();
    assert_eq!(codex_thread["thread"]["codexNativeThreadId"], "sess-codex");
    assert_eq!(
        codex_thread["thread"]["modelSelection"]["model"],
        "gpt-5-codex"
    );
    assert_eq!(
        codex_thread["messages"].as_array().unwrap().len(),
        2,
        "environment blocks are not messages"
    );

    // Importing again is idempotent; the scan now reports the directory as imported.
    let again = c
        .rpc("agentSessions.import", json!({"projectId":"pw"}))
        .unwrap();
    assert_eq!(again, json!({"importedCount":0,"skippedCount":2}));
    let scan = c.rpc("agentSessions.scan", json!({})).unwrap();
    let mine = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == w)
        .unwrap()
        .clone();
    assert_eq!(
        mine["alreadyImported"], false,
        "the old session is still not imported"
    );
    // The transcripts themselves are never modified.
    assert!(
        std::fs::read_to_string(claude.join("projects/-work/aaaa.jsonl"))
            .unwrap()
            .contains("meta noise")
    );
    let _ = std::fs::remove_dir_all(&history);
}
