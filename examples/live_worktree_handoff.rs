//! Explicit opt-in: cargo run --example live_worktree_handoff -- claudeAgent|codex [model].
//! Uses the installed CLI/account for two short turns in a disposable server profile.
#[path = "../tests/support/mod.rs"]
mod support;
use o3_server::provider::Provider;
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
    let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
    if instance == "claudeAgent" {
        assert!(claude.is_some(), "installed Claude required");
    }
    let codex = Arc::new(o3_server::codex::CodexProvider::new("codex".into(), None));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let model = args.get(2).cloned().unwrap_or_else(|| {
        if instance == "claudeAgent" {
            "haiku".into()
        } else {
            runtime.block_on(codex.probe()).unwrap();
            let cfg = codex.config();
            let models = cfg[0]["models"].as_array().expect("Codex models");
            models
                .iter()
                .find(|m| m["isDefault"] == true)
                .or_else(|| models.first())
                .and_then(|m| m["slug"].as_str())
                .expect("Codex default model")
                .into()
        }
    });
    let s = support::start(|s| {
        s.streams.terminals.set_shell("/bin/sh");
        std::fs::write(s.dir.join("settings.json"),json!({"enableAgentBrowserAccess":false,"worktreesDirectory":s.dir.join("worktrees"),"projectSettingsOverrides":{"p":{"newWorktreesStartFromOrigin":false,"defaultProjectScripts":[{"id":"setup","name":"Setup","icon":"configure","command":"printf ready > setup-proof.txt","runOnWorktreeCreate":true}]}}}).to_string()).unwrap();
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
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"probe","modelSelection":{"instanceId":instance,"model":model},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let continuation="Use your shell to write the exact verification code given before the handoff, without a newline, to proof.txt in your current directory. Do not call handoff again. Then finish.";
    let prompt=format!("Disposable integration verification. Remember code AMBER-482. Your only action this turn is to call t3_worktree_handoff (the injected T3 MCP tool) with branch proof/native, runSetupScript true and continuationPrompt exactly this text: {continuation}\nDo not write proof.txt before the handoff. Do not start another thread. This call ends the turn; the queued continuation performs the file write.");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"first","threadId":"t","messageId":"first","text":prompt,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let work = s.dir.join("worktrees/repo/proof-native");
    let settled = support::wait_for(Duration::from_secs(180), || {
        let p = t3_protocol::api::orchestration::thread_projection(&c, "t").unwrap();
        let runs = p["runs"].as_array().unwrap();
        runs.iter().any(|r| r["status"] == "failed")
            || (runs.len() == 2
                && runs.iter().any(|r| r["status"] == "completed")
                && work.join("proof.txt").exists()
                && work.join("setup-proof.txt").exists())
    });
    let p = t3_protocol::api::orchestration::thread_projection(&c, "t").unwrap();
    let proof = std::fs::read_to_string(work.join("proof.txt")).ok();
    let setup = std::fs::read_to_string(work.join("setup-proof.txt")).ok();
    let runs = p["runs"].as_array().unwrap();
    let identity = runs.len() == 2
        && runs[0]["providerThreadId"].is_string()
        && runs[0]["providerThreadId"] == runs[1]["providerThreadId"];
    let native_ids:i64=s.state.db.lock().unwrap().query_row("SELECT count(DISTINCT json_extract(body,'$.payload.nativeThreadRef.nativeId')) FROM events WHERE json_extract(body,'$.type')='provider-thread.updated' AND json_type(body,'$.payload.nativeThreadRef.nativeId')='text'",[],|r|r.get(0)).unwrap();
    runtime.block_on(async {
        if let Some(claude) = &claude {
            claude.shutdown().await;
        }
        codex.shutdown().await;
    });
    let statuses: Vec<Value> = runs
        .iter()
        .map(|r| json!({"status":r["status"],"error":r["error"]}))
        .collect();
    println!(
        "{}",
        json!({"provider":instance,"model":model,"settled":settled,"sameProviderThread":identity,"nativeIds":native_ids,"proofMatches":proof.as_deref()==Some("AMBER-482"),"setupReady":setup.as_deref()==Some("ready"),"runs":statuses,"profile":s.dir})
    );
    assert!(
        settled
            && identity
            && native_ids == 1
            && proof.as_deref() == Some("AMBER-482")
            && setup.as_deref() == Some("ready"),
        "live handoff proof failed"
    );
}
