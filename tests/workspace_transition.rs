mod common;
mod support;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use t3_protocol::Connection;
const WAIT: Duration = Duration::from_secs(12);
fn command(c: &Connection, v: Value) -> Value {
    c.rpc("orchestration.dispatchCommand", v).unwrap()
}
fn projection(c: &Connection, id: &str) -> Value {
    t3_protocol::api::orchestration::thread_projection(c, id).unwrap()
}
fn count(s: &o3_server::store::State, sql: &str) -> i64 {
    s.db.lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn observe(root: &Path, label: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(format!("observe-{label}.json"))).unwrap())
        .unwrap()
}
fn auth(v: &Value) -> String {
    v["config"]["headers"]["Authorization"]
        .as_str()
        .or(v["config"]["http_headers"]["Authorization"].as_str())
        .unwrap()
        .into()
}
fn mcp_status(s: &support::Server, token: &str) -> u16 {
    common::http(&s.base,"POST","/mcp",&[("authorization",token),("content-type","application/json"),("accept","application/json")],json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"transition","version":"1"},"capabilities":{}}}).to_string().as_bytes()).0
}
fn send(c: &Connection, tid: &str, text: &str, mode: &str) {
    command(
        c,
        json!({"type":"message.dispatch","commandId":text,"threadId":tid,"messageId":text,"text":text,"attachments":[],"dispatchMode":{"type":mode}}),
    );
}

