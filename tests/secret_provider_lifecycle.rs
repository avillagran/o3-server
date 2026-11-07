//! Native secret-provider edits, permission boundaries and private-state preservation.
mod support;
use serde_json::{json, Value};
use std::collections::BTreeMap;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/secret-provider-lifecycle-4314.json")).unwrap()
}
fn server(current: &Value) -> support::Server {
    support::start(|state| {
        std::fs::write(state.dir.join("settings.json"), current.to_string()).unwrap()
    })
}
fn materialized(server: &support::Server) -> Value {
    let stored: Value =
        serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap();
    o3_server::settings_secrets::materialize_profile(&server.dir, &stored).unwrap()
}
fn private_bytes(server: &support::Server) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(server.dir.join("secrets"))
        .into_iter()
        .flatten()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
#[test]
fn native_entry_replacement_null_removal_and_legacy_echo_reach_real_rpc() {
    for case in fixture()["cases"].as_array().unwrap() {
        let server = server(&case["current"]);
        let client = server.connect();
        client
            .rpc("server.updateSettings", json!({"patch":{}}))
            .unwrap();
        let public = client
            .rpc("server.updateSettings", json!({"patch":case["patch"]}))
            .unwrap();
        let actual = materialized(&server);
        let expected = &case["result"];
        for field in ["secretProviders", "sourceControlHosts"] {
            assert_eq!(actual[field], expected[field], "{}: {field}", case["label"]);
        }
        assert_eq!(
            actual["usageLimitSources"]["source"]["managementKey"],
            expected["usageLimitSources"]["source"]["managementKey"],
            "{}: usage",
            case["label"]
        );
        assert_eq!(
            actual["providerInstances"]["custom"]["environment"],
            expected["providerInstances"]["custom"]["environment"],
            "{}: provider env",
            case["label"]
        );
        for field in ["easDeviceHosts", "simlockDeviceHosts"] {
            assert_eq!(actual[field], expected[field], "{}: {field}", case["label"]);
        }
        for id in ["annotated", "future"] {
            assert_eq!(
                actual["providerInstances"][id]["config"],
                expected["providerInstances"][id]["config"],
                "{}: {id} opaque config",
                case["label"]
            );
        }
        assert_eq!(
            public["secretProviders"], case["public"]["secretProviders"],
            "{}: public envelopes",
            case["label"]
        );
        assert!(!public.to_string().contains("must-stay-private"));
        assert!(!public.to_string().contains("unchanged-private"));
    }
}
#[test]
fn loaded_kind_guards_reject_before_mutating_settings_or_private_slots() {
    let fixture = fixture();
    let server = server(&fixture["cases"][0]["current"]);
    let client = server.connect();
    client
        .rpc("server.updateSettings", json!({"patch":{}}))
        .unwrap();
    assert!(!private_bytes(&server).is_empty());
    for case in fixture["guards"].as_array().unwrap() {
        let before = std::fs::read(server.dir.join("settings.json")).unwrap();
        let slots = private_bytes(&server);
        let result = client.rpc("server.updateSettings", json!({"patch":case["patch"]}));
        if case["allowed"] == true {
            result.unwrap();
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("write-secret-provider"), "{error}");
            assert_eq!(
                std::fs::read(server.dir.join("settings.json")).unwrap(),
                before
            );
            assert_eq!(private_bytes(&server), slots);
        }
    }
    let before = std::fs::read(server.dir.join("settings.json")).unwrap();
    let slots = private_bytes(&server);
    assert!(client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"secretProviders":{"vault":null},"branchNamingMode":"invalid"}})
        )
        .is_err());
    assert_eq!(
        std::fs::read(server.dir.join("settings.json")).unwrap(),
        before
    );
    assert_eq!(private_bytes(&server), slots);
}
#[test]
fn authority_matches_native_patch_scope_decisions_and_actual_wire() {
    for case in fixture()["scopeCases"].as_array().unwrap() {
        for scopes in [
            vec![],
            vec!["settings:write"],
            vec!["providers:manage"],
            vec!["settings:write", "providers:manage"],
        ] {
            let expected = case["scopes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|scope| scopes.iter().any(|allowed| scope == *allowed));
            assert_eq!(
                o3_server::rpc::authorize(
                    &json!({"scopes":scopes}),
                    "server.updateSettings",
                    &json!({"patch":case["patch"]})
                )
                .is_ok(),
                expected,
                "{case}"
            );
        }
    }
    let fixture = fixture();
    let server = server(&fixture["cases"][0]["current"]);
    let owner = server.connect();
    let settings = server.limited(&owner, &["orchestration:read", "settings:write"]);
    let providers = server.limited(&owner, &["orchestration:read", "providers:manage"]);
    let both = server.limited(
        &owner,
        &["orchestration:read", "settings:write", "providers:manage"],
    );
    let update = json!({"patch":{"secretProviders":{"added":{"kind":"one_password","label":"Added","config":{"account":"added"}}}}});
    assert!(settings
        .rpc("server.updateSettings", update.clone())
        .is_err());
    providers.rpc("server.updateSettings", update).unwrap();
    let remove = json!({"patch":{"secretProviders":{"vault":null}}});
    assert!(providers
        .rpc("server.updateSettings", remove.clone())
        .is_err());
    assert!(settings
        .rpc("server.updateSettings", remove.clone())
        .is_err());
    both.rpc("server.updateSettings", remove).unwrap();
    assert_eq!(
        materialized(&server)["sourceControlHosts"]["github"]["tokens"]["fixture.test"],
        ""
    );
}
