//! Relay endpoint metadata stays derived and private; this starts no relay or provider.
mod support;
use serde_json::json;

#[test]
fn task_responses_derive_relay_urls_and_rotation_preserves_native_private_configuration() {
    assert!(std::path::Path::new("/run/.containerenv").is_file());
    let server = support::start(|_| {});
    let client = server.connect();
    let secrets = server.state.dir.join("secrets");
    let config = json!({"providerKind":"t3_relay","connectorToken":"fixture-private-connector","tunnelName":"managed-endpoint-0123456789abcdef"}).to_string();
    std::fs::write(
        secrets.join("cloud-relay-url.bin"),
        b"https://relay.example///",
    )
    .unwrap();
    std::fs::write(secrets.join("cloud-endpoint-runtime-config.bin"), &config).unwrap();
    let task = client.rpc("scheduledTasks.upsert",json!({"id":"relay:hook","title":"Relay","prompt":"Inspect request","enabled":false,"schedule":{"type":"webhook","signature":null,"maxDeliveryAgeMinutes":null},"projectId":"p","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let path = task["task"]["webhook"]["path"].as_str().unwrap();
    let expected = format!(
        "https://relay.example/v1/hooks/0123456789abcdef/{}",
        path.strip_prefix("/api/hooks/").unwrap()
    );
    assert!(expected.contains("/relay%3Ahook/"));
    assert_eq!(task["task"]["webhook"]["url"], expected);
    assert!(!task.to_string().contains("fixture-private-connector"));
    let stored: String = server
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT body FROM records WHERE kind='scheduledTasks' AND id='relay:hook'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !stored.contains("relay.example"),
        "derived origin must not become a retained task snapshot"
    );
    let listed = client.rpc("scheduledTasks.list", json!({})).unwrap();
    assert_eq!(listed["tasks"][0]["webhook"]["url"], expected);
    let reader = server.limited(&client, &["orchestration:read"]);
    assert!(
        reader.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"][0]
            .get("webhook")
            .is_none()
    );
    let rotated = client
        .rpc(
            "scheduledTasks.rotateWebhookToken",
            json!({"id":"relay:hook"}),
        )
        .unwrap();
    assert_ne!(rotated["task"]["webhook"]["path"], path);
    assert_eq!(
        rotated["task"]["webhook"]["url"],
        format!(
            "https://relay.example/v1/hooks/0123456789abcdef/{}",
            rotated["task"]["webhook"]["path"]
                .as_str()
                .unwrap()
                .strip_prefix("/api/hooks/")
                .unwrap()
        )
    );
    assert_eq!(
        std::fs::read_to_string(secrets.join("cloud-endpoint-runtime-config.bin")).unwrap(),
        config
    );
    std::fs::write(
        secrets.join("cloud-endpoint-runtime-config.bin"),
        b"invalid retained configuration",
    )
    .unwrap();
    let listed = client.rpc("scheduledTasks.list", json!({})).unwrap();
    assert!(listed["tasks"][0]["webhook"]["url"].is_null());
    assert_eq!(
        std::fs::read(secrets.join("cloud-endpoint-runtime-config.bin")).unwrap(),
        b"invalid retained configuration"
    );
}
