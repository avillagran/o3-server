mod common;
mod support;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_protocol::Connection;
const WAIT: Duration = Duration::from_secs(12);
fn command(c: &Connection, p: Value) -> Value {
    c.rpc("orchestration.dispatchCommand", p).unwrap()
}
fn projection(c: &Connection) -> Value {
    t3_protocol::api::orchestration::thread_projection(c, "t").unwrap()
}
fn count(s: &support::Server, sql: &str) -> i64 {
    s.state
        .db
        .lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn put(s: &support::Server, kind: &str, id: &str, v: Value) {
    s.state.db.lock().unwrap().execute("INSERT INTO records VALUES(?1,?2,'t',?3) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",rusqlite::params![kind,id,v.to_string()]).unwrap();
}
fn http(base: &str, auth: &str, sid: Option<&str>, method: &str, params: Value) -> (String, Value) {
    let mut h = vec![
        ("authorization", auth),
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    if let Some(sid) = sid {
        h.push(("mcp-session-id", sid));
    }
    let (code, h, b) = common::http(
        base,
        "POST",
        "/mcp",
        &h,
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&b));
    (h, serde_json::from_slice(&b).unwrap())
}
struct Fixture {
    s: support::Server,
    c: Connection,
    root: PathBuf,
    codex: Arc<o3_server::codex::CodexProvider>,
    claude: Arc<o3_server::claude::ClaudeProvider>,
    auth: String,
    sid: String,
}
impl Fixture {
    fn new(instance: &str) -> Self {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/restart-peer.py");
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer.clone(), None));
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer));
        let s = support::start(|s| {
            std::fs::write(
                s.dir.join("settings.json"),
                r#"{"enableAgentBrowserAccess":false}"#,
            )
            .unwrap();
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let root = s.dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["commit", "--allow-empty", "-m", "initial"]);
        let c = s.connect();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":root})).unwrap();
        command(
            &c,
            json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"t","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"}),
        );
        command(
            &c,
            json!({"type":"message.dispatch","commandId":"initial","threadId":"t","messageId":"initial","text":"hold initial","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(WAIT, || projection(&c)["runs"][0]
            ["status"]
            == "running"
            && root.join("restart-observe.json").exists()));
        let observed: Value =
            serde_json::from_slice(&std::fs::read(root.join("restart-observe.json")).unwrap())
                .unwrap();
        let auth: String = observed["config"]["headers"]["Authorization"]
            .as_str()
            .or(observed["config"]["http_headers"]["Authorization"].as_str())
            .unwrap()
            .into();
        let (h, _) = http(
            &s.base,
            &auth,
            None,
            "initialize",
            json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"restart-test","version":"1"},"capabilities":{}}),
        );
        let sid = h
            .lines()
            .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
            .unwrap()
            .into();
        Self {
            s,
            c,
            root,
            codex,
            claude,
            auth,
            sid,
        }
    }
    fn restart(&self, id: &str) -> Value {
        http(&self.s.base,&self.auth,Some(&self.sid),"tools/call",json!({"name":"t3_thread_send","arguments":{"threadId":"t","message":"replacement","mode":"restart","clientRequestId":id}})).1
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            self.codex.shutdown().await;
            self.claude.shutdown().await;
        });
    }
}
#[test]
fn mcp_restart_preserves_run_and_native_identity_waits_for_terminal_and_fences_old_attempt() {
    let f = Fixture::new("codex");
    assert!(support::wait_for(WAIT, || projection(&f.c)["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "command_execution")));
    let before = projection(&f.c);
    let rid = before["runs"][0]["id"].clone();
    let old = before["runs"][0]["activeAttemptId"].clone();
    let native = before["providerThreads"][0]["nativeThreadRef"]["nativeId"].clone();
    f.s.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.providerHandoff','HISTORICAL-MUST-NOT-RESEED') WHERE kind='thread' AND id='t'",[]).unwrap();
    std::fs::write(f.root.join("hold-interrupt"), "").unwrap();
    // A queued message must not hide the live owner from native MCP admission.
    command(
        &f.c,
        json!({"type":"message.dispatch","commandId":"queued","threadId":"t","messageId":"queued","text":"queued later","attachments":[],"dispatchMode":{"type":"queue_after_active"}}),
    );
    put(
        &f.s,
        "subagents",
        "owned",
        json!({"id":"owned","threadId":"t","runId":rid,"origin":"app_owned","status":"running"}),
    );
    let result = f.restart("one");
    assert_eq!(result["result"]["isError"], false, "{result}");
    let result = &result["result"]["structuredContent"];
    assert_eq!(result["runId"], rid);
    assert_eq!(result["delivery"], "restarted");
    assert!(support::wait_for(WAIT, || f
        .root
        .join("interrupt-seen")
        .exists()));
    assert_eq!(
        std::fs::read_to_string(f.root.join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let during = projection(&f.c);
    assert_eq!(during["runs"].as_array().unwrap().len(), 2);
    assert_eq!(during["runs"][0]["status"], "starting");
    assert_ne!(during["runs"][0]["activeAttemptId"], old);
    assert_eq!(during["runs"][1]["status"], "queued");
    assert!(during["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == "owned" && t["status"] == "running"));
    let replay = f.restart("one");
    assert_eq!(replay["result"]["isError"], false, "{replay}");
    assert_eq!(count(&f.s, "SELECT count(*) FROM run_restarts"), 1);
    std::fs::write(f.root.join("release-interrupt"), "").unwrap();
    assert!(support::wait_for(WAIT, || projection(&f.c)["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["status"] == "completed")
        .count()
        == 2));
    let p = projection(&f.c);
    assert_eq!(p["runs"][0]["id"], rid);
    assert_eq!(
        p["providerThreads"][0]["nativeThreadRef"]["nativeId"],
        native
    );
    assert_eq!(
        p["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["runId"] == rid)
            .count(),
        2
    );
    assert!(p["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"] == old && a["status"] == "superseded"));
    assert!(p["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["attemptOrdinal"] == 2
            && a["reason"] == "steering_restart"
            && a["status"] == "completed"));
    assert!(!p.to_string().contains("STALE-"), "{p}");
    assert!(p["runtimeRequests"].as_array().unwrap().is_empty());
    assert_eq!(count(&f.s,"SELECT count(*) FROM records WHERE kind='turnItems' AND json_extract(body,'$.type')='command_execution'"),3);
    assert_eq!(
        count(
            &f.s,
            "SELECT count(*) FROM run_restarts WHERE completed_at IS NOT NULL"
        ),
        1
    );
    let starts: Vec<Value> = std::fs::read_to_string(f.root.join("starts.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let ordinals: Vec<i64> = p["providerTurns"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["ordinal"].as_i64())
        .collect();
    assert_eq!(ordinals, vec![1, 2, 3]);
    assert_eq!(starts.len(), 3);
    assert_eq!(starts[1]["input"][0]["text"], "replacement");
    assert_eq!(starts[2]["input"][0]["text"], "queued later");
    let observed: Value =
        serde_json::from_slice(&std::fs::read(f.root.join("restart-observe.json")).unwrap())
            .unwrap();
    assert_eq!(observed["config"]["http_headers"]["Authorization"], f.auth);
    assert_eq!(
        count(
            &f.s,
            "SELECT count(*) FROM records WHERE kind='checkpoints' AND id LIKE '%-baseline'"
        ),
        3
    );
}
#[test]
fn restart_receipt_rollback_does_not_interrupt_and_same_instance_selection_is_applied() {
    let f = Fixture::new("codex");
    let p = projection(&f.c);
    let rid = &p["runs"][0]["id"];
    let c = json!({"type":"message.dispatch","commandId":"restart","threadId":"t","messageId":"replacement","text":"replacement","attachments":[],"modelSelection":{"instanceId":"codex","model":"new-model"},"dispatchMode":{"type":"restart_active","targetRunId":rid}});
    f.s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_restart BEFORE INSERT ON receipts WHEN NEW.command_id='restart' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(f.c.rpc("orchestration.dispatchCommand", c.clone()).is_err());
    assert_eq!(count(&f.s, "SELECT count(*) FROM run_restarts"), 0);
    assert_eq!(projection(&f.c)["attempts"].as_array().unwrap().len(), 1);
    assert!(!f.root.join("interrupt-seen").exists());
    f.s.state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_restart")
        .unwrap();
    let receipt = command(&f.c, c.clone());
    assert!(support::wait_for(WAIT, || projection(&f.c)["runs"][0]
        ["status"]
        == "completed"));
    assert_eq!(command(&f.c, c), receipt);
    assert_eq!(projection(&f.c)["runs"].as_array().unwrap().len(), 1);
    let starts: Vec<Value> = std::fs::read_to_string(f.root.join("starts.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[1]["model"], "new-model");
}
#[test]
fn unsupported_claude_and_maintenance_are_refused_and_interrupt_failure_is_explicit() {
    let f = Fixture::new("claudeAgent");
    let denied = f.restart("claude");
    assert_eq!(denied["result"]["isError"], true);
    assert!(denied.to_string().contains("interrupt-and-restart"));
    assert_eq!(count(&f.s, "SELECT count(*) FROM run_restarts"), 0);
    let f = Fixture::new("codex");
    let p = projection(&f.c);
    let rid = &p["runs"][0]["id"];
    for text in ["/compact", "/logout", "/goal do something"] {
        assert!(f.c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":text,"threadId":"t","messageId":text,"text":text,"attachments":[],"dispatchMode":{"type":"restart_active","targetRunId":rid}})).is_err());
    }
    std::fs::write(f.root.join("fail-interrupt"), "").unwrap();
    let sent = f.restart("fail");
    assert_eq!(sent["result"]["isError"], false, "{sent}");
    assert!(support::wait_for(WAIT, || projection(&f.c)["runs"][0]
        ["status"]
        == "failed"));
    assert_eq!(
        std::fs::read_to_string(f.root.join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(projection(&f.c)["runs"][0]["error"]
        .as_str()
        .unwrap()
        .contains("acknowledgement"));
    let access =
        f.s.state
            .issue_mcp_session("full-access", 60000, "outside")
            .unwrap();
    let auth = format!("Bearer {access}");
    let (h, _) = http(
        &f.s.base,
        &auth,
        None,
        "initialize",
        json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"outside","version":"1"},"capabilities":{}}),
    );
    let sid = h
        .lines()
        .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
        .unwrap();
    let idle=http(&f.s.base,&auth,Some(sid),"tools/call",json!({"name":"t3_thread_send","arguments":{"threadId":"t","message":"idle restart","mode":"restart"}})).1;
    assert_eq!(idle["result"]["isError"], true);
}

#[test]
fn stop_or_delete_while_restart_waits_never_starts_the_replacement() {
    for action in ["thread.stop", "thread.delete", "thread.archive"] {
        let f = Fixture::new("codex");
        std::fs::write(f.root.join("hold-interrupt"), "").unwrap();
        let reply = f.restart("stop");
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        assert!(support::wait_for(WAIT, || f
            .root
            .join("interrupt-seen")
            .exists()));
        command(
            &f.c,
            json!({"type":action,"commandId":"stop","threadId":"t"}),
        );
        std::fs::write(f.root.join("release-interrupt"), "").unwrap();
        assert!(support::wait_for(WAIT, || count(
            &f.s,
            "SELECT count(*) FROM run_restarts WHERE completed_at IS NOT NULL"
        ) == 1));
        assert_eq!(
            std::fs::read_to_string(f.root.join("starts.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(
            projection(&f.c)["runs"][0]["status"],
            if action == "thread.stop" {
                "interrupted"
            } else {
                "cancelled"
            }
        );
    }
}
