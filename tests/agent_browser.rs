mod support;
use serde_json::json;
#[test]
fn browser_capability_tracks_settings_and_fixture_availability() {
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
    );
    let s = support::start(|_| {});
    let c = s.connect();
    let config = t3_protocol::api::server::config(&c).unwrap();
    assert_eq!(
        config["environment"]["capabilities"]["agentBrowserAccess"],
        true
    );
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"enableAgentBrowserAccess":false}}),
    )
    .unwrap();
    let config = t3_protocol::api::server::config(&c).unwrap();
    assert_eq!(
        config["environment"]["capabilities"]["agentBrowserAccess"],
        false
    );
}

#[test]
fn native_sessions_receive_scoped_broker_and_close_it_after_turn() {
    use std::{sync::Arc, time::Duration};
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
    );
    for instance in ["claudeAgent", "codex"] {
        let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-browser-peer.py");
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                    peer.clone(),
                ))),
                codex: Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
            }))
        });
        let c = s.connect();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"c","threadId":"t","projectId":"p","title":"Browser","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        for (id, text) in [("first", "enabled"), ("second", "disabled")] {
            if text == "disabled" {
                c.rpc(
                    "server.updateSettings",
                    json!({"patch":{"projectAgentBrowserAccessOverrides":{"p":false}}}),
                )
                .unwrap();
            }
            c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"t","text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
            assert!(support::wait_for(Duration::from_secs(12), || {
                let p = t3_protocol::api::orchestration::thread_projection(&c, "t").unwrap();
                let runs = p["runs"].as_array().unwrap();
                assert!(!runs.iter().any(|r| r["status"] == "failed"), "{p}");
                runs.len() == if text == "enabled" { 1 } else { 2 }
                    && runs.iter().all(|r| r["status"] == "completed")
            }));
            let tabs = t3_protocol::api::preview::list(&c, "t").unwrap();
            assert!(
                tabs["sessions"].as_array().unwrap().is_empty(),
                "session tabs must be removed"
            );
        }
        if s.dir.join("browser-profiles").exists() {
            assert_eq!(
                std::fs::read_dir(s.dir.join("browser-profiles"))
                    .unwrap()
                    .count(),
                0,
                "ephemeral profiles removed"
            );
        }
    }
}
