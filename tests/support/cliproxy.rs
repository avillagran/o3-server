use super::*;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/cliproxy-4288.json")).unwrap()
}
#[test]
fn actual_native_api_schemas_and_every_ast_node() {
    let fixture = fixture();
    for case in fixture["schemaCases"].as_array().unwrap() {
        let result = decode(case["root"].as_str().unwrap(), &case["input"]);
        if case["invalid"] == true {
            assert!(result.is_err(), "{case}");
        } else {
            assert_eq!(result, Ok(case["decoded"].clone()), "{case}");
        }
    }
    for case in fixture["nodeCases"].as_array().unwrap() {
        let result = decode_node(
            index(&case["node"]),
            fixture["schemaValues"].get(index(&case["valueIndex"])),
        );
        if case["invalid"] == true {
            assert!(result.is_err(), "{case}: {result:?}");
        } else {
            assert_eq!(result, Ok(case["decoded"].get("value").cloned()), "{case}");
        }
    }
}
#[test]
fn actual_native_redemption_ids_and_safe_failure_details() {
    let fixture = fixture();
    for case in fixture["numberCases"].as_array().unwrap() {
        assert_eq!(
            js_number(case["value"].as_f64().unwrap()),
            case["text"],
            "{case}"
        );
    }
    for case in fixture["idsCases"].as_array().unwrap() {
        assert_eq!(
            redemption_id(
                case["account"].as_str().unwrap(),
                case["credit"].as_str().unwrap()
            ),
            case["id"],
            "{case}"
        );
    }
    for case in fixture["failureCases"].as_array().unwrap() {
        let result = match case["kind"].as_str().unwrap() {
            "transport" => transport_detail(case["code"].as_str()),
            "status" => format!("The hub answered with HTTP {}.", case["status"]),
            _ => unreachable!(),
        };
        assert_eq!(result, case["expected"], "{case}");
    }
}
async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
    use tokio::io::AsyncReadExt;
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
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 1024 * 1024);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (head, body)
}
#[tokio::test]
async fn actual_native_reset_traces_through_private_http() {
    crate::test_sandbox::assert_isolated();
    use tokio::io::AsyncWriteExt;
    for case in fixture()["apiCases"].as_array().unwrap() {
        if case["operation"] == "readAccounts" || case["label"] == "timeout" {
            continue;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let expected = case["calls"].as_array().unwrap().clone();
        let answers = case["answers"].as_array().unwrap().clone();
        let base = origin.clone();
        let server = tokio::spawn(async move {
            let mut calls = Vec::new();
            for (index, _) in expected.iter().enumerate() {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (head, body) = read_request(&mut socket).await;
                let mut first = head.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let target = first.next().unwrap();
                let authorization = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_owned())
                    })
                    .unwrap();
                let mut call = json!({"method":method,"url":format!("{base}{target}"),"authorization":authorization});
                if !body.is_null() {
                    call["body"] = body;
                }
                calls.push(call);
                let answer = &answers[index];
                let status = answer["status"].as_u64().unwrap_or(200);
                let body: Vec<u8> = if let Some(bytes) = answer["bytes"].as_array() {
                    bytes
                        .iter()
                        .map(|value| value.as_u64().unwrap() as u8)
                        .collect()
                } else {
                    answer["body"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| answer["body"].to_string())
                        .into_bytes()
                };
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
            calls
        });
        let configured = case["hub"]["url"].as_str().unwrap();
        let configured = configured.replace("https://fixture.invalid", &origin);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            if case["operation"] == "consume" {
                consume(
                    &configured,
                    case["hub"]["managementKey"].as_str().unwrap(),
                    case["input"]["accountId"].as_str().unwrap(),
                    case["input"]["creditId"].as_str().unwrap(),
                )
                .await
            } else {
                credits(
                    &configured,
                    case["hub"]["managementKey"].as_str().unwrap(),
                    &case["input"]["account"],
                    fixture()["now"].as_i64().unwrap(),
                )
                .await
                .map(Value::Array)
            }
        })
        .await
        .expect("private hub timeout")
        .map_err(ApiError::detail);
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result, Err(error.into()), "{}", case["label"]);
        } else {
            assert_eq!(result, Ok(case["result"].clone()), "{}", case["label"]);
        }
        let calls = tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        let mut expected = case["calls"].clone();
        for call in expected.as_array_mut().unwrap() {
            call["url"] = json!(call["url"]
                .as_str()
                .unwrap()
                .replace("https://fixture.invalid", &origin));
        }
        assert_eq!(json!(calls), expected, "{}", case["label"]);
    }
}

#[tokio::test]
async fn private_management_transport_refusal_and_native_deadline_are_safe() {
    crate::test_sandbox::assert_isolated();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let refused = consume(&base, "private-token-must-not-escape", "account", "credit")
        .await
        .unwrap_err()
        .detail();
    assert_eq!(refused, "The hub request failed (ECONNREFUSED).");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (head, _) = read_request(&mut socket).await;
        assert!(head.starts_with("GET /v0/management/auth-files HTTP/1.1\r\n"));
        std::future::pending::<()>().await;
        drop(socket);
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        consume(&base, "private-token-must-not-escape", "account", "credit"),
    )
    .await
    .unwrap()
    .unwrap_err()
    .detail();
    assert_eq!(result, "The hub did not answer within 15 seconds.");
    assert!(!result.contains("private-token"));
    peer.abort();
    assert!(peer.await.unwrap_err().is_cancelled());
}
