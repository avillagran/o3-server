use super::*;
use crate::secret_resolver::SecretDriver;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/provider-secrets-4275.json")).unwrap()
}
#[test]
fn native_provider_mapping_oracles() {
    let fixture = fixture();
    for c in fixture["cases"].as_array().unwrap() {
        let label = c["label"].as_str().unwrap();
        let decoded = codec::decode_provider_instance(c["input"].clone());
        if c["invalidEntry"] == true {
            assert!(decoded.is_err(), "{label}: invalid envelope");
            continue;
        }
        let entry = decoded.unwrap();
        assert_eq!(entry, c["entry"], "{label}: envelope");
        if c.get("blocked").is_some() {
            continue;
        }
        let driver = entry["driver"].as_str().unwrap();
        if !["codex", "claudeAgent", "antigravity", "keyed"].contains(&driver) {
            assert!(matches!(
                Prepared::new(&c["input"]),
                Err(PreparationError::Unsupported(_))
            ));
            continue;
        }
        if c["invalidConfig"] == true {
            if matches!(driver, "codex" | "claudeAgent") {
                let Err(PreparationError::InvalidConfig(detail)) = Prepared::new(&c["input"])
                else {
                    panic!("{label}: invalid config accepted")
                };
                assert_eq!(detail, c["nativeDetail"]);
            }
            continue;
        }
        let typed = if matches!(driver, "codex" | "claudeAgent") {
            codec::decode_provider_config(
                driver,
                entry
                    .get("config")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .unwrap()
        } else {
            c["typedConfig"].clone()
        };
        assert_eq!(typed, c["typedConfig"], "{label}: typed config");
        let fields: Vec<Field> =
            serde_json::from_value(fixture["declaredFields"][driver].clone()).unwrap();
        let prepared = Prepared {
            raw: c["input"].clone(),
            decoded: entry,
            enabled: resolve_enabled(&c["entry"], &typed),
            typed_config: typed,
            fields,
        };
        assert_eq!(
            prepared.enabled,
            c["enabled"].as_bool().unwrap(),
            "{label}: enabled"
        );
        let refs = prepared.references();
        assert_eq!(
            json!(refs
                .iter()
                .map(SecretReference::to_value)
                .collect::<Vec<_>>()),
            c["references"],
            "{label}: references"
        );
        let values = refs
            .iter()
            .map(|r| {
                c["values"]
                    .get(format!("{}|{}", r.provider(), r.reference()))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        let result = prepared.with_values(&values);
        assert_eq!(
            json!(result.secrets),
            c["secrets"],
            "{label}: mapped secrets"
        );
        assert_eq!(
            json!(result.unreadable),
            c["unreadable"],
            "{label}: unreadable"
        );
        assert_eq!(
            result.config(&prepared.typed_config),
            c["resolvedConfig"],
            "{label}: resolved config"
        );
        assert_eq!(
            result.reason(),
            c.get("unavailable")
                .and_then(Value::as_str)
                .map(str::to_owned),
            "{label}: unavailable"
        );
    }
}
#[derive(Default)]
struct DriverState {
    calls: Mutex<Vec<Vec<String>>>,
    values: Mutex<Value>,
}
struct Fake(Arc<DriverState>);
impl SecretDriver for Fake {
    fn kind(&self) -> &str {
        "fixture"
    }
    fn decode_config(&self, value: &Value) -> Option<Value> {
        value["account"].as_str().map(|v| json!({"account":v}))
    }
    fn is_reference(&self, value: &str) -> bool {
        value.starts_with("op://")
    }
    fn read<'a>(
        &'a self,
        config: Value,
        references: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            self.0.calls.lock().unwrap().push(references.clone());
            references
                .iter()
                .map(|r| {
                    self.0
                        .values
                        .lock()
                        .unwrap()
                        .get(format!("{}|{r}", config["account"].as_str().unwrap()))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        })
    }
}
fn entry(reference: &str) -> Value {
    json!({"driver":"codex","environment":[{"name":"KEY","value":"","secret":{"provider":"vault","reference":reference},"sensitive":false}]})
}
fn settings(account: &str) -> Value {
    json!({"secretProviders":{"vault":{"kind":"fixture","config":{"account":account}}}})
}
fn registry() -> (Registry, Arc<DriverState>) {
    let state = Arc::new(DriverState::default());
    let resolver = Arc::new(SecretResolver::new(vec![Arc::new(Fake(state.clone()))]));
    (Registry::new(resolver), state)
}
fn policy() -> crate::managed_policy::Policy {
    crate::managed_policy::Policy::unmanaged(std::path::PathBuf::new())
}
fn secret(status: &Status) -> Option<&str> {
    match status {
        Status::Ready { secrets, .. } => secrets.environment[0]["value"].as_str(),
        _ => None,
    }
}
#[tokio::test]
async fn registry_batches_and_parks_unreadable_until_explicit_reload() {
    let (registry, state) = registry();
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"first"});
    let entries = json!({"first":entry("op://a/b/c"),"second":entry("op://a/b/d")});
    let policy = policy();
    let settings = settings("a");
    let initial = registry.reconcile(&entries, &settings, &policy);
    assert_eq!(initial["first"], Status::Reading);
    registry.wait().await;
    let loaded = registry.reconcile(&entries, &settings, &policy);
    assert_eq!(secret(&loaded["first"]), Some("first"));
    assert!(matches!(loaded["second"], Status::Unavailable(_)));
    assert_eq!(
        state.calls.lock().unwrap().as_slice(),
        &[vec!["op://a/b/c".to_owned(), "op://a/b/d".to_owned()]]
    );
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"first","a|op://a/b/d":"second"});
    registry.reconcile(&entries, &settings, &policy);
    registry.wait().await;
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    registry
        .reload(&entries, &settings, &policy, Some("second"))
        .await;
    let reloaded = registry.reconcile(&entries, &settings, &policy);
    assert_eq!(secret(&reloaded["second"]), Some("second"));
    assert_eq!(state.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn failed_live_reload_keeps_working_credentials_and_successful_change_replaces() {
    let (registry, state) = registry();
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"working"});
    let entries = json!({"instance":entry("op://a/b/c")});
    let policy = policy();
    let settings = settings("a");
    registry.reconcile(&entries, &settings, &policy);
    registry.wait().await;
    let initial = registry.reconcile(&entries, &settings, &policy);
    *state.values.lock().unwrap() = json!({});
    registry.reload(&entries, &settings, &policy, None).await;
    assert_eq!(registry.reconcile(&entries, &settings, &policy), initial);
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"working"});
    registry.reload(&entries, &settings, &policy, None).await;
    assert_eq!(registry.reconcile(&entries, &settings, &policy), initial);
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"changed"});
    registry.reload(&entries, &settings, &policy, None).await;
    let next = registry.reconcile(&entries, &settings, &policy);
    assert_eq!(secret(&next["instance"]), Some("changed"));
    assert_ne!(next, initial);
    assert_eq!(state.calls.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn disabled_unknown_and_invalid_configs_do_not_read_vault() {
    let (registry, state) = registry();
    let mut disabled = entry("op://a/b/c");
    disabled["enabled"] = json!(false);
    let mut unknown = entry("op://a/b/c");
    unknown["driver"] = json!("notRegistered");
    let mut invalid = entry("op://a/b/c");
    invalid["config"] = json!({"enabled":"bad"});
    let entries = json!({"disabled":disabled,"unknown":unknown,"invalid":invalid});
    let states = registry.reconcile(&entries, &settings("a"), &policy());
    registry.wait().await;
    assert!(matches!(states["disabled"], Status::Ready { .. }));
    assert!(matches!(states["unknown"], Status::Unavailable(_)));
    assert!(matches!(states["invalid"], Status::Unavailable(_)));
    assert!(state.calls.lock().unwrap().is_empty());
    registry
        .reload(&entries, &settings("a"), &policy(), None)
        .await;
    assert!(state.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn provider_config_change_waits_for_explicit_secret_reload() {
    let (registry, state) = registry();
    *state.values.lock().unwrap() = json!({"a|op://a/b/c":"account a","b|op://a/b/c":"account b"});
    let entries = json!({"instance":entry("op://a/b/c")});
    let policy = policy();
    registry.reconcile(&entries, &settings("a"), &policy);
    registry.wait().await;
    let retained = registry.reconcile(&entries, &settings("b"), &policy);
    assert_eq!(secret(&retained["instance"]), Some("account a"));
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    registry
        .reload(&entries, &settings("b"), &policy, None)
        .await;
    let next = registry.reconcile(&entries, &settings("b"), &policy);
    assert_eq!(secret(&next["instance"]), Some("account b"));
    assert_eq!(state.calls.lock().unwrap().len(), 2);
}
