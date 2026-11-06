//! Explicit opt-in: cargo run --example live_staged_launch -- claudeAgent|codex [model].
//! Uses the installed CLI/account and a disposable server profile.
#[path = "../tests/support/mod.rs"]
mod support;
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use t3_protocol::api::orchestration::{self, ThreadLaunch};
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
        std::fs::write(s.dir.join("settings.json"), json!({"enableAgentBrowserAccess":false,"newWorktreesStartFromOrigin":false,"worktreesDirectory":s.dir.join("managed"),"defaultProjectScripts":[{"id":"setup","name":"Setup","icon":"configure","runOnWorktreeCreate":true,"async":false,"command":"echo started > setup-started; while [ ! -f \"$T3CODE_PROJECT_ROOT/release-setup\" ]; do sleep 0.05; done; printf READY-739 > setup-proof.txt"}]}).to_string()).unwrap();
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
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"probe","workspaceRoot":root})).unwrap();
    let launch=ThreadLaunch::from_json(&json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Staged launch proof","modelSelection":{"instanceId":instance,"model":model},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"main","branch":"proof/staged","startFromOrigin":false},"initialMessage":{"messageId":"m","text":"Disposable integration verification. Read setup-proof.txt in your current working directory. Copy its exact contents, without extra text or a newline, into proof.txt using your shell. Finish after that. Do not create another thread or worktree.","attachments":[]}})).unwrap();
    let accepted = orchestration::launch_thread(&c, &launch).unwrap();
    assert_eq!(accepted["projection"]["runs"][0]["status"], "preparing");
    let rid = accepted["projection"]["runs"][0]["id"].clone();
    let work = s.dir.join("managed/repo/proof-staged");
    assert!(support::wait_for(Duration::from_secs(20), || work
        .join("setup-started")
        .exists()));
    let before = orchestration::thread_projection(&c, "t").unwrap();
    assert!(before["providerTurns"].as_array().unwrap().is_empty());
    assert_eq!(before["runs"][0]["status"], "preparing");
    let replay = orchestration::launch_thread(&c, &launch).unwrap();
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["projection"]["runs"][0]["id"], rid);
    let spec = t3_protocol::api::streams::worktree_setup_spec("t").unwrap();
    let mut tracker = support::subscribe(s.connect(), spec.tag(), spec.payload.clone());
    let running = tracker.next(Duration::from_secs(5));
    assert_eq!(running["phase"], "running");
    std::fs::write(root.join("release-setup"), "").unwrap();
    let settled = support::wait_for(Duration::from_secs(180), || {
        let p = orchestration::thread_projection(&c, "t").unwrap();
        matches!(
            p["runs"][0]["status"].as_str(),
            Some("completed" | "failed")
        )
    });
    let p = orchestration::thread_projection(&c, "t").unwrap();
    tracker.until(Duration::from_secs(5), |v| v["phase"] == "done");
    let proof = std::fs::read_to_string(work.join("proof.txt")).ok();
    let one_run = p["runs"].as_array().unwrap().len() == 1 && p["runs"][0]["id"] == rid;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let _ =
            o3_server::terminal::call(&s.state, "terminal.close", &json!({"threadId":"t"})).await;
        if let Some(claude) = &claude {
            claude.shutdown().await;
        }
        codex.shutdown().await;
    });
    println!(
        "{}",
        json!({"provider":instance,"model":model,"settled":settled,"oneRun":one_run,"status":p["runs"][0]["status"],"error":p["runs"][0]["error"],"proofMatches":proof.as_deref()==Some("READY-739"),"noProviderBeforeSetup":true,"replaySameRun":true,"trackingDone":true,"profile":s.dir})
    );
    assert!(
        settled
            && one_run
            && p["runs"][0]["status"] == "completed"
            && proof.as_deref() == Some("READY-739")
    );
}
