mod common;
mod support;
use serde_json::json;
static BROWSER_ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[test]
fn browser_capability_tracks_settings_and_fixture_availability() {
    let _browser_environment = BROWSER_ENVIRONMENT.lock().unwrap();
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
    );
    let s = support::start(|_| {});
    let c = s.connect();
    let config = t3_protocol::api::server::config(&c).unwrap();
    assert_eq!(
        config["environment"]["capabilities"]["agentBrowserAccess"],
        true
    );
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"enableAgentBrowserAccess":false}}),
    )
    .unwrap();
    let config = t3_protocol::api::server::config(&c).unwrap();
    assert_eq!(
        config["environment"]["capabilities"]["agentBrowserAccess"],
        false
    );
}

#[test]
fn native_sessions_reuse_mcp_credentials_enforce_modes_and_revoke_on_exit() {
    let _browser_environment = BROWSER_ENVIRONMENT.lock().unwrap();
    use std::{sync::Arc, time::Duration};
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
    );
    for instance in ["claudeAgent", "codex"] {
        let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-browser-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let removals = Arc::new(support::profile_removal::Recording::default());
        let s = support::start(|s| {
            s.streams.p2.preview.set_profile_removal(removals.clone());
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }))
        });
        let c = s.connect();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Native","workspaceRoot":s.dir})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"c","threadId":"t","projectId":"p","title":"Browser","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"higher","threadId":"higher","projectId":"p","title":"Higher","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let capture = s.dir.join("peer-observation.json");
        let mut previous = None;
        for (index, text) in ["enabled", "enabled", "disabled", "disabled"]
            .into_iter()
            .enumerate()
        {
            let id = format!("turn-{index}");
            if text == "disabled" {
                c.rpc(
                    "server.updateSettings",
                    json!({"patch":{"projectAgentBrowserAccessOverrides":{"p":false}}}),
                )
                .unwrap();
            }
            c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"t","text":format!("{text}|{}",capture.display()),"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
            assert!(support::wait_for(Duration::from_secs(12), || {
                let p = t3_protocol::api::orchestration::thread_projection(&c, "t").unwrap();
                let runs = p["runs"].as_array().unwrap();
                assert!(!runs.iter().any(|r| r["status"] == "failed"), "{p}");
                runs.len() == index + 1 && runs.iter().all(|r| r["status"] == "completed")
            }));
            let observed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&capture).unwrap()).unwrap();
            std::fs::remove_file(&capture).unwrap();
            if let Some(old) = &previous {
                let old: &serde_json::Value = old;
                if index == 1 {
                    assert_eq!(
                        old, &observed,
                        "unchanged native process and credential are reused"
                    );
                } else {
                    assert_ne!(old["config"], observed["config"]);
                    assert_eq!(native_request(&s, old, "ping", json!({})).0, 401);
                }
            }
            // Foreground completion preserves read access for the cached CLI, but no write authority.
            let (status, reply) = native_request(
                &s,
                &observed,
                "tools/call",
                json!({"name":"t3_thread_configure","arguments":{}}),
            );
            assert_eq!(status, 200);
            assert_eq!(reply["result"]["isError"], false, "{reply}");
            let (_, reply) = native_request(
                &s,
                &observed,
                "tools/call",
                json!({"name":"t3_thread_organize","arguments":{"action":"unpin"}}),
            );
            assert_eq!(
                reply["result"]["structuredContent"]["code"], "parent_not_active",
                "{reply}"
            );
            if index == 0 {
                s.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.deletedAt','2026-11-04T00:00:00Z') WHERE kind='project' AND id='p'",[]).unwrap();
                let (_, missing) = native_request(
                    &s,
                    &observed,
                    "tools/call",
                    json!({"name":"t3_worktree_status","arguments":{}}),
                );
                assert_eq!(
                    missing["result"]["structuredContent"]["_tag"],
                    "WorktreeMcpFailure"
                );
                assert_eq!(
                    missing["result"]["structuredContent"]["code"],
                    "project_not_found"
                );
                s.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.deletedAt',NULL) WHERE kind='project' AND id='p'",[]).unwrap();
            }
            let auth = native_auth(&observed);
            for (method, path) in [
                ("GET", "/api/projects"),
                ("POST", "/api/auth/websocket-ticket"),
                ("GET", "/ws?orchestrationProtocol=2"),
            ] {
                assert_eq!(
                    common::http(&s.base, method, path, &[("authorization", &auth)], b"").0,
                    401
                );
            }
            let token = auth.strip_prefix("Bearer ").unwrap();
            let db = s.state.db.lock().unwrap();
            for table in ["records", "auth_sessions"] {
                let exists: bool=db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE instr(CAST({} AS TEXT),?1)>0)",if table=="auth_sessions"{"session_id"}else if table=="settings"{"value"}else{"body"}),[token],|r|r.get(0)).unwrap();
                assert!(!exists, "native credential must not be stored in {table}");
            }
            drop(db);
            assert!(!std::fs::read_to_string(s.dir.join("settings.json"))
                .unwrap_or_default()
                .contains(token));
            previous = Some(observed);
            if index == 2 {
                let pid = previous.as_ref().unwrap()["pid"].as_u64().unwrap() as i32;
                // SAFETY: this is the isolated native peer created by the fixture.
                assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
                assert!(support::wait_for(
                    Duration::from_secs(2),
                    || native_request(&s, previous.as_ref().unwrap(), "ping", json!({})).0 == 401
                ));
            }
            let tabs = t3_protocol::api::preview::list(&c, "t").unwrap();
            assert!(
                tabs["sessions"].as_array().unwrap().is_empty(),
                "session tabs must be removed"
            );
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            claude.shutdown().await;
            codex.shutdown().await;
        });
        assert_eq!(
            native_request(&s, &previous.unwrap(), "ping", json!({})).0,
            401
        );
        if s.dir.join("browser-profiles").exists() {
            assert_eq!(
                std::fs::read_dir(s.dir.join("browser-profiles"))
                    .unwrap()
                    .count(),
                2,
                "Recording executor retains both isolated profile fixtures"
            );
            let plans = removals.0.lock().unwrap();
            assert_eq!(
                plans.len(),
                2,
                "Both completed browser profiles have removal plans"
            );
            for plan in plans.iter() {
                let root = &plan.operations().last().unwrap().path;
                assert!(root.starts_with(s.dir.join("browser-profiles")));
                assert!(root.join("fixture.pid").is_file());
            }
        }
    }
}

