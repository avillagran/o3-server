//! Differential acceptance for admitted terminal-provider inputs.
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[test]
fn whole_native_terminal_environment_matches_for_admitted_inputs() {
    crate::test_sandbox::assert_isolated();
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/terminal-environment-4288.json"
    ))
    .unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 315);
    let mut compared = 0;
    let mut differences = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        if case["rawIdIsDecoded"] != true || case["input"].get("settingsError").is_some() {
            continue;
        }
        let input = &case["input"];
        let env: BTreeMap<String, String> = input["env"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
            .collect();
        let actual = match crate::terminal_environment::resolve_with(
            &input["settings"],
            input["id"].as_str().unwrap(),
            env,
            Path::new(input["home"].as_str().unwrap()),
            Path::new(input["cwd"].as_str().unwrap()),
        ) {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(error) => {
                json!({"ok":false,"error":{"tag":error["_tag"],"message":error["message"],"providerInstanceId":error["providerInstanceId"]}})
            }
        };
        compared += 1;
        if actual != case["expected"] {
            differences.push(
                json!({"case":index,"input":input,"actual":actual,"expected":case["expected"]}),
            );
        }
    }
    assert_eq!(compared, 307);
    assert!(
        differences.is_empty(),
        "{} whole-native divergences:\n{}",
        differences.len(),
        serde_json::to_string_pretty(&differences).unwrap()
    );
}
