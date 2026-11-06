use super::*;

#[tokio::test]
async fn private_op_management_key_reaches_only_the_synthetic_hub() {
    crate::test_sandbox::assert_isolated();
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "o3-usage-secret-{}",
        crate::store::random_id().unwrap()
    ));
    let mut state = State::open(&directory).unwrap();
    let binary = directory.join("op");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/secret-op.py"),
        &binary,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        directory.join("case.json"),
        json!({"read":{"op://fixture/hub/key":{"stdout":"private-management-key"}}}).to_string(),
    )
    .unwrap();
    state.secret_resolver = Arc::new(crate::secret_resolver::SecretResolver::new(vec![Arc::new(
        crate::secret_resolver::OnePassword::with_binary(binary),
    )]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = json!({"secretProviders":{"vault":{"kind":"one_password","config":{"account":"fixture.1password.test"}}},"usageLimitSources":{"hub":{"kind":"cliproxy","url":base,"managementKey":{"provider":"vault","reference":"op://fixture/hub/key"}}}});
    std::fs::write(directory.join("settings.json"), settings.to_string()).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.extend(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let body = "{\"files\":[]}";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    let sources = tokio::time::timeout(std::time::Duration::from_secs(5), refresh(&state))
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0]["error"].is_null(), "{sources:?}");
    let request = server.await.unwrap().to_lowercase();
    assert!(request.starts_with("get /v0/management/auth-files http/1.1\r\n"));
    assert!(request.contains("authorization: bearer private-management-key"));
    std::fs::write(
        directory.join("case.json"),
        json!({"read":{"op://fixture/hub/key":{"code":1}}}).to_string(),
    )
    .unwrap();
    crate::settings_secrets::invalidate(state.secret_resolver().as_ref(), Some(&settings)).await;
    let sources = refresh(&state).await;
    assert_eq!(
        sources[0]["error"],
        "Could not read the management key from its secret provider."
    );
    let mut blank = settings.clone();
    blank["usageLimitSources"]["hub"]["managementKey"] = json!("");
    std::fs::write(directory.join("settings.json"), blank.to_string()).unwrap();
    assert_eq!(
        refresh(&state).await[0]["error"],
        "No management key configured."
    );
    assert!(!crate::settings::get(&state)
        .unwrap()
        .to_string()
        .contains("private-management-key"));
}