fn native_auth(v: &serde_json::Value) -> String {
    v["config"]["headers"]["Authorization"]
        .as_str()
        .or(v["config"]["http_headers"]["Authorization"].as_str())
        .unwrap()
        .to_owned()
}
fn native_request(
    s: &support::Server,
    v: &serde_json::Value,
    method: &str,
    params: serde_json::Value,
) -> (u16, serde_json::Value) {
    let auth = native_auth(v);
    let (code, _, body) = common::http(
        &s.base,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-session-id", v["session"].as_str().unwrap()),
            ("t3-mode-limit", "invalid-native-header-is-ignored"),
        ],
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            .to_string()
            .as_bytes(),
    );
    (code, serde_json::from_slice(&body).unwrap())
}

#[test]
fn actual_mcp_dialog_status_and_answer_bypass_a_held_owned_evaluate() {
    let _browser_environment = BROWSER_ENVIRONMENT.lock().unwrap();
    use std::{sync::Arc, time::Duration};
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/browser-cdp-events.py"),
    );
    let provider = Arc::new(o3_server::codex::CodexProvider::new(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-browser-peer.py"),
        None,
    ));
    let removals = Arc::new(support::profile_removal::Recording::default());
    let server = support::start(|state| {
        state
            .streams
            .p2
            .preview
            .set_profile_removal(removals.clone());
        *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: None,
            codex: provider.clone(),
        }));
    });
    let client = server.connect();
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Private dialog","workspaceRoot":server.dir})).unwrap();
    client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Dialog","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
    client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"higher","threadId":"higher","projectId":"p","title":"Higher","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let capture = server.dir.join("dialog-observation.json");
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"turn","messageId":"turn","threadId":"t","text":format!("dialog|{}",capture.display()),"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(15), || {
        let projection = t3_protocol::api::orchestration::thread_projection(&client, "t").unwrap();
        let runs = projection["runs"].as_array().unwrap();
        assert!(
            !runs.iter().any(|run| run["status"] == "failed"),
            "{projection}"
        );
        runs.len() == 1 && runs[0]["status"] == "completed"
    }));
    assert!(capture.exists());
}
