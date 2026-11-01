mod support;
// Real Connection -> dispatch -> Codex stdio fixture peer -> SQLite projection.
use serde_json::{json, Value};
use t3_protocol::{pair_bearer, Connection, ConnectionOptions, PairingClient};
#[test]
fn codex_approval_and_steer_through_client() {
    let dir = std::env::temp_dir().join(format!(
        "o3-codex-contract-{}",
        o3_server::store::random_id().unwrap()
    ));
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("proof"), "before\n").unwrap();
    let state = o3_server::store::State::open(&dir.join("state")).unwrap();
    let secret = state.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let handle = std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async move{
            let provider=std::sync::Arc::new(o3_server::codex::CodexProvider::new(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/mock-server.py"),None));
            provider.probe().await.unwrap();*state.provider.lock().unwrap()=Some(std::sync::Arc::new(o3_server::provider::Providers{claude:None,codex:provider.clone()}));
            tokio::select!{r=o3_server::serve(tokio::net::TcpListener::from_std(listener).unwrap(),std::sync::Arc::new(state))=>r.unwrap(),_=stopped=>{}}
            provider.shutdown().await;
        });
    });
    let c = pair_bearer(
        &base,
        &secret,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )
    .unwrap()
    .connection(&ConnectionOptions::default())
    .unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(config["providers"][1]["driver"], "codex");
    assert!(matches!(
        config["providers"][1]["status"].as_str(),
        Some("warning" | "ready")
    ));
    assert_eq!(
        c.rpc("server.refreshProviders", json!({})).unwrap()["providers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Project","workspaceRoot":root,"scripts":[]})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Codex","modelSelection":{"instanceId":"codex","model":"gpt-6.1-sol","options":[]},"runtimeMode":"approval-required","interactionMode":"default","worktreePath":root,"branch":null})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"fixture","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let p = wait(&c, "waiting");
    let request = &p["runtimeRequests"][0];
    let rid = p["runs"][0]["id"].clone();
    assert_eq!(request["nativeRequestRef"]["nativeId"], "101");
    assert_ne!(request["nodeId"], rid);
    assert_eq!(request["responseCapability"]["type"], "live");
    assert!(p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "approval_request"));
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"steer","messageId":"m-steer","threadId":"t","text":"more","attachments":[],"dispatchMode":{"type":"queue_after_active"}})).unwrap();
    let queued = c
        .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap();
    let queued = queued["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["status"] == "queued")
        .unwrap()["id"]
        .clone();
    c.rpc("orchestration.dispatchCommand",json!({"type":"queued-message.promote-to-steer","commandId":"promote","threadId":"t","queuedRunId":queued,"targetRunId":rid})).unwrap();
    std::fs::write(root.join("proof"), "after\n").unwrap();
    let response = json!({"type":"runtime-request.respond","commandId":"approve","threadId":"t","requestId":request["id"],"decision":"accept"});
    let receipt = c
        .rpc("orchestration.dispatchCommand", response.clone())
        .unwrap();
    assert_eq!(
        c.rpc("orchestration.dispatchCommand", response).unwrap(),
        receipt
    );
    let p = wait(&c, "completed");
    assert_eq!(p["runtimeRequests"][0]["status"], "resolved");
    assert_eq!(p["thread"]["codexNativeThreadId"], "thread-1");
    let command = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "command_execution")
        .unwrap();
    assert_eq!(command["input"], "echo fixture");
    assert_eq!(command["output"], "fixture\n");
    assert_eq!(command["exitCode"], 0);
    let mut output = support::subscribe(
        c.clone(),
        "orchestration.subscribeCommandOutput",
        json!({"threadId":"t","itemId":command["id"]}),
    );
    assert_eq!(
        output.next(std::time::Duration::from_secs(10))["text"],
        "fixture\n"
    );
    assert!(p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .all(|m| m["streaming"] == false));
    let diff = c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t","fromTurnCount":0,"toTurnCount":1}),
        )
        .unwrap();
    assert!(diff["diff"].as_str().unwrap().contains("+after"));
    let diff = c
        .rpc(
            "orchestration.getFullThreadDiff",
            json!({"threadId":"t","toTurnCount":1}),
        )
        .unwrap();
    assert!(diff["diff"].as_str().unwrap().contains("-before"));
    let baseline = p["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cp| cp["ordinalWithinScope"] == 0)
        .unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"checkpoint.rollback","commandId":"rollback","threadId":"t","scopeId":baseline["scopeId"],"checkpointId":baseline["id"]})).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("proof")).unwrap(),
        "before\n"
    );
    drop(c);
    let _ = stop.send(());
    handle.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
fn wait(c: &Connection, status: &str) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let p = c
            .request_json("GET", "/api/orchestration/threads/t/bounded", None, true)
            .unwrap()["projection"]
            .clone();
        if p["runs"][0]["status"] == status {
            return p;
        }
        assert!(std::time::Instant::now() < deadline, "{p}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
