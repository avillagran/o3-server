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
    dispatch_to(client, "thread", "vaulted", message)
}
fn dispatch_to(client: &t3_protocol::Connection, thread: &str, instance: &str, message: &str) {
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":message,"messageId":message,"threadId":thread,"modelSelection":{"instanceId":instance,"model":"test"},"text":"Private synthetic native transport","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(20), || {
        let projection = client
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":thread}),
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
                    .with_settings_lock(state.settings_lock.clone())
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

#[test]
fn private_literals_reach_both_transports_and_failed_publication_keeps_old_identity_and_values() {
    for driver in ["codex", "claudeAgent"] {
        let server = support::start(|state| {
            let (resolver, mut settings) = setup(&state.dir, driver, None);
            let variable = &mut settings["providerInstances"]["vaulted"]["environment"][0];
            variable["value"] = json!("private-literal-one");
            variable.as_object_mut().unwrap().remove("secret");
            let neighbor_home = state.dir.join("neighbor-home");
            std::fs::create_dir(&neighbor_home).unwrap();
            std::fs::write(
                neighbor_home.join("auth.json"),
                r#"{"fixtureOwner":"neighbor"}"#,
            )
            .unwrap();
            let mut neighbor = settings["providerInstances"]["vaulted"].clone();
            neighbor["config"]["homePath"] = json!(neighbor_home);
            neighbor["environment"][0]["value"] = json!("private-neighbor-stable");
            settings["providerInstances"]["neighbor"] = neighbor;
            o3_server::settings::update(state, &json!({"patch":settings})).unwrap();
            *state.provider.lock().unwrap() = Some(Arc::new(
                ConfiguredProviders::new(Arc::new(ScriptedProvider), state.dir.clone())
                    .with_secret_resolver(resolver)
                    .with_settings_lock(state.settings_lock.clone())
                    .with_notifications(state.changed.clone()),
            ));
        });
        let signing = server.state.signing_key.clone();
        let environment_id = server.state.environment_id.clone();
        let client = server.connect();
        client.rpc("server.refreshProviders", json!({})).unwrap();
        let workspace = server.dir.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"project","title":"Private literal","workspaceRoot":workspace})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"thread","projectId":"project","title":"Private literal","modelSelection":{"instanceId":"vaulted","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        dispatch(&client, "initial-private");
        assert!(launches(&server.dir)
            .iter()
            .any(|v| v["key"] == "private-literal-one"));
        assert_eq!(calls(&server.dir), 0);
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"neighbor-thread","threadId":"neighbor-thread","projectId":"project","title":"Neighbor","modelSelection":{"instanceId":"neighbor","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        dispatch_to(&client, "neighbor-thread", "neighbor", "neighbor-initial");
        assert!(launches(&server.dir)
            .iter()
            .any(|v| v["key"] == "private-neighbor-stable"));
        let before: Value =
            serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap())
                .unwrap();
        assert!(!before.to_string().contains("private-literal-one"));
        assert_eq!(
            before["providerInstances"]["vaulted"]["environment"][0]["value"],
            ""
        );
        assert_eq!(
            before["providerInstances"]["vaulted"]["environment"][0]["valueRedacted"],
            true
        );
        let public = client.rpc("server.getConfig", json!({})).unwrap();
        assert!(!public.to_string().contains("private-literal-one"));
        client
            .rpc(
                "server.updateSettings",
                json!({"patch":{"providerInstances":public["settings"]["providerInstances"]}}),
            )
            .unwrap();
        dispatch(&client, "echoed-private");
        assert_eq!(
            launches(&server.dir).last().unwrap()["key"],
            "private-literal-one"
        );
        let mut replacement = before["providerInstances"].clone();
        let value = &mut replacement["vaulted"]["environment"][0];
        value["value"] = json!("private-literal-two");
        value.as_object_mut().unwrap().remove("valueRedacted");
        let blocked = server
            .dir
            .join("settings.json")
            .with_extension(format!("{}.tmp", std::process::id()));
        std::fs::create_dir(&blocked).unwrap();
        assert!(client
            .rpc(
                "server.updateSettings",
                json!({"patch":{"providerInstances":replacement}})
            )
            .is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(server.dir.join("settings.json")).unwrap()
            )
            .unwrap(),
            before
        );
        dispatch(&client, "rolled-back-private");
        assert_eq!(
            launches(&server.dir).last().unwrap()["key"],
            "private-literal-one"
        );
        std::fs::rename(&blocked, server.dir.join("retained-publication-blocker")).unwrap();
        // Prime the neighbor after the first lazy metadata discovery. Measure
        // only the following credential-only write, not initial inspection.
        dispatch_to(
            &client,
            "neighbor-thread",
            "neighbor",
            "neighbor-before-rotation",
        );
        let neighbor_launches = launches(&server.dir)
            .iter()
            .filter(|v| v["key"] == "private-neighbor-stable")
            .count();
        client
            .rpc(
                "server.updateSettings",
                json!({"patch":{"providerInstances":replacement}}),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(server.dir.join("settings.json")).unwrap()
            )
            .unwrap(),
            before
        );
        dispatch(&client, "rotated-private");
        assert_eq!(
            launches(&server.dir).last().unwrap()["key"],
            "private-literal-two"
        );
        assert_eq!(calls(&server.dir), 0);
        assert_eq!(
            launches(&server.dir)
                .iter()
                .filter(|v| v["key"] == "private-neighbor-stable")
                .count(),
            neighbor_launches,
            "{driver}: an unrelated instance must not restart for another private slot"
        );
        assert_eq!(server.state.signing_key, signing);
        assert_eq!(
            std::fs::read(server.dir.join("secrets/server-signing-key.bin")).unwrap(),
            signing
        );
        assert_eq!(server.state.environment_id, environment_id);
    }
}

