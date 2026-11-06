//! Managed launch pins must reach the actual native transport without changing stored preferences.
mod support;
use o3_server::managed_policy::Policy;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};

#[test]
fn managed_pins_control_both_native_transports_and_preserve_stored_values() {
    use std::os::unix::fs::PermissionsExt;
    assert!(Path::new("/run/.containerenv").is_file());
    for (driver, whole_instance) in [
        ("codex", false),
        ("claudeAgent", false),
        ("codex", true),
        ("claudeAgent", true),
    ] {
        let server = support::start(|state| {
            let home = state.dir.join("managed-home");
            std::fs::create_dir(&home).unwrap();
            std::fs::write(home.join("auth.json"), r#"{"fixtureOwner":"managed"}"#).unwrap();
            let peer = state.dir.join("managed-peer.py");
            let reference =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-home-peer.py");
            let program = format!(
                "#!/usr/bin/python3\nimport os,sys,json,runpy\nfrom pathlib import Path\nPath({:?}).write_text(json.dumps({{'argv':sys.argv,'token':os.environ.get('MANAGED_TOKEN')}}))\nrunpy.run_path({:?},run_name='__main__')\n",
                state.dir.join("launch.json").to_str().unwrap(),
                reference.to_str().unwrap()
            );
            std::fs::write(&peer, program).unwrap();
            std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
            let home_variable = if driver == "codex" {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            };
            let environment = json!([{"name":home_variable,"value":home,"sensitive":false},{"name":"MANAGED_TOKEN","value":"managed","sensitive":false}]);
            let mut launch_environment = environment.clone();
            for variable in launch_environment.as_array_mut().unwrap() {
                variable.as_object_mut().unwrap().remove("sensitive");
            }
            let document = if whole_instance {
                json!({"schemaVersion":1,"organization":"Fixture Corp","settings":{format!("providerInstances.{driver}"):{"locked":true,"value":{"driver":driver,"enabled":true,"config":{"binaryPath":peer,"launchArgs":"--expected-owner managed"},"environment":environment}}}})
            } else {
                json!({"schemaVersion":1,"organization":"Fixture Corp","limits":{"providers":{"instances":{driver:{"locked":true,"binaryPath":peer,"launchArgs":"--expected-owner managed","environment":launch_environment}}}}})
            };
            let policy =
                Policy::parse(Path::new("/fixture/policy.json"), &document.to_string()).unwrap();

            std::fs::write(state.dir.join("settings.json"), json!({"providerInstances":{driver:{"driver":driver,"enabled":true,"config":{"binaryPath":"/missing/user-binary","launchArgs":"--user"},"environment":[{"name":"MANAGED_TOKEN","value":"user","sensitive":true}]}}}).to_string()).unwrap();
            *state.provider.lock().unwrap() = Some(Arc::new(
                o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    state.dir.clone(),
                )
                .with_policy(policy),
            ));
        });
        let client = server.connect();
        let workspace = server.dir.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Managed","workspaceRoot":workspace})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Managed","modelSelection":{"instanceId":driver,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"m","messageId":"m","threadId":"t","modelSelection":{"instanceId":driver,"model":"test"},"text":"Synthetic managed transport","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(
            support::wait_for(Duration::from_secs(20), || {
                let projection = client
                    .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                    .unwrap();
                projection["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|run| run["status"] == "completed")
            }),
            "{driver}"
        );
        let launch: Value =
            serde_json::from_slice(&std::fs::read(server.dir.join("launch.json")).unwrap())
                .unwrap();
        assert_eq!(launch["token"], "managed");
        assert!(launch["argv"]
            .as_array()
            .unwrap()
            .contains(&json!("--expected-owner")));
        assert!(!launch["argv"]
            .as_array()
            .unwrap()
            .contains(&json!("--user")));
        let stored: Value =
            serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            stored["providerInstances"][driver]["config"]["binaryPath"],
            "/missing/user-binary"
        );
        assert_eq!(
            stored["providerInstances"][driver]["environment"][0]["value"],
            "user"
        );
    }
}

