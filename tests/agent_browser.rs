mod common;
mod support;
use serde_json::json;
static BROWSER_ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn assert_native_keyboard_packets(packets: &[serde_json::Value]) {
    let input = packets
        .iter()
        .filter(|packet| packet["method"] == "Input.dispatchKeyEvent")
        .map(|packet| &packet["params"])
        .collect::<Vec<_>>();
    assert!(input.iter().any(|event| event["type"] == "rawKeyDown"
        && event["code"] == "KeyV"
        && event["key"] == "v"
        && event["modifiers"] == 2
        && event["text"] == ""));
    assert!(
        input
            .iter()
            .any(|event| event["code"] == "ControlLeft" && event["autoRepeat"] == true),
        "failed chord preserves its already-pressed modifier"
    );
    assert!(input.iter().any(|event| event["type"] == "keyDown"
        && event["code"] == "Digit1"
        && event["text"] == "!"
        && event["modifiers"] == 8));
    assert!(input.iter().any(|event| event["type"] == "keyUp"
        && event["code"] == "ControlLeft"
        && event["modifiers"] == 0));
    assert!(
        input.iter().all(|event| event["key"]
            .as_str()
            .is_some_and(|key| !key.contains("Control+"))),
        "a chord is never sent as a literal CDP key"
    );
}
#[test]
fn provider_mcp_uses_the_owned_desktop_without_a_local_browser() {
    let _browser_environment = BROWSER_ENVIRONMENT.lock().unwrap();
    use std::{sync::Arc, time::Duration};
    let previous = std::env::var_os("O3_BROWSER_BINARY");
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("O3_BROWSER_BINARY", value),
                None => std::env::remove_var("O3_BROWSER_BINARY"),
            }
        }
    }
    let _restore = Restore(previous);
    std::env::set_var("O3_BROWSER_BINARY", "/no-such-owned-browser");
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
    assert_eq!(
        t3_protocol::api::server::config(&client).unwrap()["environment"]["capabilities"]
            ["agentBrowserAccess"],
        false
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (driver, peer, messages) =
        runtime.block_on(support::desktop_pipe::install(server.state.clone()));
    assert_eq!(
        t3_protocol::api::server::config(&client).unwrap()["environment"]["capabilities"]
            ["agentBrowserAccess"],
        true
    );
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Private desktop","workspaceRoot":server.dir})).unwrap();
    for (id, mode) in [("t", "approval-required"), ("higher", "full-access")] {
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":id,"threadId":id,"projectId":"p","title":"Desktop","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":mode,"interactionMode":"default"})).unwrap();
    }
    let capture = server.dir.join("desktop-observation.json");
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"turn","messageId":"turn","threadId":"t","text":format!("enabled|{}",capture.display()),"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(15), || {
        let projection = t3_protocol::api::orchestration::thread_projection(&client, "t").unwrap();
        let runs = projection["runs"].as_array().unwrap();
        assert!(
            !runs.iter().any(|run| run["status"] == "failed"),
            "{projection}"
        );
        runs.len() == 1 && runs[0]["status"] == "completed"
    }));
    assert!(capture.is_file());
    assert!(
        !server.dir.join("browser-profiles").exists(),
        "desktop admission never starts a local profile"
    );
    let methods = messages
        .lock()
        .unwrap()
        .iter()
        .filter(|message| message["type"] == "cdp")
        .map(|message| {
            serde_json::from_str::<serde_json::Value>(message["message"].as_str().unwrap()).unwrap()
                ["method"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    for method in [
        "Target.getTargets",
        "Target.attachToTarget",
        "Page.navigate",
        "Runtime.evaluate",
    ] {
        assert!(methods.iter().any(|actual| actual == method), "{methods:?}");
    }
    assert!(!methods
        .iter()
        .any(|method| method == "Target.createTarget" || method == "Browser.close"));
    let packets = messages
        .lock()
        .unwrap()
        .iter()
        .filter(|message| message["type"] == "cdp")
        .map(|message| serde_json::from_str(message["message"].as_str().unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert_native_keyboard_packets(&packets);
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"enableAgentBrowserAccess":false}}),
        )
        .unwrap();
    assert_eq!(
        t3_protocol::api::server::config(&client).unwrap()["environment"]["capabilities"]
            ["agentBrowserAccess"],
        false
    );
    runtime.block_on(async {
        provider.shutdown().await;
        o3_server::preview::shutdown(&server.state).await;
        driver.shutdown().await;
        peer.abort();
        let _ = peer.await;
    });
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"enableAgentBrowserAccess":true}}),
        )
        .unwrap();
    assert_eq!(
        t3_protocol::api::server::config(&client).unwrap()["environment"]["capabilities"]
            ["agentBrowserAccess"],
        false
    );
    assert!(
        removals.0.lock().unwrap().is_empty(),
        "desktop route has no profile removal plan"
    );
}
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
        let mut retained_tab = None;
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
            let sessions = tabs["sessions"].as_array().unwrap();
            assert_eq!(
                sessions.len(),
                1,
                "foreground completion and provider revocation retain the user's native tab"
            );
            let current = sessions[0]["tabId"].as_str().unwrap().to_owned();
            if let Some(expected) = &retained_tab {
                assert_eq!(
                    expected, &current,
                    "default open reuses the same native tab"
                );
            } else {
                retained_tab = Some(current);
            }
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
        c.rpc(
            "preview.close",
            json!({"threadId":"t","tabId":retained_tab.unwrap()}),
        )
        .unwrap();
        if s.dir.join("browser-profiles").exists() {
            assert_eq!(
                std::fs::read_dir(s.dir.join("browser-profiles"))
                    .unwrap()
                    .count(),
                1,
                "Recording executor retains the reused isolated profile fixture"
            );
            let plans = removals.0.lock().unwrap();
            assert_eq!(
                plans.len(),
                1,
                "Explicit close prepares one removal plan after the retained tab stops"
            );
            for plan in plans.iter() {
                let root = &plan.operations().last().unwrap().path;
                assert!(root.starts_with(s.dir.join("browser-profiles")));
                assert!(root.join("fixture.pid").is_file());
                let packets = std::fs::read_to_string(root.join("commands.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<_>>();
                assert_native_keyboard_packets(&packets);
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
