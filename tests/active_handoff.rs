mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use t3_protocol::Connection;
fn projection(c: &Connection) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap()
}
fn fixture() -> (support::Server, Connection, std::path::PathBuf) {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            )));
    });
    let c = s.connect();
    let root = s.dir.join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join(".selection-fixture"), "").unwrap();
    let home = s.dir.join("shared");
    std::fs::create_dir(&home).unwrap();
    let peer =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selection-peer.py");
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":home}},"alternate":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":home}},"claudeAgent":{"driver":"claudeAgent","enabled":true,"config":{"binaryPath":peer,"homePath":s.dir.join("claude")}}}}})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Active transfer","workspaceRoot":root})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Active transfer","modelSelection":{"instanceId":"codex","model":"one"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    (s, c, root)
}
fn held(c: &Connection) {
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":"HOLD SOURCE-HISTORY-WORD","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || {
        let p = projection(c);
        p["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["status"] == "running")
            && p["messages"].as_array().unwrap().iter().any(|m| {
                m["role"] == "assistant"
                    && m["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("SOURCE-HISTORY-WORD"))
            })
    }));
}
fn trace(root: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("selection-trace.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[test]
fn active_cross_instance_restart_preserves_the_run_and_transfers_or_resumes_native_context() {
    for (target, mode) in [
        ("claudeAgent", "restart_active"),
        ("alternate", "restart_active"),
        ("claudeAgent", "steer_active"),
        ("alternate", "steer_active"),
        ("claudeAgent", "auto"),
        ("alternate", "auto"),
        ("claudeAgent", "preferences"),
        ("alternate", "preferences"),
    ] {
        let (s, c, root) = fixture();
        held(&c);
        let before = projection(&c);
        let old = before["runs"][0].clone();
        let old_pt = before["providerThreads"][0].clone();
        if mode == "preferences" {
            c.rpc("orchestration.dispatchCommand", json!({"type":"thread.model-selection.set","commandId":"preferences","threadId":"t","modelSelection":{"instanceId":"codex","model":"two"}})).unwrap();
        }
        let mut request = json!({"type":"message.dispatch","commandId":"restart","threadId":"t","messageId":"replacement","text":"NEXT replacement","attachments":[],"modelSelection":{"instanceId":target,"model":"two"},"dispatchMode":{"type":if mode == "preferences" {"steer_active"} else {mode},"targetRunId":old["id"]}});
        if mode == "auto" {
            request.as_object_mut().unwrap().remove("dispatchMode");
            request["deliveryIntent"] = json!("auto");
        }
        let receipt = c
            .rpc("orchestration.dispatchCommand", request.clone())
            .unwrap();
        assert_eq!(
            c.rpc("orchestration.dispatchCommand", request).unwrap(),
            receipt
        );
        let finished = support::wait_for(Duration::from_secs(15), || {
            let p = projection(&c);
            assert_ne!(
                p["runs"][0]["status"], "failed",
                "{target}: {}",
                p["runs"][0]["error"]
            );
            p["runs"][0]["status"] == "completed"
        });
        if !finished {
            let db = s.state.db.lock().unwrap();
            for sql in ["SELECT json_array(terminal_at,terminal_status,ready_at,admission_at,completed_at,error) FROM run_restarts", "SELECT json_array(released_at,last_error,json_extract(payload,'$.runIds')) FROM provider_switches"] {
                let mut q = db.prepare(sql).unwrap();
                let rows = q.query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
                eprintln!("journal: {rows:?}");
            }
            drop(db);
            eprintln!("run: {}", projection(&c)["runs"]);
            eprintln!(
                "trace: {}",
                std::fs::read_to_string(root.join("selection-trace.jsonl")).unwrap_or_default()
            );
        }
        assert!(finished, "{target}: replacement did not complete");
        let p = projection(&c);
        assert_eq!(p["runs"].as_array().unwrap().len(), 1);
        let run = &p["runs"][0];
        assert_eq!(run["id"], old["id"]);
        assert_eq!(run["providerInstanceId"], target);
        assert_eq!(run["userMessageId"], "replacement");
        assert_ne!(run["rootNodeId"], old["rootNodeId"]);
        assert_ne!(run["activeAttemptId"], old["activeAttemptId"]);
        assert_eq!(
            p["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["id"] == run["activeAttemptId"])
                .unwrap()["status"],
            "completed"
        );
        assert!(p["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pt| pt["runAttemptId"] == run["activeAttemptId"] && pt["status"] == "completed"));
        assert_eq!(s.state.db.lock().unwrap().query_row("SELECT count(*) FROM events WHERE thread_id='t' AND json_extract(body,'$.type')='thread.provider-switched'", [], |r| r.get::<_,i64>(0)).unwrap(), 1);
        let methods = trace(&root);
        let release = methods
            .iter()
            .position(|f| f["method"] == "thread/unsubscribe")
            .unwrap();
        let incoming = methods
            .iter()
            .enumerate()
            .find(|(i, f)| {
                *i > release
                    && matches!(f["method"].as_str(), Some("thread/resume" | "claude/user"))
            })
            .unwrap()
            .0;
        assert!(incoming > release);
        assert_eq!(p["attempts"].as_array().unwrap().len(), 2);
        assert!(p["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == old["activeAttemptId"] && a["status"] == "superseded"));
        let pt = p["providerThreads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pt| pt["id"] == run["providerThreadId"])
            .unwrap();
        if target == "alternate" {
            assert_eq!(pt["id"], old_pt["id"]);
            assert_eq!(pt["nativeThreadRef"], old_pt["nativeThreadRef"]);
            assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
        } else {
            assert_ne!(pt["id"], old_pt["id"]);
            let released = p["providerThreads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|pt| pt["id"] == old_pt["id"])
                .unwrap();
            assert_eq!(released["status"], "not_loaded");
            assert!(released["providerSessionId"].is_null());
            assert_eq!(pt["driver"], "claudeAgent");
            let h = p["contextHandoffs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["targetRunId"] == run["id"])
                .unwrap();
            assert!(h.to_string().contains("SOURCE-HISTORY-WORD"));
            assert_eq!(h["coveredRunOrdinals"], json!({"from":1,"to":1}));
            assert!(p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] != "seed"
                    && m["role"] == "assistant"
                    && m["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("SOURCE-HISTORY-WORD"))));
        }
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.delete","commandId":"delete","threadId":"t"}),
        )
        .unwrap();
    }
}

fn restart(c: &Connection, target: &str) -> Value {
    json!({"type":"message.dispatch","commandId":"restart","threadId":"t","messageId":"replacement","text":"NEXT replacement","attachments":[],"modelSelection":{"instanceId":target,"model":"two"},"dispatchMode":{"type":"restart_active","targetRunId":projection(c)["runs"][0]["id"]}})
}
fn count(s: &support::Server, sql: &str) -> i64 {
    s.state
        .db
        .lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
#[test]
fn active_handoff_receipt_failure_rolls_back_ownership_and_does_not_interrupt() {
    let (s, c, root) = fixture();
    held(&c);
    let before = projection(&c);
    s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_restart BEFORE INSERT ON receipts WHEN NEW.command_id='restart' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(c
        .rpc("orchestration.dispatchCommand", restart(&c, "claudeAgent"))
        .is_err());
    let after = projection(&c);
    for key in [
        "runs",
        "attempts",
        "providerThreads",
        "providerSessions",
        "contextTransfers",
        "contextHandoffs",
    ] {
        assert_eq!(after[key], before[key], "{key}");
    }
    assert_eq!(count(&s, "SELECT count(*) FROM provider_switches"), 0);
    assert_eq!(count(&s, "SELECT count(*) FROM run_restarts"), 0);
    assert!(!trace(&root).iter().any(|f| f["method"] == "turn/interrupt"));
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
    )
    .unwrap();
}
#[test]
fn active_handoff_waits_for_old_ack_and_fences_late_output() {
    for target in ["claudeAgent", "alternate"] {
        let (s, c, root) = fixture();
        held(&c);
        std::fs::write(root.join(".selection-hold-interrupt"), "").unwrap();
        c.rpc("orchestration.dispatchCommand", restart(&c, target))
            .unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || trace(&root)
            .iter()
            .any(|f| f["method"] == "turn/interrupt")));
        let before = trace(&root);
        assert_eq!(
            before
                .iter()
                .filter(|f| f["method"] == "turn/start")
                .count(),
            1
        );
        assert!(!before
            .iter()
            .any(|f| f["method"] == "thread/unsubscribe" || f["method"] == "claude/user"));
        std::fs::remove_file(root.join(".selection-hold-interrupt")).unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || projection(&c)
            ["runs"][0]["status"]
            == "completed"));
        assert!(!projection(&c)["messages"]
            .to_string()
            .contains("STALE-OLD-ATTEMPT"));
        assert_eq!(count(&s,"SELECT count(*) FROM run_restarts WHERE completed_at IS NOT NULL AND error IS NULL"),1);
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
        )
        .unwrap();
    }
}
#[test]
fn active_handoff_release_failure_prevents_replacement_delivery() {
    let (s, c, root) = fixture();
    held(&c);
    std::fs::write(root.join(".selection-release-fails"), "").unwrap();
    c.rpc("orchestration.dispatchCommand", restart(&c, "claudeAgent"))
        .unwrap();
    assert!(support::wait_for(Duration::from_secs(8), || projection(&c)
        ["runs"][0]["status"]
        == "failed"));
    assert_eq!(count(&s,"SELECT count(*) FROM provider_switches WHERE released_at IS NULL AND last_error IS NOT NULL"),1);
    assert!(!trace(&root).iter().any(|f| f["method"] == "claude/user"));
    std::fs::remove_file(root.join(".selection-release-fails")).unwrap();
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
    )
    .unwrap();
}
#[test]
fn stop_delete_or_archive_during_cross_provider_restart_does_not_send_replacement() {
    for action in ["thread.stop", "thread.delete", "thread.archive"] {
        let (s, c, root) = fixture();
        held(&c);
        std::fs::write(root.join(".selection-hold-interrupt"), "").unwrap();
        c.rpc("orchestration.dispatchCommand", restart(&c, "claudeAgent"))
            .unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || trace(&root)
            .iter()
            .any(|f| f["method"] == "turn/interrupt")));
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":action,"commandId":"stop","threadId":"t"}),
        )
        .unwrap();
        std::fs::remove_file(root.join(".selection-hold-interrupt")).unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || count(
            &s,
            "SELECT count(*) FROM run_restarts WHERE completed_at IS NOT NULL"
        ) == 1));
        assert!(!trace(&root).iter().any(|f| f["method"] == "claude/user"));
    }
}

