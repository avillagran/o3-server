//! Actual authenticated State sessions and framed RPC connection retirement.
//! No browser, provider process, filesystem cleanup or host command is used.
use super::*;
use tokio::io::DuplexStream;
use tokio_tungstenite::tungstenite::protocol::Role;
type Client = WebSocketStream<DuplexStream>;
async fn connect(state: &Arc<State>, scope: &str) -> (Client, tokio::task::JoinHandle<()>) {
    let token = state
        .issue_session(
            "profile-report-fixture",
            &[scope.to_owned()],
            60_000,
            "fixture",
        )
        .unwrap();
    let session = state.session(&token).unwrap();
    let (client, server) = tokio::io::duplex(65536);
    let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let task = tokio::spawn(run(server, state.clone(), session));
    (client, task)
}
async fn request(client: &mut Client, id: &str, input: Value) -> Value {
    client
        .send(Message::Text(
            json!({"_tag":"Request","id":id,"tag":"preview.reportProfiles","payload":input})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let frame = client.next().await.unwrap().unwrap();
            if let Message::Text(text) = frame {
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["_tag"] == "Exit" && frame["requestId"] == id {
                    return frame;
                }
            }
        }
    })
    .await
    .unwrap()
}
async fn disconnect(mut client: Client, task: tokio::task::JoinHandle<()>) {
    client.close(None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
fn report(id: &str, rate: Option<u8>) -> Value {
    let mut value =
        json!({"profiles":[{"id":id,"name":id,"kind":"persistent"}],"defaultProfileId":id});
    if let Some(rate) = rate {
        value["recordingFrameRate"] = json!(rate);
    }
    value
}
#[tokio::test]
async fn actual_ws_reports_union_update_and_retire_by_connection_not_input_token() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "profile-report-{}",
        crate::store::random_id().unwrap()
    ));
    let state = Arc::new(State::open(&root).unwrap());
    let (mut a, a_task) = connect(&state, "preview:operate").await;
    let (mut b, b_task) = connect(&state, "preview:operate").await;
    let (mut denied, denied_task) = connect(&state, "diagnostics:read").await;
    let initial = state.streams.p2.preview.profile_catalog();
    let refusal = request(&mut denied, "denied", report("forged", Some(60))).await;
    assert_eq!(
        refusal["exit"]["cause"][0]["error"]["_tag"],
        "EnvironmentAuthorizationError"
    );
    assert_eq!(state.streams.p2.preview.profile_catalog(), initial);
    let mut first = report("a", Some(60));
    first["clientId"] = json!("forged-token");
    assert_eq!(
        request(&mut a, "a1", first).await["exit"],
        json!({"_tag":"Success"})
    );
    assert_eq!(
        request(&mut b, "b1", report("b", None)).await["exit"],
        json!({"_tag":"Success"})
    );
    assert_eq!(
        state.streams.p2.preview.profile_catalog().status()["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["default", "incognito", "a", "b"]
    );
    assert_eq!(
        request(&mut a, "a2", report("a", None)).await["exit"],
        json!({"_tag":"Success"})
    );
    let reordered = state.streams.p2.preview.profile_catalog();
    assert_eq!(
        reordered.status()["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["default", "incognito", "b", "a"]
    );
    assert_eq!(reordered.default_profile_id, Some("a".into()));
    assert_eq!(reordered.recording_frame_rate, 60);
    let malformed = request(
        &mut a,
        "bad",
        json!({"profiles":[],"defaultProfileId":"default","recordingFrameRate":null}),
    )
    .await;
    assert_eq!(malformed["exit"]["cause"][0]["_tag"], "Die");
    assert_eq!(state.streams.p2.preview.profile_catalog(), reordered);
    disconnect(a, a_task).await;
    let retained = state.streams.p2.preview.profile_catalog();
    assert_eq!(retained.default_profile_id, Some("b".into()));
    assert_eq!(retained.status()["profiles"].as_array().unwrap().len(), 3);
    disconnect(b, b_task).await;
    assert_eq!(state.streams.p2.preview.profile_catalog(), retained);
    disconnect(denied, denied_task).await;
    assert_eq!(state.streams.p2.preview.profile_catalog(), retained);
    // Files are retained under the private container's owned fixture root.
}
