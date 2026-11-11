//! Compatible-client and real native transport admission under provider policy.
mod support;
use o3_server::provider::{ConfiguredProviders, Provider, ScriptedProvider};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
const WAIT: Duration = Duration::from_secs(15);
fn project(s: &support::Server, id: &str) -> PathBuf {
    let root = s.dir.join(id);
    std::fs::create_dir_all(&root).unwrap();
    s.connect().rpc("projects.mutate",json!({"type":"project.create","commandId":id,"projectId":id,"title":id,"workspaceRoot":root})).unwrap();
    root
}
fn projection(c: &t3_protocol::Connection, tid: &str) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":tid}))
        .unwrap()
}
fn create(s: &support::Server, tid: &str, pid: &str, id: &str) {
    s.connect().rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("create-{tid}"),"threadId":tid,"projectId":pid,"title":tid,"modelSelection":{"instanceId":id,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
}
fn send(
    s: &support::Server,
    tid: &str,
    cid: &str,
    instance: Option<&str>,
) -> Result<Value, String> {
    let mut p = json!({"type":"message.dispatch","commandId":cid,"messageId":cid,"threadId":tid,"text":"test","attachments":[],"dispatchMode":{"type":"defer_start"}});
    if let Some(id) = instance {
        p["modelSelection"] = json!({"instanceId":id,"model":"test"})
    }
    s.connect()
        .rpc("orchestration.dispatchCommand", p)
        .map_err(|e| e.to_string())
}
fn registry(s: &Arc<o3_server::store::State>) {
    *s.provider.lock().unwrap() = Some(Arc::new(ConfiguredProviders::new(
        Arc::new(ScriptedProvider),
        s.dir.clone(),
    )));
}
fn row(c: &t3_protocol::Connection, id: &str) -> Value {
    c.rpc("server.getConfig", json!({})).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["instanceId"] == id)
        .cloned()
        .unwrap()
}
#[test]
fn project_override_probes_and_executes_both_native_drivers_without_global_enablement() {
    for driver in ["claudeAgent", "codex"] {
        let s = support::start(registry);
        project(&s, "allowed");
        project(&s, "other");
        let c = s.connect();
        let id = "work";
        let home = s.dir.join(id);
        std::fs::create_dir_all(&home).unwrap();
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/instance.py");
        c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{id:{"driver":driver,"enabled":true,"config":{"enabled":false,"binaryPath":peer,"homePath":home,"launchArgs":"--instance-marker \"quoted value\""},"environment":[{"name":"INSTANCE_MARKER","value":id}]}}}})).unwrap();
        assert_eq!(row(&c, id)["enabled"], false);
        assert!(!home.join("starts").exists());
        create(&s, "blocked", "other", id);
        let err = send(&s, "blocked", "blocked", None).unwrap_err();
        assert!(err.contains("disabled"), "{err}");
        c.rpc("server.updateSettings",json!({"patch":{"projectSettingsOverrides":{"allowed":{"providerInstances":{id:{"enabled":true}}}}}})).unwrap();
        assert!(
            support::wait_for(WAIT, || {
                let r = row(&c, id);
                r["enabled"] == true
                    && r["status"] != "disabled"
                    && if driver == "codex" {
                        r["models"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|m| m["slug"] == "test"))
                    } else {
                        r["auth"]["status"] == "authenticated"
                    }
            }),
            "{}",
            row(&c, id)
        );
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        assert!(t3_protocol::models::catalog(&config)["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["instanceId"] == id && m["available"] == true));
        let settings = c.rpc("server.getSettings", json!({})).unwrap();
        assert_eq!(settings["providerInstances"][id]["enabled"], false);
        assert!(settings["providerInstances"][id]["config"]
            .get("enabled")
            .is_none());
        c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"native","projectId":"allowed","title":"Native","modelSelection":{"instanceId":id,"model":"test","options":if driver=="codex"{json!([{"id":"serviceTier","value":"fast"}])}else{json!([])}},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"native proof","attachments":[]}})).unwrap();
        assert!(support::wait_for(WAIT, || projection(&c, "native")["runs"]
            [0]["completedAt"]
            .is_string()));
        assert_eq!(
            projection(&c, "native")["runs"][0]["status"],
            "completed",
            "{}",
            projection(&c, "native")
        );
        assert_eq!(
            std::fs::read_to_string(home.join("starts")).unwrap(),
            "start\nstart\n"
        );
        assert!(send(&s, "blocked", "still-blocked", None)
            .unwrap_err()
            .contains("disabled"));
        assert!(projection(&c, "blocked")["messages"]
            .as_array()
            .unwrap()
            .is_empty());
        c.rpc(
            "server.updateSettings",
            json!({"patch":{"projectSettingsOverrides":{"allowed":null}}}),
        )
        .unwrap();
        assert_eq!(row(&c, id)["enabled"], false);
        assert!(send(&s, "native", "now-blocked", None)
            .unwrap_err()
            .contains("disabled"));
        c.rpc("server.updateSettings",json!({"patch":{"textGenerationModelSelection":{"instanceId":id,"model":"test"},"projectSettingsOverrides":{"other":{"providerInstances":{id:{"enabled":true}}}}}})).unwrap();
        assert_eq!(row(&c, id)["enabled"], true);
        let error=c.rpc("orchestration.dispatchCommand",json!({"type":"thread.metadata.update","commandId":"denied-title","threadId":"native","regenerateTitle":true})).unwrap_err();
        assert!(error.contains("disabled"), "{error}");
        c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{},"projectSettingsOverrides":{"allowed":{"providerInstances":{id:{"enabled":true}}}}}})).unwrap();
        assert!(!c.rpc("server.getConfig", json!({})).unwrap()["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["instanceId"] == id));
    }
}
#[test]
fn disabled_message_admission_is_atomic_replays_survive_disable_and_selection_uses_target_instance()
{
    let s = support::start(|_| {});
    let c = s.connect();
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"work":{"driver":"codex","enabled":true}},"projectSettingsOverrides":{"p":{"providerInstances":{"codex":{"enabled":false}}}}}})).unwrap();
    create(&s, "t", "p", "work");
    send(&s, "t", "accepted", None).unwrap();
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"providerInstances":{"work":{"driver":"codex","enabled":false}}}}),
    )
    .unwrap();
    send(&s, "t", "accepted", None).unwrap();
    assert!(send(&s, "t", "denied", None)
        .unwrap_err()
        .contains("disabled"));
    assert_eq!(projection(&c, "t")["messages"].as_array().unwrap().len(), 1);
    assert!(send(&s, "t", "switch-denied", Some("codex"))
        .unwrap_err()
        .contains("disabled"));
    create(&s, "future", "none", "unconfigured");
    assert!(send(&s, "future", "unknown", None)
        .unwrap_err()
        .contains("disabled"));
    c.rpc("server.updateSettings",json!({"patch":{"projectSettingsOverrides":{"p":{"providerInstances":{"claudeAgent":{"enabled":true}}}}}})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.model-selection.set","commandId":"model","threadId":"t","modelSelection":{"instanceId":"claudeAgent","model":"test"}})).unwrap_err();
    // A second idle thread tests the command's own model selection rather than its previous one.
    create(&s, "idle", "p", "work");
    send(&s, "idle", "switch-allowed", Some("claudeAgent")).unwrap();
    assert_eq!(
        projection(&c, "idle")["messages"][0]["id"],
        "switch-allowed"
    );
}
#[test]
fn catalog_reports_unsupported_drivers_without_inventing_installation_and_preserves_default_flags()
{
    let s = support::start(registry);
    let c = s.connect();
    let next=c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"grok":{"driver":"grok","config":{}},"custom":{"driver":"unknownVendor","config":{"binaryPath":"/bin/true","enabled":"nope"}}}}})).unwrap();
    assert!(next["providerInstances"]["grok"].get("enabled").is_none());
    assert!(next["providerInstances"]["custom"].get("enabled").is_none());
    assert_eq!(
        next["providerInstances"]["custom"]["config"]["enabled"],
        "nope"
    );
    for id in ["grok", "custom"] {
        let provider = row(&c, id);
        assert_eq!(provider["availability"], "unavailable");
        assert_eq!(provider["enabled"], false);
        assert_eq!(provider["installed"], false);
        assert_eq!(provider["status"], "error");
        assert!(provider["unavailableReason"]
            .as_str()
            .unwrap()
            .contains("not registered"));
        assert!(provider.get("setup").is_none());
    }
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(s.dir.join("settings.json")).unwrap()).unwrap();
    assert!(persisted["providerInstances"]["grok"]
        .get("enabled")
        .is_none());
}

