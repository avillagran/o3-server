mod support;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_protocol::api::orchestration;

#[test]
fn unsupported_subagent_stop_preserves_real_native_owner() {
    for instance in ["claudeAgent", "codex"] {
        let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/input/stop_guard.py");
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                    peer.clone(),
                ))),
                codex: Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
            }));
        });
        let c = s.connect();
        c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":"c","threadId":"t","projectId":"p","title":"Stop guard","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        c.rpc("orchestration.dispatchCommand", json!({"type":"message.dispatch","commandId":"m","threadId":"t","messageId":"m","text":"wait","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(
            support::wait_for(Duration::from_secs(10), || {
                orchestration::thread_projection(&c, "t").unwrap()["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| {
                        m["text"]
                            .as_str()
                            .is_some_and(|t| t.contains("owner-ready"))
                    })
            }),
            "{}",
            orchestration::thread_projection(&c, "t").unwrap()
        );
        let p = orchestration::thread_projection(&c, "t").unwrap();
        let rid = p["runs"][0]["id"].as_str().unwrap();
        // Seed a discovered child: neither adapter currently exposes a verified child-stop control.
        let sub = json!({"id":"child","threadId":"t","runId":rid,"status":"running","prompt":"work","result":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-11-02T00:00:00Z"});
        s.state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('subagents','child','t',?)",
                [sub.to_string()],
            )
            .unwrap();
        let result = orchestration::dispatch(
            &c,
            &orchestration::Command::SubagentStop {
                command_id: "stop".into(),
                thread_id: "t".into(),
                run_id: rid.into(),
                subagent_id: "child".into(),
            },
        );
        assert!(
            result.is_err(),
            "must refuse unsupported child cancellation: {result:?}"
        );
        let p = orchestration::thread_projection(&c, "t").unwrap();
        assert_eq!(p["runs"][0]["status"], "running", "{p}");
        assert_eq!(p["subagents"][0]["status"], "running", "{p}");
        // A real continuation proves the owner transport survived the rejected stop.
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"continue","threadId":"t","messageId":"next","text":"finish","attachments":[],"dispatchMode":{"type":"steer_active","targetRunId":rid}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(10), || {
            orchestration::thread_projection(&c, "t").unwrap()["runs"][0]["status"] == "completed"
        }));
    }
}
