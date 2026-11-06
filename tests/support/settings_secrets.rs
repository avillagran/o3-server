use super::*;
fn fixtures() -> Value {
    serde_json::from_str(include_str!("../fixtures/settings-secrets-4288.json")).unwrap()
}
#[test]
fn native_mapping_slots_values_and_provider_removal() {
    for case in fixtures()["cases"].as_array().unwrap() {
        let label = case["label"].as_str().unwrap();
        let mut calls = Vec::new();
        let mapped = map_secrets(&case["input"], |secret| {
            calls.push(serde_json::to_value(&secret).unwrap());
            SecretValue {
                value: format!("mapped:{}", secret.value),
                redacted: secret.redacted,
            }
        });
        assert_eq!(json!(calls), case["calls"], "{label}: call order/metadata");
        assert_eq!(mapped, case["mapped"], "{label}: mapped");
        let mut calls = Vec::new();
        let mapped = map_values(&case["input"], |value, name| {
            calls.push(json!({"value":value,"name":name}));
            if let Some(value) = value.as_str() {
                json!(format!("value:{value}"))
            } else {
                value
            }
        });
        assert_eq!(json!(calls), case["valueCalls"], "{label}: values metadata");
        assert_eq!(mapped, case["valueMapped"], "{label}: values");
        assert_eq!(
            without_removed_providers(&case["removedCurrent"], &case["input"]),
            case["removed"],
            "{label}: removed providers"
        );
    }
}
#[test]
fn native_selective_invalidation_reference_collection() {
    for case in fixtures()["cases"].as_array().unwrap() {
        let collected = collect_settings_references(&case["input"]);
        assert_eq!(
            json!(collected
                .iter()
                .map(SecretReference::to_value)
                .collect::<Vec<_>>()),
            case["invalidations"][0],
            "{}",
            case["label"]
        )
    }
}

#[test]
fn native_host_schema_decoding_and_fallback() {
    for case in fixtures()["hostDecode"].as_array().unwrap() {
        assert_eq!(
            codec::read_source_host_settings(case["kind"].as_str().unwrap(), case["input"].clone()),
            Some(case["decoded"].clone()),
            "{case}"
        )
    }
}

#[tokio::test]
async fn selective_refresh_retries_settings_refs_without_evicting_instance_refs() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Driver(Arc<AtomicUsize>);
    impl crate::secret_resolver::SecretDriver for Driver {
        fn kind(&self) -> &str {
            "fixture"
        }
        fn decode_config(&self, value: &Value) -> Option<Value> {
            Some(value.clone())
        }
        fn is_reference(&self, value: &str) -> bool {
            value.starts_with("fixture://")
        }
        fn read<'a>(
            &'a self,
            _: Value,
            references: Vec<String>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Option<String>>> + Send + 'a>>
        {
            Box::pin(async move {
                self.0.fetch_add(references.len(), Ordering::SeqCst);
                references.into_iter().map(Some).collect()
            })
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = SecretResolver::new(vec![Arc::new(Driver(calls.clone()))]);
    let providers = json!({"vault":{"kind":"fixture","config":{}}});
    let setting = json!({"provider":"vault","reference":"fixture://settings"});
    let instance = json!({"provider":"vault","reference":"fixture://instance"});
    let references = vec![
        SecretReference::decode(&setting).unwrap(),
        SecretReference::decode(&instance).unwrap(),
    ];
    resolver.read(&providers, &references).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let settings = json!({"sourceControlHosts":{"github":{"tokens":{"fixture.test":setting}}},"providerInstances":{"fixture":{"env":{"TOKEN":instance}}}});
    invalidate(&resolver, Some(&settings)).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "global settings reload must leave instance-scoped cached references intact"
    );
    invalidate(&resolver, Some(&json!({}))).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "empty settings invalidate no references"
    );
    invalidate(&resolver, None).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "unreadable settings invalidate all"
    );
}

#[test]
fn private_slot_materialization_is_read_only_and_does_not_replace_references() {
    crate::test_sandbox::assert_isolated();
    let directory = std::env::temp_dir().join(format!(
        "o3-settings-private-{}",
        crate::store::random_id().unwrap()
    ));
    let state = crate::store::State::open(&directory).unwrap();
    let settings = json!({"sourceControlHosts":{"github":{"tokens":{"fixture.test":REDACTED,"unreadable.test":{"provider":"vault","reference":"op://fixture/item/token"}}}}});
    let secret_name = map_slot_name(&settings);
    let file = state.dir.join("secrets").join(format!("{secret_name}.bin"));
    std::fs::write(&file, b" private-slot-token \n").unwrap();
    let materialized = materialize_profile(&state.dir, &settings).unwrap();
    assert_eq!(
        materialized["sourceControlHosts"]["github"]["tokens"]["fixture.test"],
        " private-slot-token \n"
    );
    assert_eq!(
        materialized["sourceControlHosts"]["github"]["tokens"]["unreadable.test"],
        settings["sourceControlHosts"]["github"]["tokens"]["unreadable.test"]
    );
    assert_eq!(
        settings["sourceControlHosts"]["github"]["tokens"]["fixture.test"],
        REDACTED
    );
    assert_eq!(std::fs::read(file).unwrap(), b" private-slot-token \n");
    let missing = json!({"sourceControlHosts":{"github":{"tokens":{"missing.test":REDACTED}}}});
    assert_eq!(
        materialize_profile(&state.dir, &missing).unwrap()["sourceControlHosts"]["github"]
            ["tokens"]["missing.test"],
        ""
    );
}
fn map_slot_name(settings: &Value) -> String {
    let mut name = None;
    map_secrets(settings, |secret| {
        if secret.redacted {
            name = Some(secret.secret_name);
        }
        SecretValue {
            value: secret.value,
            redacted: secret.redacted,
        }
    });
    name.unwrap()
}

#[tokio::test]
async fn native_schema_reference_resolution_and_literal_refusal() {
    struct Driver(Option<String>);
    impl crate::secret_resolver::SecretDriver for Driver {
        fn kind(&self) -> &str {
            "fixture"
        }
        fn decode_config(&self, value: &Value) -> Option<Value> {
            Some(value.clone())
        }
        fn is_reference(&self, _: &str) -> bool {
            true
        }
        fn read<'a>(
            &'a self,
            _: Value,
            references: Vec<String>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Option<String>>> + Send + 'a>>
        {
            Box::pin(async move { references.into_iter().map(|_| self.0.clone()).collect() })
        }
    }
    for case in fixtures()["resolveCases"].as_array().unwrap() {
        let resolver = SecretResolver::new(vec![std::sync::Arc::new(Driver(
            case["answer"].as_str().map(str::to_owned),
        ))]);
        let providers = json!({"vault":{"kind":"fixture","config":{}}});
        assert_eq!(
            resolve_fields(
                &resolver,
                &providers,
                case["category"].as_str().unwrap(),
                case["kind"].as_str(),
                &case["input"]
            )
            .await,
            case["resolved"],
            "{case}"
        );
    }
    for case in fixtures()["literalCases"].as_array().unwrap() {
        let actual = require_literal(&case["value"], &case["providers"]);
        if let Some(error) = case["error"].as_str() {
            assert_eq!(actual, Err(error.into()), "{case}")
        } else {
            assert_eq!(
                actual,
                Ok(case["literal"].as_str().unwrap().into()),
                "{case}"
            )
        }
    }
}
