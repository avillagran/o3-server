mod support;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn uploaded_images_files_and_context_reach_both_native_transports() {
    for instance in ["claudeAgent", "codex"] {
        let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/input/peer.py");
        let server = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                    binary.clone(),
                ))),
                codex: Arc::new(o3_server::codex::CodexProvider::new(binary, None)),
            }));
        });
        let c = server.connect();
        let image = c
            .upload_attachment(
                "pixel.png",
                "image/png",
                8,
                std::io::Cursor::new(b"\x89PNG\r\n\x1a\n"),
            )
            .unwrap();
        let file = c
            .upload_attachment(
                "notes.txt",
                "text/plain",
                10,
                std::io::Cursor::new(b"file-proof"),
            )
            .unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Input","modelSelection":{"instanceId":instance,"model":"test","options":[{"id":"effort","value":"high"},{"id":"fastMode","value":true}]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"input-proof","context":{"records":[{"kind":"terminal","text":"context-proof"}]},"attachments":[image,file],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let p = c
                .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap();
            assert_ne!(p["runs"][0]["status"], "failed", "{p}");
            if p["runs"][0]["status"] == "completed" {
                assert!(p["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["text"] == "verified-input"));
                let shell = c
                    .request_json("GET", "/api/orchestration/shell", None, true)
                    .unwrap();
                assert_eq!(
                    shell["threads"][0]["latestUserAuthoredMessageAt"],
                    p["messages"][0]["createdAt"]
                );
                assert_eq!(
                    shell["threads"][0]["latestRunCompletedAt"],
                    p["runs"][0]["completedAt"]
                );
                if instance == "claudeAgent" {
                    let config = c.rpc("server.getConfig", json!({})).unwrap();
                    assert_eq!(
                        config["providers"][0]["slashCommands"][0]["name"],
                        "compact"
                    );
                    assert_eq!(config["providers"][0]["skills"][0]["name"], "test-skill");
                    assert!(config["providers"][0]["models"].as_array().unwrap().len() > 3);
                }
                break;
            }
            assert!(Instant::now() < deadline, "{p}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn launch_initial_message_and_receipt_roll_back_together_and_retry_once() {
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
        s.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_run BEFORE INSERT ON records WHEN NEW.kind='runs' BEGIN SELECT RAISE(ABORT, 'injected run failure'); END;").unwrap();
    });
    let c = server.connect();
    let launch = json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Atomic launch","modelSelection":{"instanceId":"scripted","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"messageId":"m","text":"hello","context":{"proof":"retained"},"attachments":[]}});
    assert!(c.rpc("orchestration.launchThread", launch.clone()).is_err());
    {
        let db = server.state.db.lock().unwrap();
        for table in ["records", "events", "receipts"] {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        db.execute_batch("DROP TRIGGER reject_run").unwrap();
    }
    assert!(server.state.active.lock().unwrap().is_empty());
    let result = c.rpc("orchestration.launchThread", launch.clone()).unwrap();
    assert_eq!(
        result["projection"]["messages"][0]["context"]["proof"],
        "retained"
    );
    assert_eq!(c.rpc("orchestration.launchThread", launch).unwrap(), result);
    let db = server.state.db.lock().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}

#[test]
fn claude_structured_question_displays_and_accepts_client_answers() {
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::claude::ClaudeProvider::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/peer.py"),
        )));
    });
    let c = server.connect();
    c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Question","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"approval-required","interactionMode":"plan","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"question-proof","attachments":[]}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let request = loop {
        let p = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        assert_ne!(p["runs"][0]["status"], "failed", "{p}");
        let questions = t3_protocol::questions::pending(&p);
        if !questions.as_array().unwrap().is_empty() {
            assert_eq!(questions[0]["questions"][0]["id"], "Which approach?");
            assert_eq!(questions[0]["questions"][0]["multiSelect"], true);
            break questions[0]["requestId"].clone();
        }
        assert!(Instant::now() < deadline, "{p}");
        std::thread::sleep(Duration::from_millis(20));
    };
    // Exactly the client helper's answer shape: no permission decision.
    let mut command=t3_protocol::questions::command(&json!({"thread":"t","runtimeRequestId":request,"action":"respond","answers":{"Which approach?":["Simple","Safe"]}})).unwrap();
    command["commandId"] = json!("answer");
    c.rpc("orchestration.dispatchCommand", command).unwrap();
    loop {
        let p = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        assert_ne!(p["runs"][0]["status"], "failed", "{p}");
        if p["runs"][0]["status"] == "completed" {
            break;
        }
        assert!(Instant::now() < deadline, "{p}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "uses installed Claude and Codex credentials for two short real turns"]
fn live_claude_and_codex_through_real_client() {
    for (instance, model) in [("claudeAgent", "haiku"), ("codex", "gpt-6.1-sol")] {
        let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
        let codex = Arc::new(o3_server::codex::CodexProvider::default());
        let server = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: claude.clone(),
                codex: codex.clone(),
            }));
        });
        let c = server.connect();
        let cwd = server.dir.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Live","workspaceRoot":cwd})).unwrap();
        c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Live","modelSelection":{"instanceId":instance,"model":model,"options":[]},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"Reply with exactly O3_OK. Do not use tools.","attachments":[]}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let p = c
                .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap();
            assert_ne!(
                p["runs"][0]["status"], "failed",
                "{instance}: {}",
                p["runs"][0]["error"]
            );
            if p["runs"][0]["status"] == "completed" {
                assert!(
                    p["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|m| m["role"] == "assistant"
                            && m["text"].as_str().is_some_and(|t| t.contains("O3_OK"))),
                    "{instance}: no expected response"
                );
                eprintln!("{instance}: real turn completed");
                // Wait for the run supervisor to return the idle session to its pool.
                while !server.state.active.lock().unwrap().is_empty() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        if let Some(claude) = &claude {
                            claude.shutdown().await;
                        }
                        codex.shutdown().await;
                    });
                break;
            }
            assert!(Instant::now() < deadline, "{instance}: live timeout");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
