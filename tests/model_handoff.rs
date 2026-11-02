mod support;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use t3_protocol::api::orchestration;

#[test]
fn instance_switch_starts_fresh_with_real_transcript_in_both_directions() {
    for explicit in [true, false] {
        for (source, target) in [("claudeAgent", "codex"), ("codex", "claudeAgent")] {
            let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/handoff.py");
            let s = support::start(|s| {
                *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                    claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                        peer.clone(),
                    ))),
                    codex: Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
                }));
            });
            let c = s.connect();
            c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Handoff","modelSelection":{"instanceId":source,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
            for (n, text) in [(0, "seed-proof"), (1, "continue-proof")] {
                if n == 1 && explicit {
                    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.model-selection.set","commandId":"switch","threadId":"t","modelSelection":{"instanceId":target,"model":"test"}})).unwrap();
                }
                let mut command = json!({"type":"message.dispatch","commandId":format!("send{n}"),"threadId":"t","messageId":format!("m{n}"),"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}});
                if n == 1 && !explicit {
                    command["modelSelection"] = json!({"instanceId":target,"model":"test"});
                }
                c.rpc("orchestration.dispatchCommand", command).unwrap();
                assert!(support::wait_for(Duration::from_secs(10), || {
                    let p = orchestration::thread_projection(&c, "t").unwrap();
                    assert!(
                        !p["runs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|r| r["status"] == "failed"),
                        "{p}"
                    );
                    p["runs"].as_array().unwrap().len() == n + 1
                        && p["runs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|r| r["status"] == "completed")
                }));
                let p = orchestration::thread_projection(&c, "t").unwrap();
                let timeline = t3_protocol::timeline::messages(
                    &p,
                    &t3_protocol::timeline::Render::neutral(false),
                )
                .unwrap();
                assert!(
                    serde_json::to_string(&timeline)
                        .unwrap()
                        .contains("remembered-proof"),
                    "{timeline:?}"
                );
            }
        }
    }
}

#[test]
fn historical_fork_supplies_image_and_context_to_fresh_native_peers() {
    for instance in ["claudeAgent", "codex"] {
        let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/input/history.py");
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                    peer.clone(),
                ))),
                codex: Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
            }));
        });
        let c = s.connect();
        let image = c
            .upload_attachment(
                "pixel.png",
                "image/png",
                8,
                std::io::Cursor::new(b"\x89PNG\r\n\x1a\n"),
            )
            .unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"History","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let mut boundary = serde_json::Value::Null;
        for (n, tid, text) in [
            (0, "t", "seed-proof"),
            (1, "t", "later-proof"),
            (2, "fork", "fork-proof"),
        ] {
            if n == 2 {
                c.rpc("orchestration.dispatchCommand",json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"t","targetThreadId":"fork","sourcePoint":{"type":"run","runId":boundary}})).unwrap();
            }
            c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":format!("s{n}"),"threadId":tid,"messageId":format!("m{n}"),"text":text,"modelSelection":{"instanceId":instance,"model":if n==1 {"changed"} else {"test"}},"dispatchMode":{"type":"start_immediately"},"attachments":if n==0 {json!([image])} else {json!([])},"context":if n==0 {json!({"proof":"context-proof"})} else {json!(null)}})).unwrap();
            assert!(support::wait_for(Duration::from_secs(10), || {
                let p = orchestration::thread_projection(&c, tid).unwrap();
                assert!(
                    !p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["status"] == "failed"),
                    "{p}"
                );
                p["runs"].as_array().unwrap().len() == if n == 1 { 2 } else { 1 }
                    && p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| r["status"] == "completed")
            }));
            let p = orchestration::thread_projection(&c, tid).unwrap();
            if n == 1 {
                assert!(p["thread"]["providerGeneration"].is_null());
                assert_eq!(p["thread"]["modelSelection"]["model"], "changed");
            }
            if n == 0 {
                let db = s.state.db.lock().unwrap();
                let rid = p["runs"][0]["id"].as_str().unwrap();
                db.execute("INSERT INTO records(kind,id,thread_id,body) VALUES ('turnItems','read-proof','t',?)", [json!({"id":"read-proof","threadId":"t","runId":rid,"type":"dynamic_tool","toolName":"Read","input":{"path":"proof.txt"},"output":"tool-output-proof","status":"completed"}).to_string()]).unwrap();
            }
            if n == 0 {
                boundary = p["runs"][0]["id"].clone();
            }
            let timeline =
                t3_protocol::timeline::messages(&p, &t3_protocol::timeline::Render::neutral(false))
                    .unwrap();
            assert!(serde_json::to_string(&timeline)
                .unwrap()
                .contains("remembered-proof"));
        }
    }
}

#[test]
fn fallback_tool_context_drops_oldest_with_an_explicit_marker() {
    let s = support::start(|_| {});
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Bounded","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    {
        let db = s.state.db.lock().unwrap();
        db.execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES ('runs','r','t',?)",
            [json!({"id":"r","ordinal":1,"status":"failed"}).to_string()],
        )
        .unwrap();
        for n in 0..3 {
            db.execute("INSERT INTO records(kind,id,thread_id,body) VALUES ('turnItems',?,'t',?)",rusqlite::params![format!("tool{n}"),json!({"id":format!("tool{n}"),"runId":"r","type":"dynamic_tool","toolName":"Read","output":format!("proof-{n} {}", "x".repeat(600000))}).to_string()]).unwrap();
        }
    }
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"t","targetThreadId":"fork","sourcePoint":{"type":"latest_stable"}})).unwrap();
    let p = orchestration::thread_projection(&c, "fork").unwrap();
    let text = p["thread"]["providerHandoff"].as_str().unwrap();
    assert!(text.len() <= 1024 * 1024);
    assert!(text.contains("Historical context truncated"));
    assert!(!text.contains("proof-0"));
    assert!(text.contains("proof-2"));
}
