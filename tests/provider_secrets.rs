//! Private synthetic vault values must reach actual transports without entering stored settings.
mod support;
#[path = "support/sandbox.rs"]
mod test_sandbox;
use o3_server::{
    provider::{ConfiguredProviders, Provider, ScriptedProvider},
    secret_resolver::{OnePassword, SecretResolver},
};
use serde_json::{json, Value};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::{path::Path, sync::Arc, time::Duration};
const REFERENCE: &str = "op://Private/Fixture/key";
fn copy_executable(source: &Path, target: &Path) {
    std::fs::copy(source, target).unwrap();
    std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o700)).unwrap();
}
fn answers(root: &Path, secret: Option<&str>) {
    let result = secret.map_or_else(|| json!({"code":1}), |value| json!({"stdout":value}));
    let path = root.join("vault/case.tmp");
    std::fs::write(&path, json!({"read":{REFERENCE:result}}).to_string()).unwrap();
    std::fs::rename(path, root.join("vault/case.json")).unwrap();
}
fn setup(root: &Path, driver: &str, secret: Option<&str>) -> (Arc<SecretResolver>, Value) {
    test_sandbox::assert_isolated();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(root.join("vault"))
        .unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/secret-op.py");
    let op = root.join("vault/op");
    copy_executable(&source, &op);
    answers(root, secret);
    let home = root.join("native-home");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&home)
        .unwrap();
    std::fs::write(home.join("auth.json"), r#"{"fixtureOwner":"private"}"#).unwrap();
    let peer = root.join("peer.py");
    let program = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider-home-peer.py"),
    )
    .unwrap();
    let header=format!("#!/usr/bin/python3\nimport os,sys,json\nfrom pathlib import Path\nwith Path({:?}).open('a') as trace: trace.write(json.dumps({{'pid':os.getpid(),'argv':sys.argv,'key':os.environ.get('KEY')}})+'\\n')\n",root.join("launches.jsonl").to_str().unwrap());
    std::fs::write(&peer, header + &program).unwrap();
    std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
    let resolver = Arc::new(SecretResolver::new(vec![Arc::new(
        OnePassword::with_binary(op),
    )]));
    let mut config = json!({"binaryPath":peer,"homePath":home});
    if driver == "codex" {
        config["setupMode"] = json!("existing")
    }
    let entry = json!({"driver":driver,"enabled":true,"config":config,"environment":[{"name":"KEY","value":"","secret":{"provider":"vault","reference":REFERENCE},"sensitive":true}]});
    (
        resolver,
        json!({"secretProviders":{"vault":{"kind":"one_password","label":"Fixture vault","config":{"account":"fixture"}}},"providerInstances":{"vaulted":entry}}),
    )
}
fn calls(root: &Path) -> usize {
    std::fs::read_to_string(root.join("vault/calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn launches(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("launches.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|v| serde_json::from_str(v).unwrap())
        .collect()
}
fn dispatch(client: &t3_protocol::Connection, message: &str) {
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":message,"messageId":message,"threadId":"thread","modelSelection":{"instanceId":"vaulted","model":"test"},"text":"Private synthetic native transport","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(20), || {
        let projection = client
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":"thread"}),
            )
            .unwrap();
        projection["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == message)
            && projection["runs"]
                .as_array()
                .unwrap()
                .last()
                .is_some_and(|run| run["status"] == "completed")
    }));
}
#[test]
fn vault_references_feed_both_native_transports_without_persisting_resolved_values() {
    for driver in ["codex", "claudeAgent"] {
        let server = support::start(|state| {
            let (resolver, settings) = setup(&state.dir, driver, Some("resolved-one"));
            std::fs::write(state.dir.join("settings.json"), settings.to_string()).unwrap();
            assert!(Arc::ptr_eq(
                &state.secret_resolver(),
                &state.secret_resolver()
            ));
            *state.provider.lock().unwrap() = Some(Arc::new(
                ConfiguredProviders::new(Arc::new(ScriptedProvider), state.dir.clone())
                    .with_secret_resolver(resolver)
                    .with_notifications(state.changed.clone()),
            ));
        });
        let client = server.connect();
        client.rpc("server.refreshProviders", json!({})).unwrap();
        assert!(support::wait_for(Duration::from_secs(5), || client
            .rpc("server.getConfig", json!({}))
            .unwrap()["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |p| p["instanceId"] == "vaulted" && p["availability"] != "unavailable"
            )));
        let workspace = server.dir.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"project","title":"Private","workspaceRoot":workspace})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"thread","projectId":"project","title":"Private","modelSelection":{"instanceId":"vaulted","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        dispatch(&client, "first");
        assert_eq!(calls(&server.dir), 1);
        assert!(launches(&server.dir)
            .iter()
            .any(|v| v["key"] == "resolved-one"));
        client
            .rpc(
                "server.updateSettings",
                json!({"patch":{"defaultAutoPull":true}}),
            )
            .unwrap();
        assert_eq!(calls(&server.dir), 1);
        let stored = std::fs::read_to_string(server.dir.join("settings.json")).unwrap();
        assert!(!stored.contains("resolved-one"));
        assert!(!client
            .rpc("server.getConfig", json!({}))
            .unwrap()
            .to_string()
            .contains("resolved-one"));
        answers(&server.dir, None);
        client
            .rpc(
                "server.refreshProviders",
                json!({"reloadSecrets":true,"instanceId":"vaulted"}),
            )
            .unwrap();
        dispatch(&client, "retained");
        assert_eq!(calls(&server.dir), 2);
        assert_eq!(launches(&server.dir).last().unwrap()["key"], "resolved-one");
        answers(&server.dir, Some("resolved-two"));
        client
            .rpc(
                "server.refreshProviders",
                json!({"reloadSecrets":true,"instanceId":"vaulted"}),
            )
            .unwrap();
        dispatch(&client, "rotated");
        assert_eq!(calls(&server.dir), 3);
        assert_eq!(launches(&server.dir).last().unwrap()["key"], "resolved-two");
        assert!(!std::fs::read_to_string(server.dir.join("settings.json"))
            .unwrap()
            .contains("resolved-two"));
    }
}
#[tokio::test]
async fn unreadable_reference_never_starts_worker_and_shared_cache_survives_registry_rebuild() {
    let root = std::env::temp_dir().join(format!(
        "o3-secret-native-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let (resolver, settings) = setup(&root, "codex", None);
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    let providers = ConfiguredProviders::new(Arc::new(ScriptedProvider), root.clone())
        .with_secret_resolver(resolver.clone());
    providers.discover(root.clone()).await.unwrap();
    providers.poll_usage().await.unwrap();
    assert_eq!(calls(&root), 1);
    assert!(launches(&root).is_empty());
    let unavailable = providers
        .config()
        .into_iter()
        .find(|p| p["instanceId"] == "vaulted")
        .unwrap();
    assert_eq!(
        unavailable["unavailableReason"],
        "Could not read KEY from its secret provider. Unlock it, then refresh."
    );
    drop(providers);
    answers(&root, Some("now-readable"));
    let providers = ConfiguredProviders::new(Arc::new(ScriptedProvider), root.clone())
        .with_secret_resolver(resolver);
    providers.discover(root.clone()).await.unwrap();
    providers.refresh().await.unwrap();
    assert_eq!(calls(&root), 1);
    assert!(launches(&root).is_empty());
    providers.reload_secrets(None).await.unwrap();
    providers.discover(root.clone()).await.unwrap();
    assert_eq!(calls(&root), 2);
    assert!(launches(&root).iter().any(|v| v["key"] == "now-readable"));
}