#[test]
fn private_environment_requires_shared_lock_and_snapshot_waits_for_writer() {
    test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "private-provider-lock-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let (_resolver, mut settings) = setup(&root, "codex", None);
    let variable = &mut settings["providerInstances"]["vaulted"]["environment"][0];
    variable["value"] = json!("");
    variable["valueRedacted"] = json!(true);
    variable.as_object_mut().unwrap().remove("secret");
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    let providers = ConfiguredProviders::new(Arc::new(ScriptedProvider), root.clone());
    let config = providers.config();
    let row = config
        .iter()
        .find(|v| v["instanceId"] == "vaulted")
        .unwrap();
    assert_eq!(row["availability"], "unavailable");
    assert!(row["unavailableReason"]
        .as_str()
        .unwrap()
        .contains("shared settings lock"));
    assert!(launches(&root).is_empty());
    let lock = Arc::new(std::sync::Mutex::new(()));
    let providers = Arc::new(
        ConfiguredProviders::new(Arc::new(ScriptedProvider), root.clone())
            .with_settings_lock(lock.clone()),
    );
    let guard = lock.lock().unwrap();
    let (captured, receive) = std::sync::mpsc::channel();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        captured.send(providers.config()).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(receive.recv_timeout(Duration::from_millis(100)).is_err());
    drop(guard);
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    assert!(launches(&root).is_empty());
    assert!(!root.join("secrets").exists());
}

#[tokio::test]
async fn unreadable_private_slot_blocks_native_launch_and_missing_slot_remains_native_empty_value()
{
    use base64::Engine;
    for driver in ["codex", "claudeAgent"] {
        let root = std::env::temp_dir().join(format!(
            "private-provider-read-error-{}",
            o3_server::store::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let (resolver, mut settings) = setup(&root, driver, None);
        let variable = &mut settings["providerInstances"]["vaulted"]["environment"][0];
        variable["value"] = json!("");
        variable["valueRedacted"] = json!(true);
        variable.as_object_mut().unwrap().remove("secret");
        std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
        std::fs::create_dir(root.join("secrets")).unwrap();
        let encoded = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        let slot = root.join("secrets").join(format!(
            "provider-env-{}-{}.bin",
            encoded("vaulted"),
            encoded("KEY")
        ));
        std::fs::create_dir(&slot).unwrap();
        let providers = ConfiguredProviders::new(Arc::new(ScriptedProvider), root.clone())
            .with_secret_resolver(resolver)
            .with_settings_lock(Arc::new(std::sync::Mutex::new(())));
        providers.discover(root.clone()).await.unwrap();
        let config = providers.config();
        let row = config
            .iter()
            .find(|v| v["instanceId"] == "vaulted")
            .unwrap();
        assert_eq!(row["availability"], "unavailable");
        assert!(row["unavailableReason"]
            .as_str()
            .unwrap()
            .contains("private environment"));
        assert!(launches(&root).is_empty());
        assert_eq!(calls(&root), 0);
        std::fs::rename(&slot, root.join("retained-unreadable-slot")).unwrap();
        providers.discover(root.clone()).await.unwrap();
        assert!(launches(&root).iter().any(|v| v["key"] == ""));
        assert_eq!(calls(&root), 0);
    }
}
#[test]
fn provider_inspection_does_not_invert_writer_pointer_and_settings_locks() {
    test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "private-provider-lock-order-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = Arc::new(o3_server::store::State::open(&root).unwrap());
    *state.provider.lock().unwrap() = Some(Arc::new(
        ConfiguredProviders::new(Arc::new(ScriptedProvider), root)
            .with_settings_lock(state.settings_lock.clone()),
    ));
    let guard = state.settings_lock.lock().unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let (sent, received) = std::sync::mpsc::channel();
    let reader = state.clone();
    let thread = std::thread::spawn(move || {
        started.send(()).unwrap();
        sent.send(o3_server::settings::config(&reader)).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(state.provider.try_lock().is_ok());
    let (updated, result) = std::sync::mpsc::channel();
    let writer = state.clone();
    let write = std::thread::spawn(move || {
        updated
            .send(o3_server::settings::update(
                &writer,
                &json!({"patch":{"defaultAutoPull":true}}),
            ))
            .unwrap();
    });
    drop(guard);
    received
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
    write.join().unwrap();
}
#[test]
fn supported_native_config_secret_annotations_remain_empty() {
    let fields: Value =
        serde_json::from_str(include_str!("fixtures/settings-secrets-4314.json")).unwrap();
    for driver in ["codex", "claudeAgent"] {
        assert_eq!(
            fields["descriptors"]["providers"][driver]["secretFields"],
            json!([])
        );
        assert_eq!(
            fields["descriptors"]["providers"][driver]["referenceFields"],
            json!([])
        );
    }
}
