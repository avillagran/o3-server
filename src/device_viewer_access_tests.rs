//! Human viewer admission through real HTTP and the owned synthetic SSH hub.
use super::ssh_fixture;
use crate::store::State;
use serde_json::json;
use std::{sync::Arc, time::Duration};
async fn request(
    state: Arc<State>,
    method: &str,
    path: &str,
    token: &str,
    upgrade: bool,
) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let serving = tokio::spawn(crate::http::serve_io(server, state));
    let upgrade = if upgrade {
        "Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n"
    } else {
        ""
    };
    client.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n{upgrade}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    serving.await.unwrap().unwrap();
    response
}
fn status(response: &[u8]) -> u16 {
    std::str::from_utf8(&response[..12])
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
fn token(state: &State, scopes: &[&str]) -> String {
    state
        .issue_session(
            "private-viewer",
            &scopes
                .iter()
                .map(|scope| scope.to_string())
                .collect::<Vec<_>>(),
            60_000,
            "Private viewer fixture",
        )
        .unwrap()
}
#[tokio::test]
async fn native_human_viewer_and_scopes_work_without_agent_access() {
    crate::test_sandbox::assert_isolated();
    let fixture = ssh_fixture::Fixture::make(0).await;
    let state = Arc::new(State::open(&fixture.directory.join("viewer-profile")).unwrap());
    state
        .streams
        .p2
        .device
        .register_ssh_executable_test(fixture.executable.clone());
    crate::settings::update(&state,&json!({"patch":{"enableDeviceSupport":true,"enableAgentDeviceAccess":false,"deviceHosts":[fixture.cli.config]}})).unwrap();
    crate::device::call(&state, "device.list", &json!({}))
        .await
        .unwrap();
    let read = token(&state, &["orchestration:read"]);
    let operate = token(&state, &["orchestration:operate"]);
    let screenshot = "/api/device-hub/vendor/serve-sim/api/screenshot?device=sim-1&hostId=mini";
    let response = request(state.clone(), "GET", screenshot, &read, false).await;
    assert_eq!(status(&response), 200);
    assert!(response
        .windows(19)
        .any(|bytes| bytes == b"private-ssh-fixture"));
    assert_eq!(
        status(&request(state.clone(), "POST", screenshot, &read, false).await),
        200,
        "native screenshot POST requires read, not operate"
    );
    assert_eq!(
        status(&request(state.clone(), "GET", screenshot, &operate, false).await),
        403,
        "operate does not grant an absent read scope"
    );
    for method in ["GET", "HEAD"] {
        assert_eq!(
            status(
                &request(
                    state.clone(),
                    method,
                    "/api/device-hub/api/devices?hostId=mini",
                    &read,
                    false
                )
                .await
            ),
            200
        );
    }
    let control = "/api/device-hub/vendor/serve-emu/api/stream-mode?hostId=mini";
    assert_eq!(
        status(&request(state.clone(), "POST", control, &read, false).await),
        403
    );
    assert_eq!(
        status(&request(state.clone(), "POST", control, &operate, false).await),
        200
    );
    assert_eq!(
        status(
            &request(
                state.clone(),
                "GET",
                "/api/device-hub/vendor/serve-emu/ws?device=phone&hostId=mini",
                &read,
                true
            )
            .await
        ),
        403
    );
    // The HTTP-only fixture refuses the upstream upgrade. A read-scoped inventory
    // socket reaches it (502), whereas an operate-only caller is stopped locally.
    assert_eq!(
        status(
            &request(
                state.clone(),
                "GET",
                "/api/device-hub/api/devices/ws?hostId=mini",
                &read,
                true
            )
            .await
        ),
        502
    );
    assert_eq!(
        status(
            &request(
                state.clone(),
                "GET",
                "/api/device-hub/api/devices/ws?hostId=mini",
                &operate,
                true
            )
            .await
        ),
        403
    );
    let mcp = state
        .issue_session(
            "mcp-client",
            &["orchestration:read".into(), "orchestration:operate".into()],
            60_000,
            "Private MCP refusal fixture",
        )
        .unwrap();
    assert_eq!(
        status(&request(state.clone(), "GET", screenshot, &mcp, false).await),
        401,
        "An MCP client cannot authenticate the human media route"
    );
    let claims = state.session(&read).unwrap();
    let rpc_ticket = crate::auth::websocket_ticket(
        &claims,
        &state.signing_key,
        crate::store::now_ms(),
        Some("rpc"),
    );
    let response = request(
        state.clone(),
        "GET",
        &format!("{screenshot}&wsTicket={rpc_ticket}"),
        "",
        false,
    )
    .await;
    assert_eq!(status(&response), 401);
    let media_ticket = crate::auth::websocket_ticket(
        &claims,
        &state.signing_key,
        crate::store::now_ms(),
        Some("media"),
    );
    assert_eq!(
        status(
            &request(
                state.clone(),
                "GET",
                &format!("{screenshot}&wsTicket={media_ticket}"),
                "",
                false
            )
            .await
        ),
        200
    );
    assert_eq!(
        crate::settings::read(&state).unwrap()["enableAgentDeviceAccess"],
        false
    );
    assert!(
        crate::mcp_tools::catalog()
            .iter()
            .all(|tool| !tool["name"].as_str().unwrap().starts_with("device")),
        "No device MCP capability may be introduced by viewer admission"
    );
    let sid = claims["sid"].as_str().unwrap();
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE auth_sessions SET revoked_at=? WHERE session_id=?",
            rusqlite::params![crate::store::now_ms(), sid],
        )
        .unwrap();
    assert_eq!(
        status(
            &request(
                state.clone(),
                "GET",
                &format!("{screenshot}&wsTicket={media_ticket}"),
                "",
                false
            )
            .await
        ),
        401
    );
    let fresh_read = token(&state, &["orchestration:read"]);
    assert_eq!(
        status(&request(state.clone(), "GET", screenshot, &fresh_read, false).await),
        200
    );
    assert!(crate::mcp_tools::call(&state, "device.screenshot", &json!({})).is_err());
    crate::settings::update(&state, &json!({"patch":{"enableDeviceSupport":false}})).unwrap();
    assert_eq!(
        status(&request(state.clone(), "GET", screenshot, &fresh_read, false).await),
        403
    );
    crate::device::call(&state, "device.list", &json!({}))
        .await
        .unwrap();
    assert!(fixture.trace().iter().any(|entry| entry["mode"] == "stop"));
}

#[test]
fn websocket_scope_predicate_matches_the_complete_native_router() {
    crate::test_sandbox::assert_isolated();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/device-viewer-scope-4301.json"
    ))
    .unwrap();
    assert_eq!(fixture["websocketCases"].as_array().unwrap().len(), 12);
    for case in fixture["websocketCases"].as_array().unwrap() {
        let input = &case["input"];
        let plan =
            crate::device_native_proxy::plan(input["uri"].as_str().unwrap(), "GET", true).unwrap();
        let required = if plan.controls {
            "orchestration:operate"
        } else {
            "orchestration:read"
        };
        let allowed = input["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope == required);
        assert_eq!(
            case["expected"]["status"],
            if allowed { 503 } else { 403 },
            "{input}"
        );
    }
}
