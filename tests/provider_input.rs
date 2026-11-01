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
                let run = &p["runs"][0];
                assert!(run["activeAttemptId"].is_string(), "{p}");
                let attempt = &p["attempts"][0];
                assert_eq!(attempt["id"], run["activeAttemptId"]);
                assert_eq!(attempt["status"], "completed");
                assert_eq!(attempt["providerInstanceId"], instance);
                assert_eq!(p["providerTurns"][0]["runAttemptId"], attempt["id"]);
                assert_eq!(p["providerThreads"][0]["id"], run["providerThreadId"]);
                assert_eq!(
                    p["thread"]["activeProviderThreadId"],
                    run["providerThreadId"]
                );
                if instance == "claudeAgent" {
                    assert_eq!(p["providerThreads"][0]["contextUsage"]["maxTokens"], 200000);
                    assert_eq!(p["providerThreads"][0]["contextUsage"]["cost"], 0.01);
                    let config = c.rpc("server.getConfig", json!({})).unwrap();
                    assert_eq!(
                        config["providers"][0]["slashCommands"][0]["name"],
                        "compact"
                    );
                    assert!(config["providers"][0]["skills"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|s| s["name"] == "test-skill"));
                    assert_eq!(
                        config["providers"][0]["usageLimits"]["windows"][0]["usedPercent"],
                        12.0
                    );
                    assert!(config["providers"][0]["usageLimits"]["checkedAt"].is_string());
                    assert!(config["providers"][0]["models"].as_array().unwrap().len() > 3);
                }
                if instance == "codex" {
                    let config = c.rpc("server.getConfig", json!({})).unwrap();
                    let provider = config["providers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|p| p["driver"] == "codex")
                        .unwrap();
                    assert_eq!(
                        provider["models"][0]["capabilities"]["optionDescriptors"][0]["id"],
                        "reasoningEffort"
                    );
                    assert_eq!(
                        provider["models"][0]["capabilities"]["optionDescriptors"][0]["options"][0]
                            ["id"],
                        "high"
                    );
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

#[test]
fn persistent_exact_operation_approval_survives_database_reopen() {
    use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
    struct Peer;
    impl Provider for Peer {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            output: tokio::sync::mpsc::Sender<ProviderEvent>,
            mut controls: tokio::sync::mpsc::Receiver<ProviderControl>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                output.send(ProviderEvent::Started).await.unwrap();
                output.send(ProviderEvent::Native(json!({"type":"permission","request_id":format!("{}:approval",input.run_id),"native_id":"approval","request":{"tool_name":"Bash","input":{"command":input.text}}}))).await.unwrap();
                match controls.recv().await {
                    Some(ProviderControl::Respond { decision, .. })
                        if decision == "accept" || decision == "acceptAlways" => {}
                    _ => return Err("approval missing".into()),
                }
                output.send(ProviderEvent::Completed).await.unwrap();
                Ok(())
            })
        }
    }
    let server = support::start(|s| *s.provider.lock().unwrap() = Some(Arc::new(Peer)));
    let c = server.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Approval","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
    for i in 0..3 {
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":format!("send{i}"),"messageId":format!("m{i}"),"threadId":"t","text":if i<2{"echo safe"}else{"rm other"},"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let p = c
                .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap();
            let requests = p["runtimeRequests"].as_array().unwrap();
            if let Some(request) = requests.iter().find(|r| r["status"] == "pending") {
                if i == 0 {
                    c.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"allow","threadId":"t","requestId":request["id"],"decision":"acceptAlways"})).unwrap();
                } else if i == 2 {
                    break;
                }
            }
            if i < 2
                && p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["status"] == "completed")
                && server.state.active.lock().unwrap().is_empty()
            {
                break;
            }
            assert!(Instant::now() < deadline, "{p}");
            std::thread::sleep(Duration::from_millis(20));
        }
        if i == 0 {
            let reopened = o3_server::store::State::open(&server.state.dir).unwrap();
            assert_eq!(
                reopened
                    .db
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM records WHERE kind='approvalPolicies'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
        }
    }
}

#[test]
fn both_plan_acceptances_dispatch_implementation_and_complete_source_plan() {
    for instance in ["codex", "claudeAgent"] {
        let server = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/input/peer.py"),
                ))),
                codex: Arc::new(o3_server::codex::CodexProvider::new(
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/input/peer.py"),
                    None,
                )),
            }))
        });
        let c = server.connect();
        c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Plan","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"plan","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"plan-proof","attachments":[]}})).unwrap();
        let wait = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let p = c
                    .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                    .unwrap();
                assert!(
                    !p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["status"] == "failed"),
                    "{p}"
                );
                if p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|r| r["status"] == "completed")
                    && server.state.active.lock().unwrap().is_empty()
                {
                    return p;
                }
                assert!(Instant::now() < deadline, "{p}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let p = wait();
        let plan = &p["plans"][0];
        assert_eq!(plan["kind"], "proposed_plan");
        let follow = t3_protocol::plan_workflow::follow_up(plan, "t", "").unwrap();
        let mut cmd = json!({"type":"message.dispatch","commandId":"implement","messageId":"implementation","threadId":"t","text":follow["text"],"interactionMode":follow["interactionMode"],"attachments":[],"dispatchMode":{"type":"start_immediately"}});
        t3_protocol::plan_workflow::attach_source_ref(&mut cmd, &follow).unwrap();
        c.rpc("orchestration.dispatchCommand", cmd).unwrap();
        let p = wait();
        assert_eq!(p["plans"][0]["status"], "completed");
        assert_eq!(p["thread"]["interactionMode"], "default");
        let rename = json!({"type":"thread.metadata.update","commandId":"title","threadId":"t","regenerateTitle":true});
        c.rpc("orchestration.dispatchCommand", rename).unwrap();
        let p = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        assert_eq!(p["thread"]["title"], "plan-proof");
    }
}

#[test]
fn startup_catalog_exposes_project_skills_and_commands_before_any_turn() {
    let server = support::start(|state| {
        *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                "/nonexistent".into(),
            ))),
            codex: Arc::new(o3_server::codex::CodexProvider::new(
                "/nonexistent".into(),
                None,
            )),
        }));
    });
    let root = server.dir.join("catalog");
    std::fs::create_dir_all(root.join(".claude/skills/proof")).unwrap();
    std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
    std::fs::write(
        root.join(".claude/skills/proof/SKILL.md"),
        "---\nname: o3-proof-skill\ndescription: Startup proof\n---\nBody",
    )
    .unwrap();
    std::fs::write(
        root.join(".claude/commands/o3-proof-command.md"),
        "---\ndescription: Startup command\n---\nBody",
    )
    .unwrap();
    let c = server.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"catalog","projectId":"p","title":"Catalog","workspaceRoot":root})).unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let claude = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["driver"] == "claudeAgent")
        .unwrap();
    let skill = claude["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "o3-proof-skill")
        .unwrap();
    assert_eq!(skill["enabled"], true);
    assert_eq!(skill["scope"], "project");
    assert_eq!(skill["description"], "Startup proof");
    assert!(skill["path"].as_str().unwrap().ends_with("SKILL.md"));
    assert!(claude["slashCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "o3-proof-command"));
}
