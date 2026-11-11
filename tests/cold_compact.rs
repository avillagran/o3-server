mod support;
use serde_json::json;
use std::{sync::Arc, time::Duration};
#[test]
fn cold_handoff_is_summarized_without_tools_before_native_compact() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::claude::ClaudeProvider::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/cold-compact.py"),
        )));
    });
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Cold context","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.providerHandoff','historical-proof') WHERE kind='thread' AND id='t'",[]).unwrap();
    }
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"compact","messageId":"m","threadId":"t","text":"/compact ","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || {
        let p = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        p["runs"][0]["status"] == "completed"
    }));
    assert!(s.state.dir.join("compact-finished").exists());
}
