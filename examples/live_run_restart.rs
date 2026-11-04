//! Opt-in: cargo run --example live_run_restart -- [model]. Uses the installed Codex account.
//! Two native turns, one application run, in a disposable server profile.
#[path = "../tests/support/mod.rs"]
mod support;
use o3_server::provider::Provider;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
fn main() {
    let codex = Arc::new(o3_server::codex::CodexProvider::new("codex".into(), None));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(codex.probe()).unwrap();
    let config = codex.config();
    let models = config[0]["models"].as_array().unwrap();
    let model = std::env::args().nth(1).unwrap_or_else(|| {
        models
            .iter()
            .find(|m| m["isDefault"] == true)
            .or_else(|| models.first())
            .and_then(|m| m["slug"].as_str())
            .unwrap()
            .into()
    });
    let s = support::start(|s| {
        std::fs::write(
            s.dir.join("settings.json"),
            r#"{"enableAgentBrowserAccess":false}"#,
        )
        .unwrap();
        *s.provider.lock().unwrap() = Some(codex.clone());
    });
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"probe","workspaceRoot":root})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"probe","modelSelection":{"instanceId":"codex","model":model},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let prompt="Disposable integration test. Remember code TOPAZ-714. Now use your shell to run exactly: touch started; sleep 60. Do not background it and do not finish your turn until the command ends. A test controller may interrupt you.";
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"first","threadId":"t","messageId":"first","text":prompt,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let read = || t3_protocol::api::orchestration::thread_projection(&c, "t").unwrap();
    let started = support::wait_for(Duration::from_secs(120), || {
        root.join("started").exists() || read()["runs"][0]["status"] == "failed"
    });
    let mut submitted = false;
    if started && root.join("started").exists() {
        let p = read();
        let request = json!({"type":"message.dispatch","commandId":"restart","threadId":"t","messageId":"replacement","text":"The test controller replaced the active turn. Do not sleep again. Write the exact code I gave before the interruption, without a newline, into proof.txt in the current directory using your shell. Then finish.","attachments":[],"dispatchMode":{"type":"restart_active","targetRunId":p["runs"][0]["id"]}});
        submitted = c.rpc("orchestration.dispatchCommand", request).is_ok();
    }
    let settled = submitted
        && support::wait_for(Duration::from_secs(180), || {
            matches!(
                read()["runs"][0]["status"].as_str(),
                Some("completed" | "failed")
            )
        });
    let p = read();
    let proof = std::fs::read_to_string(root.join("proof.txt")).ok();
    let native_ids:i64=s.state.db.lock().unwrap().query_row("SELECT count(DISTINCT json_extract(body,'$.payload.nativeThreadRef.nativeId')) FROM events WHERE json_extract(body,'$.type')='provider-thread.updated' AND json_type(body,'$.payload.nativeThreadRef.nativeId')='text'",[],|r|r.get(0)).unwrap();
    let attempts: Vec<Value> = p["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| json!({"ordinal":a["attemptOrdinal"],"status":a["status"],"reason":a["reason"]}))
        .collect();
    rt.block_on(codex.shutdown());
    println!(
        "{}",
        json!({"model":model,"submitted":submitted,"settled":settled,"runCount":p["runs"].as_array().unwrap().len(),"runStatus":p["runs"][0]["status"],"error":p["runs"][0]["error"],"nativeIds":native_ids,"attempts":attempts,"proofMatches":proof.as_deref()==Some("TOPAZ-714"),"profile":s.dir})
    );
    assert!(
        settled
            && p["runs"].as_array().unwrap().len() == 1
            && attempts.len() == 2
            && attempts[0]["status"] == "superseded"
            && attempts[1]["status"] == "completed"
            && native_ids == 1
            && proof.as_deref() == Some("TOPAZ-714"),
        "live restart proof failed"
    );
}