#[test]
#[ignore = "installed Codex active shell, then Codex profile resume and Claude portable context with private codewords"]
fn live_active_cross_instance_resume_and_portable_handoff() {
    for target in ["alternate", "claudeAgent"] {
        let s = support::start(|s| {
            *s.provider.lock().unwrap() =
                Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    s.dir.clone(),
                )));
        });
        let c = s.connect();
        let root = s.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        let codex_binary = std::env::var("O3_LIVE_CODEX_BINARY").unwrap_or_else(|_| "codex".into());
        let codex_home = std::env::var("CODEX_HOME").expect("run with tools/live-transfer-e2e");
        let claude_home = std::env::var("CLAUDE_CONFIG_DIR").expect("isolated Claude home");
        c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"homePath":codex_home,"binaryPath":codex_binary}},"claudeAgent":{"driver":"claudeAgent","enabled":true,"config":{"homePath":claude_home}}}}})).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Live active handoff","workspaceRoot":root})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Live active handoff","modelSelection":{"instanceId":"codex","model":"gpt-6.1-sol","options":[{"id":"reasoningEffort","value":"low"}]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let word = format!(
            "active-{}-private",
            s.dir.file_name().unwrap().to_string_lossy()
        );
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":format!("Remember the private codeword {word}. Execute exactly one shell command: sleep 12; printf 'SHELL-FINISHED'. Then reply only with the codeword. Use no other tools."),"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(
            support::wait_for(Duration::from_secs(90), || {
                let p = projection(&c);
                assert_ne!(
                    p["runs"][0]["status"], "failed",
                    "{target}: source failed: {}",
                    p["runs"][0]["error"]
                );
                p["turnItems"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["status"] == "running" && item["type"] == "command_execution")
            }),
            "source native shell never became active"
        );
        if target == "alternate" {
            // Initialize the fresh native SQLite home before adding another
            // process that shares it; this proof exercises transition, not first-use migrations.
            c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"homePath":codex_home,"binaryPath":codex_binary}},"alternate":{"driver":"codex","enabled":true,"config":{"homePath":codex_home,"binaryPath":codex_binary}},"claudeAgent":{"driver":"claudeAgent","enabled":true,"config":{"homePath":claude_home}}}}})).unwrap();
        }
        let before = projection(&c);
        let old = before["runs"][0].clone();
        let native = before["providerThreads"][0]["nativeThreadRef"].clone();
        let selection = if target == "alternate" {
            json!({"instanceId":target,"model":"gpt-6.1-sol","options":[{"id":"reasoningEffort","value":"low"}]})
        } else {
            json!({"instanceId":target,"model":"haiku"})
        };
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"switch","threadId":"t","messageId":"replacement","text":"Reply only with the private codeword from my earlier user message. Execute no tools.","attachments":[],"modelSelection":selection,"dispatchMode":{"type":"steer_active","targetRunId":old["id"]}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(90), || {
            let p = projection(&c);
            assert_ne!(
                p["runs"][0]["status"], "failed",
                "{target}: {}",
                p["runs"][0]["error"]
            );
            p["runs"][0]["status"] == "completed"
        }));
        let p = projection(&c);
        assert_eq!(p["runs"].as_array().unwrap().len(), 1);
        assert_eq!(p["runs"][0]["id"], old["id"]);
        assert_eq!(p["runs"][0]["providerInstanceId"], target);
        assert_eq!(p["attempts"].as_array().unwrap().len(), 2);
        assert!(
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant"
                    && m["nodeId"] == p["runs"][0]["rootNodeId"]
                    && m["text"].as_str().is_some_and(|text| text.contains(&word))),
            "{target}: incoming response did not recall private source history"
        );
        if target == "alternate" {
            assert_eq!(p["providerThreads"][0]["nativeThreadRef"], native);
            assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(p["contextHandoffs"].as_array().unwrap().len(), 1);
            assert_eq!(
                p["contextHandoffs"][0]["coveredRunOrdinals"],
                json!({"from":1,"to":1})
            );
        }
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
        )
        .unwrap();
        println!("Installed Codex -> {target}: active shell restart retained run, superseded attempt, and recalled private source history");
    }
}

