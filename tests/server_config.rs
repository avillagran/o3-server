mod support;
use o3_server::provider::{ConfiguredProviders, Provider, ScriptedProvider};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[test]
fn custom_instances_route_native_binary_environment_and_home_and_reconfigure() {
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(ConfiguredProviders::new(
            Arc::new(ScriptedProvider),
            s.dir.clone(),
        )));
    });
    let c = server.connect();
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/input/instance.py");
    let workspace = server.dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Instances","workspaceRoot":workspace})).unwrap();
    for (id, driver) in [
        ("claudeA", "claudeAgent"),
        ("claudeB", "claudeAgent"),
        ("codexA", "codex"),
        ("codexB", "codex"),
    ] {
        let home = server.dir.join(id);
        std::fs::create_dir_all(home.join("skills/test")).unwrap();
        std::fs::write(
            home.join("skills/test/SKILL.md"),
            "---\nname: isolated-skill\ndescription: test\n---\n",
        )
        .unwrap();
        c.rpc("server.updateSettings",json!({"patch":{},"providerInstanceMutation":{"operation":"create","instanceId":id,"instance":{"driver":driver,"config":{"binaryPath":binary,"homePath":home,"launchArgs":"--instance-marker \"quoted value\""},"environment":[{"name":"INSTANCE_MARKER","value":id,"sensitive":true}]}}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let config = c.rpc("server.getConfig", json!({})).unwrap();
            let provider = config["providers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["instanceId"] == id)
                .unwrap();
            if if driver == "codex" {
                provider["skills"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["name"] == "native-skill")
            } else {
                provider["slashCommands"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["name"] == "native-command")
            } {
                break;
            }
            assert!(Instant::now() < deadline, "{provider}");
            std::thread::sleep(Duration::from_millis(20));
        }
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        let provider = config["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["instanceId"] == id)
            .unwrap();
        assert!(t3_protocol::models::catalog(&config)["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["instanceId"] == id && m["available"] == true));
        let native =
            t3_protocol::provider_instances::projection(&config["settings"], &config, true);
        let row = native["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap();
        assert_eq!(row["canAuthenticate"], true);
        assert_eq!(row["canLogout"], true);
        assert_eq!(row["canInstall"], false);
        assert_eq!(row["canUpdate"], driver == "claudeAgent");
        assert_eq!(row["versionStatus"], "unknown");
        assert!(
            provider["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "isolated-skill"),
            "{provider}"
        );
        if driver == "codex" {
            assert!(provider["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "native-skill"));
            assert!(provider["models"][0]["capabilities"]["optionDescriptors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["id"] == "serviceTier"));
        } else {
            assert!(provider["slashCommands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == "native-command"));
        }
        c.rpc("orchestration.launchThread",json!({"commandId":id,"threadId":id,"projectId":"p","title":id,"modelSelection":{"instanceId":id,"model":"test","options":if driver=="codex" {json!([{"id":"serviceTier","value":"fast"}])}else{json!([])}},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"instance proof","attachments":[]}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let p = c
                .rpc("orchestration.getThreadProjection", json!({"threadId":id}))
                .unwrap();
            assert_ne!(p["runs"][0]["status"], "failed", "{p}");
            if p["runs"][0]["status"] == "completed" {
                assert_eq!(p["thread"]["providerInstanceId"], id);
                break;
            }
            assert!(Instant::now() < deadline, "{p}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(home.join("starts")).unwrap(),
            "start\nstart\n"
        );
    }
    let changed=c.rpc("server.updateSettings",json!({"patch":{"idleCompaction":"off","backgroundActivity":{"profile":"battery-saver"},"projectSettingsOverrides":{"p":{"defaultAutoPull":true}}}})).unwrap();
    assert_eq!(changed["compactIdleThreads"], false);
    assert_eq!(changed["backgroundActivityProfile"], "battery-saver");
    assert_eq!(changed["projectAutoPullOverrides"]["p"], true);
    c.rpc(
        "server.updateSettings",
        json!({"patch":{},"providerInstanceMutation":{"operation":"remove","instanceId":"codexB"}}),
    )
    .unwrap();
    assert!(!c.rpc("server.getConfig", json!({})).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["instanceId"] == "codexB"));
}
#[test]
fn failed_native_discovery_does_not_report_an_installed_binary_as_missing() {
    use std::os::unix::fs::PermissionsExt;
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(ConfiguredProviders::new(
            Arc::new(ScriptedProvider),
            s.dir.clone(),
        )));
    });
    let binary = server.dir.join("unhealthy-codex");
    std::fs::write(&binary, "#!/bin/sh\nexit 17\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let c = server.connect();
    c.rpc("server.updateSettings",json!({"patch":{},"providerInstanceMutation":{"operation":"create","instanceId":"unhealthy","instance":{"driver":"codex","config":{"binaryPath":binary}}}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        let provider = config["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["instanceId"] == "unhealthy")
            .unwrap();
        if provider["status"] == "error" {
            assert_eq!(provider["installed"], true);
            break;
        }
        assert!(Instant::now() < deadline, "{provider}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Polls(Arc<AtomicUsize>);
impl Provider for Polls {
    fn config(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"instanceId":"probe","driver":"codex","displayName":format!("Probe {}",self.0.load(Ordering::SeqCst)),"enabled":true,"installed":true}),
        ]
    }

    fn poll_usage<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn execute<'a>(
        &'a self,
        i: o3_server::provider::RunInput,
        o: tokio::sync::mpsc::Sender<o3_server::provider::ProviderEvent>,
        c: tokio::sync::mpsc::Receiver<o3_server::provider::ProviderControl>,
        x: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        ScriptedProvider.execute(i, o, c, x)
    }
}
#[test]
fn config_sources_are_opt_in_cancelled_and_themes_are_typed() {
    let polls = Arc::new(AtomicUsize::new(0));
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(Polls(polls.clone())));
    });
    let c = server.connect();
    let mut plain = support::subscribe(server.connect(), "subscribeServerConfig", json!({}));
    assert_eq!(plain.next(Duration::from_secs(2))["type"], "snapshot");
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let mut stream = support::subscribe(
        server.connect(),
        "server.subscribeServerConfig",
        json!({"usageLimitSources":true,"environmentThemes":true}),
    );
    assert_eq!(stream.next(Duration::from_secs(2))["type"], "snapshot");
    stream.until(Duration::from_secs(2), |v| {
        v["type"] == "usageLimitSourcesUpdated"
    });
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    let themes = server.dir.join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(
        themes.join("sample.json"),
        json!({"name":"Sample","appearance":"dark","canvas":"#112233","accent":"#aabbcc"})
            .to_string(),
    )
    .unwrap();
    let theme = stream.until(Duration::from_secs(3), |v| {
        v["type"] == "environmentThemesUpdated"
            && v["payload"]["themes"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
    });
    assert_eq!(
        theme.last().unwrap()["payload"]["themes"][0]["id"],
        "sample"
    );
    assert_eq!(
        polls.load(Ordering::SeqCst),
        1,
        "theme watcher must not refresh quotas"
    );
    c.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"missing":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"","enabled":true}}}})).unwrap();
    let sources = stream.until(Duration::from_secs(3), |v| {
        v["type"] == "usageLimitSourcesUpdated" && v["payload"]["sources"][0]["id"] == "missing"
    });
    assert!(sources.last().unwrap()["payload"]["sources"][0]["error"].is_string());
    // A coalesced settings/provider change sends a replacement snapshot. The
    // real quota reducer clears source state there, so cached extensions follow it.
    while stream.try_next(Duration::from_millis(50)).is_some() {}
    polls.fetch_add(1, Ordering::SeqCst);
    o3_server::settings::update(&server.state, &json!({"patch":{"defaultTheme":"dark"}})).unwrap();
    stream.until(Duration::from_secs(3), |v| v["type"] == "snapshot");
    let replay = stream.until(Duration::from_secs(3), |v| {
        v["type"] == "usageLimitSourcesUpdated"
    });
    assert_eq!(
        replay.last().unwrap()["payload"]["sources"][0]["id"],
        "missing"
    );
    stream.cancel();
    std::thread::sleep(Duration::from_millis(300));
    let count = polls.load(Ordering::SeqCst);
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"usageLimitSources":{"missing":null}}}),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(polls.load(Ordering::SeqCst), count);
    assert!(plain.try_next(Duration::from_millis(100)).is_some());
    assert!(!plain.seen.iter().any(|v| matches!(
        v["type"].as_str(),
        Some("usageLimitSourcesUpdated" | "environmentThemesUpdated")
    )));
    plain.cancel();
}

#[test]
fn hub_account_windows_and_failures_reach_the_real_quota_reducer() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let hub = std::thread::spawn(move || {
        for mut socket in listener.incoming().take(3).flatten() {
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut b = [0; 4096];
            loop {
                let n = socket.read(&mut b).unwrap();
                bytes.extend_from_slice(&b[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("Authorization: Bearer test-management"));
            let response = if text.starts_with("GET /v0/management/auth-files") {
                json!({"files":[{"id":"a","auth_index":"idx","provider":"codex","email":"person@example.invalid"},{"id":"disabled","provider":"claude","disabled":true}]})
            } else if text.contains("rate-limit-reset-credits") {
                json!({"status_code":200,"body":json!({"credits":[{"id":"private-credit","status":"available","reset_type":"codex_rate_limits","expires_at":"2099-01-01T00:00:00Z"}]}).to_string()})
            } else {
                json!({"status_code":200,"body":json!({"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":38,"reset_at":1893456000i64,"limit_window_seconds":18000},"secondary_window":{"used_percent":67}}}).to_string()})
            };
            let body = response.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",body.len(),body).unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    let server = support::start(|_| {});
    let c = server.connect();
    c.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"hub":{"kind":"cliproxy","url":url,"managementKey":"test-management","enabled":true}}}})).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut stream = support::subscribe(
        server.connect(),
        "subscribeServerConfig",
        json!({"usageLimitSources":true}),
    );
    let snapshot = stream.next(Duration::from_secs(2));
    let mut state = json!({});
    t3_protocol::quota_sources::apply(&mut state, &snapshot);
    let events = stream.until(Duration::from_secs(5), |v| {
        v["type"] == "usageLimitSourcesUpdated"
    });
    assert!(t3_protocol::quota_sources::apply(
        &mut state,
        events.last().unwrap()
    ));
    let account = &state["usageLimitSources"][0]["accounts"][0];
    assert_eq!(account["usageLimits"]["windows"][0]["usedPercent"], 38.0);
    assert_eq!(account["usageLimits"]["resetCredits"]["availableCount"], 1);
    assert!(!state.to_string().contains("test-management"));
    assert!(!state.to_string().contains("private-credit"));
    stream.cancel();
    hub.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn normalized_settings_are_readable_by_native_settings_and_atomic_on_rejection() {
    let server = support::start(|_| {});
    let c = server.connect();
    let update = |patch| {
        c.rpc("server.updateSettings", json!({"patch":patch}))
            .unwrap()
    };
    let initial = c.rpc("server.getSettings", json!({})).unwrap();
    assert_eq!(initial["sidebarAutoSettleAfterDays"], 3);
    assert_eq!(
        initial["sourceControlWritingStyle"]["followChangeRequestTemplates"],
        true
    );
    let normalized = update(
        json!({"branchNamingMode":"semantic","branchNamePrefix":"  task  ","compactIdleThreads":true,"backgroundActivity":{"profile":"custom","baseProfile":"balanced","overrides":{"providerHealthRefreshInterval":1234}},"textGenerationModelSelection":{"model":"test"},"projectSettingsOverrides":{"p":{"defaultAutoPull":true,"enableAgentBrowserAccess":false}}}),
    );
    assert_eq!(normalized["branchNamePrefix"], "task");
    assert_eq!(normalized["idleCompaction"], "beforeCacheExpires");
    assert_eq!(normalized["providerHealthRefreshInterval"], 1234);
    assert_eq!(
        normalized["textGenerationModelSelection"]["instanceId"],
        "codex"
    );
    assert_eq!(
        normalized["textGenerationModelSelection"]["options"],
        json!([])
    );
    let view = t3_protocol::native_settings::snapshot(&normalized);
    assert!(view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["key"] == "branchNamingMode" && f["value"] == "semantic"));
    let preset = update(json!({"backgroundActivity":{"profile":"performance"}}));
    assert_eq!(preset["backgroundActivity"]["overrides"], json!({}));
    assert_eq!(preset["providerHealthRefreshInterval"], 60000);
    assert_eq!(
        update(json!({"sidebarAutoSettleAfterDays":null}))["sidebarAutoSettleAfterDays"],
        serde_json::Value::Null
    );
    let before = c.rpc("server.getSettings", json!({})).unwrap();
    assert!(c
        .rpc(
            "server.updateSettings",
            json!({"patch":{"branchNamingMode":"invalid"}})
        )
        .is_err());
    assert_eq!(c.rpc("server.getSettings", json!({})).unwrap(), before);
}
