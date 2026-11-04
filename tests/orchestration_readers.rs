mod support;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn wait(c: &t3_protocol::Connection, f: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let p = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        if f(&p) {
            return p;
        }
        assert!(Instant::now() < end, "{p}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn reused_launch_import_title_and_native_attachment_steering() {
    for instance in ["codex", "claudeAgent"] {
        for queued in [false, true] {
            let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/steer.py");
            let s = support::start(|s| {
                *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                    claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                        binary.clone(),
                    ))),
                    codex: Arc::new(o3_server::codex::CodexProvider::new(binary, None)),
                }))
            });
            let c = s.connect();
            let root = s.dir.join("workspace");
            std::fs::create_dir_all(&root).unwrap();
            c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Readers","workspaceRoot":root})).unwrap();
            // This steering peer has one fixed native id; title failure is isolated.
            c.rpc("server.updateSettings",json!({"patch":{"textGenerationModelSelection":{"instanceId":"unavailable","model":"none"}}})).unwrap();
            c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Original","modelSelection":{"instanceId":instance,"model":"old"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
            let launch = json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Seed","reuseExistingThread":true,"generateTitle":true,"modelSelection":{"instanceId":instance,"model":"override"},"workspaceStrategy":{"type":"root"},"importedNativeThread":{"ref":{"driver":instance,"nativeId":"native","strength":"strong"},"metadata":{"title":"Imported"}},"initialMessage":{"text":"hold","attachments":[]}});
            let result = c.rpc("orchestration.launchThread", launch.clone()).unwrap();
            let replay = c.rpc("orchestration.launchThread", launch).unwrap();
            assert_eq!(result["threadId"], replay["threadId"]);
            assert_eq!(result["resumed"], false);
            assert_eq!(replay["resumed"], true);
            assert_eq!(result["projection"]["thread"]["title"], "hold");
            assert_eq!(
                result["projection"]["runs"][0]["modelSelection"]["model"],
                "override"
            );
            assert_eq!(
                result["projection"]["providerThreads"][0]["nativeThreadRef"]["nativeId"],
                "native"
            );
            let p = wait(&c, |p| p["runs"][0]["status"] == "running");
            let rid = p["runs"][0]["id"].clone();
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
            let mut command = json!({"type":"message.dispatch","commandId":"steer","threadId":"t","messageId":"steered","text":"input-proof","context":{"text":"context-proof"},"attachments":[image,file],"dispatchMode":if queued {json!({"type":"queue_after_active"})} else {json!({"type":"steer_active","targetRunId":rid})}});
            if !queued {
                command["dispatchMode"] = json!({"type":"start_immediately"});
                command["deliveryIntent"] = json!("auto");
            }
            c.rpc("orchestration.dispatchCommand", command).unwrap();
            if queued {
                let p = wait(&c, |p| {
                    p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["status"] == "queued")
                });
                let q = p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["status"] == "queued")
                    .unwrap();
                c.rpc("orchestration.dispatchCommand",json!({"type":"queued-message.promote-to-steer","commandId":"promote","threadId":"t","queuedRunId":q["id"],"targetRunId":rid})).unwrap();
            }
            let p = wait(&c, |p| {
                p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["status"] == "completed")
            });
            assert!(
                !p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["status"] == "failed"),
                "{p}"
            );
            assert_eq!(
                p["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["id"] == "steered")
                    .unwrap()["attachments"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
        }
    }
}

fn shell(c: &t3_protocol::Connection) -> Value {
    let stop = std::cell::Cell::new(false);
    let mut out = Value::Null;
    c.subscribe(
        "orchestration.subscribeShell",
        json!({}),
        || stop.get(),
        |v| {
            if let Some(v) = v {
                out = v;
                stop.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    out
}
#[test]
fn codex_quota_failure_and_reset_reach_shell_reader() {
    let binary =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/quota.py");
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::codex::CodexProvider::new(binary, None)))
    });
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Quota","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    for text in ["fail", "reset"] {
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":text,"messageId":text,"threadId":"t","text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        wait(&c, |p| {
            p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| matches!(r["status"].as_str(), Some("completed" | "failed")))
                .count()
                == if text == "fail" { 1 } else { 2 }
        });
        let snap = shell(&c);
        let t = &snap["snapshot"]["threads"][0];
        if text == "fail" {
            assert_eq!(t["lastErrorClass"], "usage_limit", "{snap}");
            assert_eq!(t["usageLimitResetAt"], "2099-01-01T00:00:00.000Z");
        } else {
            assert!(t["lastErrorClass"].is_null());
            assert!(t["usageLimitResetAt"].is_null());
        }
    }
}
