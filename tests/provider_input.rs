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
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Input","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
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
            assert_eq!(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r|r.get::<_,i64>(0)).unwrap(),0);
        }
        db.execute_batch("DROP TRIGGER reject_run").unwrap();
    }
    assert!(server.state.active.lock().unwrap().is_empty());
    let result = c.rpc("orchestration.launchThread",launch.clone()).unwrap();
    assert_eq!(result["projection"]["messages"][0]["context"]["proof"], "retained");
    assert_eq!(c.rpc("orchestration.launchThread",launch).unwrap(), result);
    let db = server.state.db.lock().unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r|r.get::<_,i64>(0)).unwrap(),1);
}
