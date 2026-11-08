//! Real settings RPC writes use only private synthetic profiles and exact owned files.
#[path = "support/sandbox.rs"]
mod sandbox;
mod support;
use serde_json::{json, Value};
fn stored(server: &support::Server) -> Value {
    serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap()
}
fn slot(name: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(name)
}
#[test]
fn private_source_credentials_commit_echo_replace_clear_and_reopen_without_identity_changes() {
    sandbox::assert_isolated();
    let server = support::start(|_| {});
    let client = server.connect();
    let identity = std::fs::read(server.dir.join("secrets/server-signing-key.bin")).unwrap();
    let environment_id = std::fs::read(server.dir.join("environment-id")).unwrap();
    let value=client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"private-usage-one","enabled":false}},"sourceControlHosts":{"github":{"tokens":{"github.com":"private-forge-one"}}}}})).unwrap();
    assert!(!value.to_string().contains("private-usage-one"));
    assert!(!value.to_string().contains("private-forge-one"));
    let disk = stored(&server);
    assert_eq!(
        disk["usageLimitSources"]["synthetic"]["managementKey"],
        "••••••"
    );
    assert_eq!(
        disk["sourceControlHosts"]["github"]["tokens"]["github.com"],
        "••••••"
    );
    assert!(!disk.to_string().contains("private-"));
    let usage = server
        .dir
        .join("secrets")
        .join(format!("usage-limit-source-{}.bin", slot("synthetic")));
    assert_eq!(std::fs::read(&usage).unwrap(), b"private-usage-one");
    let config = client.rpc("server.getConfig", json!({})).unwrap();
    assert!(!config.to_string().contains("private-usage-one"));
    assert!(!config.to_string().contains("private-forge-one"));
    client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":disk["usageLimitSources"]["synthetic"]}}})).unwrap();
    assert_eq!(std::fs::read(&usage).unwrap(), b"private-usage-one");
    client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"private-usage-two","enabled":false}}}})).unwrap();
    assert_eq!(std::fs::read(&usage).unwrap(), b"private-usage-two");
    assert_eq!(
        stored(&server)["usageLimitSources"]["synthetic"]["managementKey"],
        "••••••"
    );
    let reopened = o3_server::store::State::open(&server.dir).unwrap();
    assert_eq!(
        o3_server::settings::get(&reopened).unwrap()["usageLimitSources"]["synthetic"]
            ["managementKey"],
        "••••••"
    );
    drop(reopened);
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"usageLimitSources":{"synthetic":null}}}),
        )
        .unwrap();
    assert!(!usage.exists());
    assert_eq!(
        std::fs::read(server.dir.join("secrets/server-signing-key.bin")).unwrap(),
        identity
    );
    assert_eq!(
        std::fs::read(server.dir.join("environment-id")).unwrap(),
        environment_id
    );
}
#[test]
fn failed_settings_publication_rolls_back_all_slots_and_keeps_previous_json() {
    sandbox::assert_isolated();
    let server = support::start(|_| {});
    let client = server.connect();
    client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"previous-private","enabled":false}},"sourceControlHosts":{"github":{"tokens":{"github.com":"previous-forge"}}}}})).unwrap();
    let previous = std::fs::read(server.dir.join("settings.json")).unwrap();
    let before = std::fs::read_dir(server.dir.join("secrets"))
        .unwrap()
        .map(|entry| {
            let p = entry.unwrap().path();
            (p.file_name().unwrap().to_owned(), std::fs::read(p).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let temporary = server
        .dir
        .join("settings.json")
        .with_extension(format!("{}.tmp", std::process::id()));
    std::fs::create_dir(&temporary).unwrap();
    let result=client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"replacement-private","enabled":false}},"sourceControlHosts":{"github":{"tokens":{"github.com":"replacement-forge"}}}}}));
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(server.dir.join("settings.json")).unwrap(),
        previous
    );
    let after = std::fs::read_dir(server.dir.join("secrets"))
        .unwrap()
        .map(|entry| {
            let p = entry.unwrap().path();
            (p.file_name().unwrap().to_owned(), std::fs::read(p).unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(after, before);
    assert!(temporary.is_dir());
}
#[test]
fn legacy_reference_echo_is_preserved_and_unactivated_provider_storage_stays_unchanged() {
    sandbox::assert_isolated();
    let reference = json!({"provider":"vault","reference":"op://synthetic/item/token"});
    let server = support::start(|s| {
        std::fs::write(s.dir.join("settings.json"),json!({"providerInstances":{"scripted":{"driver":"scripted","enabled":true,"environment":[{"name":"UNCHANGED","value":"provider-inline","sensitive":true}]}},"sourceControlHosts":{"github":{"tokens":{"github.com":reference}}}}).to_string()).unwrap();
    });
    let client = server.connect();
    let config = client.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(
        config["settings"]["sourceControlHosts"]["github"]["tokens"]["github.com"],
        "••••••"
    );
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"sourceControlHosts":{"github":{"tokens":{"github.com":"••••••"}}}}}),
        )
        .unwrap();
    assert_eq!(
        stored(&server)["sourceControlHosts"]["github"]["tokens"]["github.com"],
        reference
    );
    assert_eq!(
        stored(&server)["providerInstances"]["scripted"]["environment"][0]["value"],
        "provider-inline"
    );
    assert_eq!(
        std::fs::read_dir(server.dir.join("secrets"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn credential_only_change_refreshes_actual_http_consumer_with_identical_public_settings() {
    sandbox::assert_isolated();
    use std::{
        io::{Read, Write},
        time::Duration,
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = std::sync::mpsc::channel();
    let hub = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.extend(byte);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(request.len() < 16384);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /v0/management/auth-files "));
            let auth = request
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap()
                .to_owned();
            let body = "{\"files\":[]}";
            write!(socket,"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",body.len()).unwrap();
            sent.send(auth).unwrap();
        }
    });
    let server = support::start(|_| {});
    let client = server.connect();
    let source =
        |key: &str| json!({"kind":"cliproxy","url":url,"managementKey":key,"enabled":true});
    client.rpc("server.updateSettings",json!({"patch":{"providerHealthRefreshInterval":0,"usageLimitSources":{"synthetic":source("private-first")}}})).unwrap();
    let stream = support::subscribe(
        server.connect(),
        "subscribeServerConfig",
        json!({"usageLimitSources":true}),
    );
    assert!(received
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .ends_with("Bearer private-first"));
    let public_before = stored(&server);
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"usageLimitSources":{"synthetic":source("private-second")}}}),
        )
        .unwrap();
    assert_eq!(stored(&server), public_before);
    assert!(received
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .ends_with("Bearer private-second"));
    drop(stream);
    hub.join().unwrap();
}

