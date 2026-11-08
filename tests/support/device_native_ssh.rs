use o3_server::device_native_ssh::*;
use serde_json::{json, Value};
use std::time::Duration;

#[test]
fn complete_native_ssh_command_owner_and_codec_reference() {
    crate::test_sandbox::assert_isolated();
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/device-native-ssh-4288.json")).unwrap();
    assert_eq!(
        REMOTE_ENVIRONMENT,
        fixture["remoteEnvironment"].as_str().unwrap()
    );
    for (index, case) in fixture["configCases"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let actual = Config::decode(&case["input"])
            .map(|config| json!({"value":config}))
            .unwrap_or_else(|_| json!({"invalid":true}));
        assert_eq!(
            actual, case["expected"],
            "config {index}: {}",
            case["input"]
        );
    }
    for case in fixture["quoteCases"].as_array().unwrap() {
        assert_eq!(
            quote_remote_arg(case["input"].as_str().unwrap()),
            case["expected"].as_str().unwrap()
        );
    }
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let input = &case["input"];
        let config = Config::decode(&input["config"]).unwrap();
        let expected = &case["expected"];
        let owner = owner(
            input["environmentId"].as_str(),
            input["stateDir"].as_str().unwrap(),
            &config.id,
        );
        for actual in expected["owners"].as_array().unwrap() {
            assert_eq!(&json!(owner), actual, "owner {index}");
        }
        for command in expected["commands"].as_array().unwrap() {
            let args = command["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            let actual = if command["forwarding"] == true {
                let ports = args
                    .windows(2)
                    .filter(|pair| pair[0] == "-L")
                    .map(|pair| pair[1].split(':').collect::<Vec<_>>())
                    .map(|parts| {
                        (
                            parts[1].parse::<u16>().unwrap(),
                            parts[3].parse::<u16>().unwrap(),
                        )
                    })
                    .collect::<Vec<_>>();
                assert_eq!(command["stdin"], "pipe");
                forward_argv(&config, ports[0].0, ports[0].1, ports.get(1).copied())
            } else if command["mode"] != "" {
                bootstrap_argv(&config)
            } else {
                let run = &input["run"];
                run_argv(
                    &config,
                    &owner,
                    run["command"].as_str().unwrap(),
                    &serde_json::from_value::<Vec<String>>(run["args"].clone()).unwrap(),
                )
                .unwrap()
            };
            assert_eq!(actual, args, "command case {index}: {command}");
        }
        if let Some(failure) = input.get("runFailure") {
            let actual = failed_command(
                &config,
                failure["code"].as_i64().unwrap() as i32,
                failure["stdout"].as_str().unwrap(),
                failure["stderr"].as_str().unwrap(),
            );
            assert_eq!(
                json!({"code":actual.code,"stdout":actual.stdout,"stderr":actual.stderr}),
                expected["runResult"],
                "failure case {index}"
            );
        }
        assert_eq!(expected["activeAfterStop"], 0, "case {index}");
    }
}
#[test]
fn reconnect_and_unhealthy_windows_are_bounded() {
    crate::test_sandbox::assert_isolated();
    let mut delay = Duration::from_secs(1);
    for seconds in [1, 2, 4, 8, 16, 30, 30] {
        assert_eq!(delay, Duration::from_secs(seconds));
        delay = next_reconnect_delay(delay);
    }
    for attempt in 0..5 {
        for step in [
            "forwarding ports",
            "waiting for SSH forward",
            "probe",
            "start",
        ] {
            assert_eq!(
                retry_bind(attempt, step),
                attempt < 2 && matches!(step, "forwarding ports" | "waiting for SSH forward")
            );
        }
    }
    assert_eq!(probe_misses(2, true, true), (0, false));
    assert_eq!(probe_misses(2, true, false), (3, true));
    assert_eq!(probe_misses(0, false, true), (1, false));
    assert_eq!(
        bootstrap_timeout("agent-start"),
        Duration::from_millis(1_300_000)
    );
    assert_eq!(bootstrap_timeout("probe"), Duration::from_secs(45));
    assert!(!windows_probe_candidate(None));
    assert!(!windows_probe_candidate(Some(0)));
    assert!(windows_probe_candidate(Some(9009)));
    assert!(!windows_probe_candidate(Some(9008)));
    assert!(windows_shell_report("noise\nWindows_NT\r\n"));
    assert!(!windows_shell_report("xWindows_NT"));
    assert!(windows_node_report(" win32\n"));
    assert!(run_argv(
        &Config::decode(&json!({"id":"mini","label":"Private","target":"host"})).unwrap(),
        "../unowned",
        "node",
        &[]
    )
    .is_err());
    assert_eq!(
        windows_error("mini", "probe", Value::Null)["cause"]["_tag"],
        "SshWindowsHostError"
    );
}