#[tokio::test]
async fn disallowed_driver_cannot_launch_during_discovery_or_usage_polling() {
    use o3_server::provider::Provider;
    use std::os::unix::fs::PermissionsExt;
    assert!(Path::new("/run/.containerenv").is_file());
    let directory = std::env::temp_dir().join(format!(
        "managed-blocked-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&directory).unwrap();
    let marker = directory.join("started");
    let peer = directory.join("forbidden-peer.py");
    std::fs::write(
        &peer,
        format!(
            "#!/usr/bin/python3\nfrom pathlib import Path\nPath({:?}).write_text('forbidden')\n",
            marker.to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(directory.join("settings.json"), json!({"providerInstances":{"blocked":{"driver":"claudeAgent","enabled":true,"config":{"binaryPath":peer}}}}).to_string()).unwrap();
    let policy = Policy::parse(Path::new("/fixture/policy.json"), r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"providers":{"allowedDrivers":["codex"]}}}"#).unwrap();
    let providers = o3_server::provider::ConfiguredProviders::new(
        Arc::new(o3_server::provider::ScriptedProvider),
        directory.clone(),
    )
    .with_policy(policy);
    providers.discover(directory).await.unwrap();
    providers.poll_usage().await.unwrap();
    providers.refresh().await.unwrap();
    let config = providers.config();
    let blocked = config
        .iter()
        .find(|provider| provider["instanceId"] == "blocked")
        .unwrap();
    assert_eq!(blocked["availability"], "unavailable");
    assert_eq!(blocked["enabled"], false);
    assert_eq!(blocked["installed"], false);
    assert!(!marker.exists());
}

#[path = "support/reference.rs"]
mod reference;

#[test]
#[ignore = "Use tools/test-managed-provider-reference with a private container policy directory"]
fn official_provider_rpc_overlay_and_rejections_match_native_policy_contract() {
    assert_eq!(
        std::env::var("O3_POLICY_FIXED_PATH_CONTAINER").as_deref(),
        Ok("1")
    );
    assert!(Path::new("/run/.containerenv").is_file());
    let policy = Path::new("/etc/t3code/policy.json");
    std::fs::write(policy, json!({"schemaVersion":1,"organization":"Fixture Corp","settings":{"enableAgentDeviceAccess":{"locked":true,"value":false},"worktreesDirectory":{"locked":false,"value":"/managed/default-worktrees"}},"limits":{"maxRuntimeMode":"approval-required","providers":{"allowedDrivers":["codex"],"instances":{"codex":{"locked":true,"binaryPath":"/managed/codex","launchArgs":"--managed"}}}}}).to_string()).unwrap();
    let root = std::env::temp_dir().join(format!(
        "managed-native-rpc-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let raw_path = root.join("reference/userdata/settings.json");
    std::fs::create_dir_all(raw_path.parent().unwrap()).unwrap();
    std::fs::write(&raw_path, json!({"enableAgentDeviceAccess":true,"responseStreamingMode":"token","defaultRuntimeMode":"full-access","projectSettingsOverrides":{"p":{"defaultRuntimeMode":"auto"}}}).to_string()).unwrap();
    let (client, native) = reference::start(&root);
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"managed-project","projectId":"p","title":"Managed project","workspaceRoot":root})).unwrap();
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"projectSettingsOverrides":{"p":{"defaultRuntimeMode":"auto"}}}}),
        )
        .unwrap();
    let settings = client.rpc("server.getConfig", json!({})).unwrap()["settings"].clone();
    assert_eq!(
        settings["providerInstances"]["codex"]["config"]["binaryPath"],
        "/managed/codex"
    );
    assert_eq!(
        settings["providerInstances"]["codex"]["config"]["launchArgs"],
        "--managed"
    );
    assert_eq!(settings["enableAgentDeviceAccess"], false);
    assert_eq!(settings["worktreesDirectory"], "/managed/default-worktrees");
    assert_eq!(settings["responseStreamingMode"], "paragraph");
    assert_eq!(settings["defaultRuntimeMode"], "approval-required");
    assert_eq!(
        settings["projectSettingsOverrides"]["p"]["defaultRuntimeMode"],
        "approval-required"
    );
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"defaultAutoPull":true,"enableAgentDeviceAccess":false}}),
        )
        .unwrap();
    let stored: Value = serde_json::from_slice(&std::fs::read(&raw_path).unwrap()).unwrap();
    assert_eq!(stored["enableAgentDeviceAccess"], true);
    assert_eq!(stored["defaultAutoPull"], true);
    assert!(stored.get("worktreesDirectory").is_none());
    fn failure(value: &Value) -> Option<Value> {
        if value["_tag"] == "ServerSettingsError" {
            return Some(value.clone());
        }
        match value {
            Value::Object(fields) => fields.values().find_map(failure),
            Value::Array(values) => values.iter().find_map(failure),
            _ => None,
        }
    }
    for (patch, expected) in [
        (
            json!({"providerInstances":{"codex":{"driver":"codex","config":{"binaryPath":"/changed/codex"}}}}),
            json!({"operation":"write-managed-setting","managedSetting":"providerInstances.codex.config.binaryPath","managedBy":"Fixture Corp"}),
        ),
        (
            json!({"providerInstances":{"blocked":{"driver":"claudeAgent","enabled":true}}}),
            json!({"operation":"use-disallowed-provider","providerInstanceId":"blocked","providerLabel":"Claude","managedBy":"Fixture Corp"}),
        ),
    ] {
        let exit = native.raw_exit("server.updateSettings", json!({"patch":patch}));
        let error =
            failure(&exit).unwrap_or_else(|| panic!("Expected native policy failure: {exit}"));
        for (field, value) in expected.as_object().unwrap() {
            assert_eq!(&error[field], value, "{exit}");
        }
    }
    let settings = client.rpc("server.getConfig", json!({})).unwrap()["settings"].clone();
    assert_eq!(
        settings["providerInstances"]["codex"]["config"]["binaryPath"],
        "/managed/codex"
    );
    assert!(settings["providerInstances"].get("blocked").is_none());
    println!(
        "Official native RPC overlay and typed locked/disallowed rejections verified; no inference"
    );
}
