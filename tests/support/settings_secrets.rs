use super::*;
fn fixtures() -> Value {
    serde_json::from_str(include_str!("../fixtures/settings-secrets-4314.json")).unwrap()
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
    let voice = json!({"provider":"vault","reference":"fixture://voice"});
    let references = vec![
        SecretReference::decode(&setting).unwrap(),
        SecretReference::decode(&instance).unwrap(),
        SecretReference::decode(&voice).unwrap(),
    ];
    resolver.read(&providers, &references).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let settings = json!({"sourceControlHosts":{"github":{"tokens":{"fixture.test":setting}}},"providerInstances":{"fixture":{"env":{"TOKEN":instance}}},"cloudSpeechCredentials":{"openai":voice}});
    invalidate(&resolver, Some(&settings)).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "global settings reload must leave instance-scoped cached references intact"
    );
    invalidate(&resolver, Some(&json!({}))).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "empty settings invalidate no references"
    );
    invalidate(&resolver, None).await;
    resolver.read(&providers, &references).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        8,
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

#[test]
fn native_profile_materialization_matches_complete_method_observations() {
    crate::test_sandbox::assert_isolated();
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/secret-store-4314.json")).unwrap();
    assert_eq!(fixture["profileCases"].as_array().unwrap().len(), 517);
    let root = std::env::temp_dir().join(format!(
        "profile-native-materialization-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    for (index, case) in fixture["profileCases"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let profile = root.join(format!("case-{index}"));
        std::fs::create_dir(&profile).unwrap();
        if !case["files"].as_object().unwrap().is_empty() {
            std::fs::create_dir(profile.join("secrets")).unwrap();
            for (path, file) in case["files"].as_object().unwrap() {
                let slot = path.strip_prefix("/private-fixture/secrets/").unwrap();
                assert!(!slot.contains('/'));
                let bytes: Vec<u8> = serde_json::from_value(file["bytes"].clone()).unwrap();
                std::fs::write(profile.join("secrets").join(slot), bytes).unwrap();
            }
        }
        let actual = materialize_profile(&profile, &case["settings"]).unwrap();
        assert_eq!(actual, case["result"], "{}", case["label"]);
        for (path, file) in case["files"].as_object().unwrap() {
            let slot = path.strip_prefix("/private-fixture/secrets/").unwrap();
            let bytes: Vec<u8> = serde_json::from_value(file["bytes"].clone()).unwrap();
            assert_eq!(
                std::fs::read(profile.join("secrets").join(slot)).unwrap(),
                bytes,
                "materialization must not rewrite private bytes"
            );
        }
        if case["files"].as_object().unwrap().is_empty() {
            assert!(!profile.join("secrets").exists());
        }
    }
}
#[test]
fn profile_reader_refuses_ancestor_symlinks_and_keeps_missing_distinct_from_unreadable() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "profile-symlink-refusal-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let settings = json!({"providerInstances":{},"sourceControlHosts":{},"usageLimitSources":{"synthetic":{"kind":"cliproxy","managementKey":"••••••"}},"easDeviceHosts":[],"simlockDeviceHosts":[],"secretProviders":{}});
    let owned = root.join("owned");
    std::fs::create_dir(&owned).unwrap();
    let slots = owned.join("secrets");
    std::fs::create_dir(&slots).unwrap();
    let name = "usage-limit-source-c3ludGhldGlj.bin";
    std::fs::write(slots.join(name), b"private-owned-value").unwrap();
    let alias = root.join("profile-alias");
    std::os::unix::fs::symlink(&owned, &alias).unwrap();
    assert!(materialize_profile(&alias, &settings).is_err());
    let directory = root.join("directory-alias");
    std::fs::create_dir(&directory).unwrap();
    std::os::unix::fs::symlink(&slots, directory.join("secrets")).unwrap();
    assert!(materialize_profile(&directory, &settings).is_err());
    let missing = root.join("missing");
    std::fs::create_dir(&missing).unwrap();
    assert_eq!(
        materialize_profile(&missing, &settings).unwrap()["usageLimitSources"]["synthetic"]
            ["managementKey"],
        ""
    );
    assert!(!missing.join("secrets").exists());
    let unreadable = root.join("unreadable");
    std::fs::create_dir(&unreadable).unwrap();
    std::fs::create_dir(unreadable.join("secrets")).unwrap();
    std::fs::create_dir(unreadable.join("secrets").join(name)).unwrap();
    assert!(materialize_profile(&unreadable, &settings).is_err());
    assert_eq!(
        std::fs::read(slots.join(name)).unwrap(),
        b"private-owned-value"
    );
}
