//! Explicit opt-in: cargo run --example live_mcp_launch -- claudeAgent|codex [model].
//! Installed CLI invokes t3_thread_launch in a disposable profile; child proves setup ordering.
#[path = "../tests/support/mod.rs"]
mod support;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Probe")
        .env("GIT_AUTHOR_EMAIL", "probe@example.invalid")
        .env("GIT_COMMITTER_NAME", "Probe")
        .env("GIT_COMMITTER_EMAIL", "probe@example.invalid")
        .output()
        .unwrap();
    assert!(out.status.success());
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let instance = args
        .get(1)
        .map(String::as_str)
        .expect("pass claudeAgent or codex");
    assert!(matches!(instance, "claudeAgent" | "codex"));
    let model = args
        .get(2)
        .map(String::as_str)
        .unwrap_or(if instance == "claudeAgent" {
            "haiku"
        } else {
            "gpt-6.1-sol"
        });
    let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
    if instance == "claudeAgent" {
        assert!(claude.is_some(), "installed Claude required");
    }
    let codex = Arc::new(o3_server::codex::CodexProvider::new("codex".into(), None));
    let s = support::start(|s| {
        s.streams.terminals.set_shell("/bin/sh");
        std::fs::write(s.dir.join("settings.json"),json!({"enableAgentBrowserAccess":false,"newWorktreesStartFromOrigin":false,"worktreesDirectory":s.dir.join("managed"),"defaultProjectScripts":[{"id":"setup","name":"Setup","icon":"configure","runOnWorktreeCreate":true,"async":false,"command":"echo started > setup-started; while [ ! -f \"$T3CODE_PROJECT_ROOT/release\" ]; do sleep 0.05; done; printf READY-861 > setup-proof.txt"}]}).to_string()).unwrap();
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: claude.clone(),
            codex: codex.clone(),
        }));
    });
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["commit", "--allow-empty", "-m", "probe"]);
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Probe","workspaceRoot":root})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"parent","threadId":"parent","projectId":"p","title":"Parent","modelSelection":{"instanceId":instance,"model":model},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let child="Use your shell to read setup-proof.txt in your current working directory and copy its exact bytes, with no extra text, into proof.txt. Finish after that. Do not launch another thread or worktree.";
    let prompt=format!("Disposable integration verification. Create exactly one NEW TOP-LEVEL THREAD using your injected t3_thread_launch MCP tool. This is an explicit request for a new thread, not a subagent or delegation. Use title 'MCP launch proof', clientRequestId 'live-launch', workspaceStrategy {{type:'worktree',baseRef:'main',branch:'proof/mcp',startFromOrigin:false}}, and message exactly this text: {child}\nOmit projectId, modelSelection, runtimeMode and interactionMode so the tool inherits yours. Do not run shell commands or create files yourself. After the tool returns, finish. If you need to retry, reuse clientRequestId live-launch.");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"start","threadId":"parent","messageId":"parent-message","text":prompt,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let work = s.dir.join("managed/repo/proof-mcp");
    let preparing = support::wait_for(Duration::from_secs(180), || {
        let failed = c
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":"parent"}),
            )
            .unwrap()["runs"][0]["status"]
            == "failed";
        work.join("setup-started").exists() || failed
    });
    let tid: Option<String> = s
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM records WHERE kind='thread' AND id!='parent'",
            [],
            |r| r.get(0),
        )
        .ok();
    let mut no_provider_before_setup = false;
    let mut inherited = false;
    let mut child_projection = Value::Null;
    if let Some(tid) = &tid {
        child_projection = c
            .rpc("orchestration.getThreadProjection", json!({"threadId":tid}))
            .unwrap();
        no_provider_before_setup = work.join("setup-started").exists()
            && child_projection["providerTurns"]
                .as_array()
                .unwrap()
                .is_empty()
            && child_projection["runs"][0]["status"] == "preparing";
        inherited = child_projection["thread"]["modelSelection"]["instanceId"] == instance
            && child_projection["thread"]["modelSelection"]["model"] == model
            && child_projection["thread"]["projectId"] == "p"
            && child_projection["messages"][0]["senderThreadId"] == "parent"
            && child_projection["messages"][0]["createdBy"] == "agent"
            && child_projection["thread"]["creationSource"] == "mcp";
        std::fs::write(root.join("release"), "").unwrap();
        let _ = support::wait_for(Duration::from_secs(180), || {
            child_projection = c
                .rpc("orchestration.getThreadProjection", json!({"threadId":tid}))
                .unwrap();
            matches!(
                child_projection["runs"][0]["status"].as_str(),
                Some("completed" | "failed")
            )
        });
    }
    let proof = std::fs::read_to_string(work.join("proof.txt")).ok();
    let parent = c
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"parent"}),
        )
        .unwrap();
    let count: i64 = s
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE kind='thread'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        if let Some(tid) = &tid {
            let _ = o3_server::terminal::call(&s.state, "terminal.close", &json!({"threadId":tid}))
                .await;
        }
        if let Some(claude) = &claude {
            claude.shutdown().await;
        }
        codex.shutdown().await;
    });
    let valid = preparing
        && inherited
        && no_provider_before_setup
        && count == 2
        && child_projection["runs"][0]["status"] == "completed"
        && proof.as_deref() == Some("READY-861");
    println!(
        "{}",
        json!({"provider":instance,"model":model,"nativeMcpLaunch":tid.is_some(),"inherited":inherited,"noProviderBeforeSetup":no_provider_before_setup,"threadCount":count,"childStatus":child_projection["runs"][0]["status"],"childError":child_projection["runs"][0]["error"],"parentStatus":parent["runs"][0]["status"],"parentError":parent["runs"][0]["error"],"proofMatches":proof.as_deref()==Some("READY-861"),"profile":s.dir})
    );
    assert!(valid, "live MCP launch proof failed");
}