#[test]
fn materialization_failure_rolls_back_new_slot_without_touching_bad_entry() {
    sandbox::assert_isolated();
    let server = support::start(|s| {
        std::fs::write(s.dir.join("settings.json"),json!({"usageLimitSources":{"bad":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"••••••","enabled":false}}}).to_string()).unwrap();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("bad");
        std::fs::create_dir(
            s.dir
                .join("secrets")
                .join(format!("usage-limit-source-{encoded}.bin")),
        )
        .unwrap();
    });
    let previous = std::fs::read(server.dir.join("settings.json")).unwrap();
    let client = server.connect();
    let error=client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"new":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":"must-be-rolled-back","enabled":false}}}})).unwrap_err();
    assert!(error.to_string().contains("read-secret"));
    assert_eq!(
        std::fs::read(server.dir.join("settings.json")).unwrap(),
        previous
    );
    assert!(!server
        .dir
        .join("secrets")
        .join(format!("usage-limit-source-{}.bin", slot("new")))
        .exists());
    assert!(server
        .dir
        .join("secrets")
        .join(format!("usage-limit-source-{}.bin", slot("bad")))
        .is_dir());
}
#[test]
fn malformed_selected_fields_are_rejected_before_secret_mutation() {
    sandbox::assert_isolated();
    let server = support::start(|_| {});
    let client = server.connect();
    let previous = std::fs::read(server.dir.join("settings.json")).unwrap();
    assert!(client.rpc("server.updateSettings",json!({"patch":{"usageLimitSources":{"synthetic":{"kind":"cliproxy","url":"http://127.0.0.1:1","managementKey":42}}}})).is_err());
    assert_eq!(
        std::fs::read(server.dir.join("settings.json")).unwrap(),
        previous
    );
    assert_eq!(
        std::fs::read_dir(server.dir.join("secrets"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn managed_settings_startup_gate_stays_closed_without_private_mutation() {
    sandbox::assert_isolated();
    let directory = std::env::temp_dir().join(format!(
        "private-settings-policy-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = o3_server::store::State::open(&directory).unwrap();
    let original = json!({"usageLimitSources":{}}).to_string();
    std::fs::write(directory.join("settings.json"), &original).unwrap();
    let signing = state.signing_key.clone();
    drop(state);
    let policy=o3_server::managed_policy::Policy::parse(std::path::Path::new("/synthetic/policy.json"),r#"{"schemaVersion":1,"organization":"Synthetic Corp","settings":{"usageLimitSources":{"locked":true,"value":{}}}}"#).unwrap();
    let error = match o3_server::store::State::open_with_policy(&directory, policy) {
        Ok(_) => panic!("managed settings readiness gate unexpectedly opened"),
        Err(error) => error,
    };
    assert!(error.contains("Settings managed policy controls"));
    assert_eq!(
        std::fs::read_to_string(directory.join("settings.json")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(directory.join("secrets/server-signing-key.bin")).unwrap(),
        signing
    );
    assert_eq!(
        std::fs::read_dir(directory.join("secrets"))
            .unwrap()
            .count(),
        1
    );
}