#[tokio::test]
async fn removed_default_override_is_discovered_once_and_configured_defaults_stay_excluded() {
    use o3_server::provider::{ProviderControl, ProviderEvent, RunInput};
    use std::{future::Future, pin::Pin, sync::Mutex};
    #[derive(Default)]
    struct Defaults {
        discover: Mutex<Vec<Vec<String>>>,
        refresh: Mutex<Vec<Vec<String>>>,
    }
    impl Provider for Defaults {
        fn discover_defaults<'a>(
            &'a self,
            _cwd: PathBuf,
            excluded: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            self.discover.lock().unwrap().push(excluded);
            Box::pin(async { Ok(()) })
        }
        fn refresh_defaults<'a>(
            &'a self,
            excluded: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            self.refresh.lock().unwrap().push(excluded);
            Box::pin(async { Ok(()) })
        }
        fn execute<'a>(
            &'a self,
            _input: RunInput,
            _output: tokio::sync::mpsc::Sender<ProviderEvent>,
            _control: tokio::sync::mpsc::Receiver<ProviderControl>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async { Err("unused".into()) })
        }
    }
    let root = std::env::temp_dir().join(format!(
        "o3-policy-default-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    std::fs::write(&path,json!({"providerInstances":{"codex":{"driver":"codex","enabled":false,"config":{"binaryPath":"/unavailable"}}}}).to_string()).unwrap();
    let defaults = Arc::new(Defaults::default());
    let registry = ConfiguredProviders::new(defaults.clone(), root.clone());
    registry.discover(root.clone()).await.unwrap();
    registry.refresh().await.unwrap();
    assert_eq!(
        *defaults.discover.lock().unwrap(),
        vec![vec!["codex".to_owned()]]
    );
    assert_eq!(
        *defaults.refresh.lock().unwrap(),
        vec![vec!["codex".to_owned()]]
    );
    std::fs::write(&path, b"{invalid").unwrap();
    assert!(registry.config().iter().any(|p| p["instanceId"] == "codex"));
    registry.discover(root.clone()).await.unwrap();
    assert_eq!(defaults.discover.lock().unwrap().len(), 1);
    std::fs::write(&path, "{}").unwrap();
    registry.discover(root.clone()).await.unwrap();
    registry.discover(root.clone()).await.unwrap();
    assert_eq!(
        *defaults.discover.lock().unwrap(),
        vec![vec!["codex".to_owned()], vec!["claudeAgent".to_owned()]]
    );
    std::fs::remove_dir_all(root).unwrap();
}
