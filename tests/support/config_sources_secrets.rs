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
    let settings = json!({"secretProviders":{"vault":{"kind":"one_password","label":"Private fixture vault","config":{"account":"fixture.1password.test"}}},"usageLimitSources":{"hub":{"kind":"cliproxy","url":base,"managementKey":{"provider":"vault","reference":"op://fixture/hub/key"}}}});
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

#[tokio::test]
async fn private_op_reset_route_serializes_refresh_and_publishes_redeemed_credit() {
    crate::test_sandbox::assert_isolated();
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "o3-usage-reset-{}",
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
        json!({"read":{"op://fixture/hub/key":{"stdout":"private-reset-key"}}}).to_string(),
    )
    .unwrap();
    state.secret_resolver = Arc::new(crate::secret_resolver::SecretResolver::new(vec![Arc::new(
        crate::secret_resolver::OnePassword::with_binary(binary),
    )]));
    let state = Arc::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let settings = json!({"secretProviders":{"vault":{"kind":"one_password","label":"Private fixture vault","config":{"account":"fixture.1password.test"}}},"usageLimitSources":{"hub":{"kind":"cliproxy","url":format!("{base}/management.html"),"managementKey":{"provider":"vault","reference":"op://fixture/hub/key"}}}});
    std::fs::write(directory.join("settings.json"), settings.to_string()).unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut entered = Some(entered_tx);
        let mut release = Some(release_rx);
        let mut redeemed = false;
        let mut requests = Vec::new();
        for _ in 0..12 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                head.extend(byte);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(head.len() < 64 * 1024);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head
                .to_lowercase()
                .contains("authorization: bearer private-reset-key\r\n"));
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length < 1024 * 1024);
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes).await.unwrap();
            let body: Value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            let response = if head.lines().next().unwrap().contains("/auth-files ") {
                json!({"files":[{"id":"account","auth_index":"auth-index","provider":"codex","id_token":{"chatgpt_account_id":"workspace"}}]})
            } else if head.lines().next().unwrap().contains("/reset-quota ") {
                assert_eq!(body, json!({"auth_index":"auth-index"}));
                json!({})
            } else {
                assert!(head.starts_with("POST /v0/management/api-call HTTP/1.1\r\n"));
                let url = body["url"].as_str().unwrap();
                let response = if url.ends_with("/consume") {
                    let data: Value = serde_json::from_str(body["data"].as_str().unwrap()).unwrap();
                    assert_eq!(
                        data,
                        json!({"redeem_request_id":"32841ada-7baf-55b8-9cf8-00c30ab64f8d","credit_id":"credit"})
                    );
                    assert_eq!(body["header"]["Chatgpt-Account-Id"], "workspace");
                    redeemed = true;
                    entered.take().unwrap().send(()).unwrap();
                    release.take().unwrap().await.unwrap();
                    json!({"code":"reset"})
                } else if url.ends_with("/rate-limit-reset-credits") {
                    json!({"credits":if redeemed {json!([])} else {json!([{"id":"credit","status":"available","reset_type":"codex_rate_limits","expires_at":"2099-01-01T00:00:00Z"}])}})
                } else {
                    assert!(url.ends_with("/usage"));
                    json!({"plan_type":"pro","rate_limit":null})
                };
                json!({"status_code":200,"body":response.to_string()})
            };
            requests.push((head, body));
            let response = response.to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    let before = refresh(&state).await;
    assert_eq!(
        before[0]["accounts"][0]["usageLimits"]["resetCredits"]["availableCount"],
        1
    );
    assert_eq!(
        before[0]["accounts"][0]["usageLimits"]["resetCredits"]["nextCreditId"],
        "credit"
    );
    let consuming = state.clone();
    let consumer = tokio::spawn(async move {
        crate::ops::call(
            &consuming,
            "provider.consumeResetCredit",
            &json!({"sourceId":"hub","accountId":" account ","creditId":" credit "}),
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(
        state.streams.ops.usage_sources.refresh.try_lock().is_err(),
        "redemption holds the same semaphore as refresh"
    );
    let refreshing = state.clone();
    let pending = tokio::spawn(async move { refresh(&refreshing).await });
    release_tx.send(()).unwrap();
    assert_eq!(consumer.await.unwrap(), Ok(json!({"outcome":"reset"})));
    let after = pending.await.unwrap();
    assert_eq!(
        after[0]["accounts"][0]["usageLimits"]["resetCredits"]["availableCount"],
        0
    );
    assert!(after[0]["accounts"][0]["usageLimits"]["resetCredits"]["nextCreditId"].is_null());
    assert_eq!(
        state.streams.ops.usage_sources.snapshot.lock().unwrap()[0]["accounts"][0]["usageLimits"]
            ["resetCredits"]["availableCount"],
        0
    );
    assert_eq!(server.await.unwrap().len(), 12);
    std::fs::write(
        directory.join("case.json"),
        json!({"read":{"op://fixture/hub/key":{"code":1}}}).to_string(),
    )
    .unwrap();
    state.secret_resolver.invalidate(None).await;
    assert_eq!(
        consume_reset_credit(
            &state,
            &json!({"sourceId":"hub","accountId":"account","creditId":"credit"})
        )
        .await,
        Err(
            json!({"_tag":"UsageLimitSourceError","detail":"Could not read the management key from its secret provider."})
        )
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("settings.json")).unwrap(),
        settings.to_string()
    );
    assert!(!crate::settings::get(&state)
        .unwrap()
        .to_string()
        .contains("private-reset-key"));
}

#[tokio::test]
async fn native_bom_private_file_value_reaches_actual_http_header_without_marker_or_bom() {
    crate::test_sandbox::assert_isolated();
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/secret-store-4288.json")).unwrap();
    let case = fixture["profileCases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "bom-ascii-usage")
        .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "usage-native-bom-{}",
        crate::store::random_id().unwrap()
    ));
    let state = State::open(&directory).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = case["settings"].clone();
    settings["usageLimitSources"]["hub"]["url"] =
        json!(format!("http://{}", listener.local_addr().unwrap()));
    std::fs::write(directory.join("settings.json"), settings.to_string()).unwrap();
    for (path, file) in case["files"].as_object().unwrap() {
        let name = path.strip_prefix("/private-fixture/secrets/").unwrap();
        assert!(!name.contains('/'));
        let bytes: Vec<u8> = serde_json::from_value(file["bytes"].clone()).unwrap();
        std::fs::write(directory.join("secrets").join(name), bytes).unwrap();
    }
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
            assert!(request.len() < 16384);
        }
        let body = "{\"files\":[]}";
        socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let sources = tokio::time::timeout(std::time::Duration::from_secs(5), refresh(&state))
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0]["error"].is_null(), "{sources:?}");
    let request = server.await.unwrap();
    let expected = case["result"]["usageLimitSources"]["hub"]["managementKey"]
        .as_str()
        .unwrap();
    assert!(request.contains(&format!("authorization: Bearer {expected}\r\n")));
    assert!(!request.contains('\u{feff}'));
    assert!(!crate::settings::get(&state)
        .unwrap()
        .to_string()
        .contains(expected));
    assert!(!serde_json::to_string(&sources).unwrap().contains(expected));
}
