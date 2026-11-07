//! Canonical speech settings use private slots without advertising an unimplemented audio service.
mod support;
use serde_json::json;

#[test]
fn cloud_credentials_use_owned_private_slots_and_survive_unrelated_updates() {
    let server = support::start(|_| {});
    let client = server.connect();
    let credentials = json!({"openai":" synthetic-openai ","xai":"synthetic-xai","openrouter":"synthetic-openrouter"});
    let expected = json!({"openai":"synthetic-openai","xai":"synthetic-xai","openrouter":"synthetic-openrouter"});
    let public = client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"cloudSpeechCredentials":credentials}}),
        )
        .unwrap();
    for provider in ["openai", "xai", "openrouter"] {
        assert_eq!(
            public["cloudSpeechCredentials"][provider],
            o3_server::settings::SENTINEL
        );
        let path = server
            .dir
            .join("secrets")
            .join(format!("voice-cloud-{provider}.bin"));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            expected[provider].as_str().unwrap()
        );
    }
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap();
    assert!(!stored.to_string().contains("synthetic-openai"));
    assert!(!stored.to_string().contains("synthetic-xai"));
    assert!(!stored.to_string().contains("synthetic-openrouter"));
    let private = o3_server::settings_secrets::materialize_profile(&server.dir, &stored).unwrap();
    assert_eq!(private["cloudSpeechCredentials"], expected);
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"sidebarAutoSettleAfterDays":9}}),
        )
        .unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        o3_server::settings_secrets::materialize_profile(&server.dir, &stored).unwrap()
            ["cloudSpeechCredentials"],
        expected
    );
    let config = client.rpc("server.getConfig", json!({})).unwrap();
    assert_ne!(
        config["environment"]["capabilities"]["cloudVoiceTranscription"],
        true
    );
    assert_ne!(
        config["environment"]["capabilities"]["voiceTranscription"],
        true
    );
    assert!(!config.to_string().contains("synthetic-openai"));
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"cloudSpeechCredentials":{"openrouter":""}}}),
        )
        .unwrap();
    assert!(!server
        .dir
        .join("secrets/voice-cloud-openrouter.bin")
        .exists());
    assert_eq!(
        std::fs::read_to_string(server.dir.join("secrets/voice-cloud-openai.bin")).unwrap(),
        "synthetic-openai"
    );
    let reference = json!({"provider":"vault","reference":"op://Private/Voice/key"});
    client.rpc("server.updateSettings",json!({"patch":{"secretProviders":{"vault":{"kind":"one_password","label":"Private fixture","config":{"account":"fixture"}}},"cloudSpeechCredentials":{"xai":reference}}})).unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(stored["cloudSpeechCredentials"]["xai"], reference);
    assert_eq!(
        o3_server::settings_secrets::materialize_profile(&server.dir, &stored).unwrap()
            ["cloudSpeechCredentials"]["xai"],
        reference
    );
    client
        .rpc(
            "server.updateSettings",
            json!({"patch":{"secretProviders":{"vault":null}}}),
        )
        .unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(server.dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(stored["cloudSpeechCredentials"]["xai"], "");
}