#[test]
fn interrupted_commands_require_physical_terminal_containment_before_handoff() {
    for remains in [false, true] {
        let (s, c, root) = fixture();
        std::fs::write(root.join(".selection-owned-command"), "").unwrap();
        if remains {
            std::fs::write(root.join(".selection-terminal-remains"), "").unwrap();
        }
        held(&c);
        std::fs::remove_file(root.join(".selection-owned-command")).unwrap();
        c.rpc("orchestration.dispatchCommand", restart(&c, "claudeAgent"))
            .unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || matches!(
            projection(&c)["runs"][0]["status"].as_str(),
            Some("completed" | "failed")
        )));
        let p = projection(&c);
        assert_eq!(
            p["runs"][0]["status"],
            if remains { "failed" } else { "completed" }
        );
        let frames = trace(&root);
        assert!(frames
            .iter()
            .any(|f| f["method"] == "thread/backgroundTerminals/terminate"));
        if remains {
            assert!(frames
                .iter()
                .any(|f| f["method"] == "thread/backgroundTerminals/list"));
            assert!(!frames.iter().any(|f| f["method"] == "claude/user"));
        } else {
            let contained = frames
                .iter()
                .position(|f| f["method"] == "thread/backgroundTerminals/terminate")
                .unwrap();
            let incoming = frames
                .iter()
                .position(|f| f["method"] == "claude/user")
                .unwrap();
            assert!(contained < incoming);
        }
        assert_eq!(
            count(
                &s,
                "SELECT count(*) FROM run_restarts WHERE completed_at IS NOT NULL"
            ),
            1
        );
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.delete","commandId":"cleanup","threadId":"t"}),
        )
        .unwrap();
    }
}