#[test]
fn active_workspace_transition_resumes_native_history_and_preserves_shared_codex_credentials() {
    for instance in ["claudeAgent", "codex"] {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
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
        let c = s.connect();
        let root = s.dir.join("repo");
        let work = s.dir.join("worktree");
        for p in [&root, &work] {
            std::fs::create_dir_all(p).unwrap();
        }
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":root})).unwrap();
        for tid in ["target", "survivor"] {
            command(
                &c,
                json!({"type":"thread.create","commandId":tid,"threadId":tid,"projectId":"p","title":tid,"modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"}),
            );
        }
        std::fs::write(root.join("keep.txt"), "workspace data").unwrap();
        std::fs::write(root.join("hold-unload"), "").unwrap();
        send(&c, "target", "wait-target", "start_immediately");
        send(&c, "survivor", "wait-survivor", "start_immediately");
        assert!(support::wait_for(WAIT, || projection(&c, "target")
            ["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["status"] == "pending")
            && root.join("observe-wait-survivor.json").exists()));
        let original = observe(&root, "wait-target");
        let sibling = observe(&root, "wait-survivor");
        let native_before =
            projection(&c, "target")["providerThreads"][0]["nativeThreadRef"]["nativeId"].clone();
        assert_eq!(native_before, original["native"]);
        send(&c, "target", "idle-continuation", "queue_after_active");
        let update = json!({"type":"thread.metadata.update","commandId":"bind","threadId":"target","expectedWorktreePath":null,"branch":"feature","worktreePath":work});
        let receipt = command(&c, update.clone());
        assert_eq!(command(&c, update), receipt);
        if instance == "codex" {
            assert!(support::wait_for(WAIT, || root
                .join("unload-pending")
                .exists()));
            assert_eq!(
                count(
                    &s.state,
                    "SELECT count(*) FROM workspace_detaches WHERE attempts>0"
                ),
                1
            );
            assert!(!work.join("observe-idle-continuation.json").exists());
            assert!(projection(&c, "target")["providerSessions"]
                .as_array()
                .unwrap()
                .is_empty());
            assert_eq!(
                mcp_status(&s, &auth(&original)),
                200,
                "shared live process keeps its credential across detach"
            );
        }
        let alive = projection(&c, "survivor");
        let pending = alive["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["status"] == "pending")
            .unwrap();
        command(
            &c,
            json!({"type":"runtime-request.respond","commandId":"approve-sibling","threadId":"survivor","requestId":pending["id"],"decision":"accept"}),
        );
        assert!(support::wait_for(WAIT, || projection(&c, "survivor")
            ["runs"][0]["status"]
            == "completed"));
        std::fs::write(root.join("release-unload"), "").unwrap();
        assert!(support::wait_for(WAIT, || work
            .join("observe-idle-continuation.json")
            .exists()
            && projection(&c, "target")["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["status"] == "completed")));
        let resumed = observe(&work, "idle-continuation");
        assert_eq!(resumed["native"], original["native"]);
        assert_eq!(resumed["cwd"], json!(work));
        assert_eq!(
            std::fs::read_to_string(root.join("keep.txt")).unwrap(),
            "workspace data"
        );
        assert_eq!(count(&s.state, "SELECT count(*) FROM deletion_effects"), 0);
        let p = projection(&c, "target");
        assert_eq!(p["thread"]["worktreePath"], json!(work));
        assert!(p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["status"] == "interrupted"));
        assert!(p["runtimeRequests"].as_array().unwrap().iter().all(
            |r| r["status"] != "pending" && r["responseCapability"]["type"] == "not_resumable"
        ));
        if instance == "codex" {
            assert_eq!(auth(&original), auth(&resumed));
            assert_eq!(resumed["pid"], sibling["pid"]);
            assert_eq!(mcp_status(&s, &auth(&original)), 200);
            let unloaded = std::fs::read_to_string(root.join("unloaded.jsonl")).unwrap();
            assert!(unloaded.contains(original["native"].as_str().unwrap()));
            assert!(!unloaded.contains(sibling["native"].as_str().unwrap()));
        } else {
            assert_ne!(resumed["pid"], original["pid"]);
            assert_eq!(mcp_status(&s, &auth(&original)), 401);
            let args = resumed["argv"].as_array().unwrap();
            let flag = args.iter().position(|a| a == "--resume").unwrap();
            assert_eq!(args[flag + 1], original["native"]);
        }
        // An idle cached session must also be detached when returning to the project root.
        command(
            &c,
            json!({"type":"thread.metadata.update","commandId":"return","threadId":"target","expectedWorktreePath":work,"expectedBranch":"feature","branch":null,"worktreePath":null}),
        );
        assert!(support::wait_for(WAIT, || count(
            &s.state,
            "SELECT count(*) FROM workspace_detaches WHERE released_at IS NULL"
        ) == 0));
        send(&c, "target", "idle-return", "start_immediately");
        assert!(support::wait_for(WAIT, || root
            .join("observe-idle-return.json")
            .exists()
            && projection(&c, "target")["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["status"] == "completed")
                .count()
                == 2));
        assert_eq!(observe(&root, "idle-return")["native"], original["native"]);
        if instance == "codex" {
            // A cancelled resume may acknowledge after detach completed. Keeping its
            // process credential must not make that late binding look loaded.
            std::fs::write(root.join("hold-binding"), "").unwrap();
            send(&c, "target", "wait-target-again", "start_immediately");
            assert!(support::wait_for(WAIT, || root
                .join("observe-binding-pending.json")
                .exists()));
            command(
                &c,
                json!({"type":"thread.metadata.update","commandId":"late-binding","threadId":"target","expectedWorktreePath":null,"worktreePath":work}),
            );
            assert!(support::wait_for(WAIT, || count(
                &s.state,
                "SELECT count(*) FROM workspace_detaches WHERE released_at IS NULL"
            ) == 0));
            let before = std::fs::read_to_string(root.join("unloaded.jsonl"))
                .unwrap()
                .lines()
                .count();
            std::fs::write(root.join("release-binding"), "").unwrap();
            assert!(support::wait_for(WAIT, || std::fs::read_to_string(
                root.join("unloaded.jsonl")
            )
            .unwrap()
            .lines()
            .count()
                > before));
            assert_eq!(mcp_status(&s, &auth(&original)), 200);
            send(&c, "target", "idle-return-again", "start_immediately");
            assert!(support::wait_for(WAIT, || work
                .join("observe-idle-return.json")
                .exists()));
            assert_eq!(observe(&work, "idle-return")["native"], original["native"]);
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            claude.shutdown().await;
            codex.shutdown().await;
        });
    }
}
