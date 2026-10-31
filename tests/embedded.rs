//! The future OmaBridge LocalService can hand this exact handler a bounded DuplexStream.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
#[test]
fn shared_state_is_send_sync_and_duplex_uses_the_authenticated_http_ws_router() {
    fn require_shared<T: Send + Sync + 'static>() {}
    require_shared::<o3_server::store::State>();
    let dir = std::env::temp_dir().join(format!(
        "o3-duplex-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = Arc::new(o3_server::store::State::open(&dir).unwrap());
    let token = state
        .issue_session("test", &["orchestration:read".into()], 60000, "test")
        .unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for (path, status) in [
                ("/.well-known/t3/environment", 200),
                ("/api/orchestration/shell", 426),
                ("/api/auth/clients", 401),
            ] {
                let (mut client, server) = tokio::io::duplex(4096);
                let state = state.clone();
                let task =
                    tokio::spawn(async move { o3_server::serve_io(server, state).await.unwrap() });
                client
                    .write_all(
                        format!(
                            "GET {path} HTTP/1.1\r\nHost: embedded\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut response = Vec::new();
                client.read_to_end(&mut response).await.unwrap();
                let response = String::from_utf8(response).unwrap();
                assert!(
                    response
                        .lines()
                        .next()
                        .unwrap()
                        .contains(&status.to_string()),
                    "{response}"
                );
                task.await.unwrap();
            }
            let (client, server) = tokio::io::duplex(4096);
            let shared = state.clone();
            let task =
                tokio::spawn(async move { o3_server::serve_io(server, shared).await.unwrap() });
            let mut request = "ws://embedded/ws?orchestrationProtocol=2"
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("Authorization", format!("Bearer {token}").parse().unwrap());
            let (mut ws, _) = tokio_tungstenite::client_async(request, client)
                .await
                .unwrap();
            ws.send(Message::Text(
                json!({"_tag":"Request","id":"embedding","tag":"server.getConfig","payload":{}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            let response: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(response["requestId"], "embedding");
            assert_eq!(
                response["exit"]["value"]["environment"]["environmentId"],
                state.environment_id
            );
            ws.send(Message::Text(json!({"_tag":"Eof"}).to_string().into()))
                .await
                .unwrap();
            assert!(matches!(
                ws.next().await,
                Some(Ok(Message::Close(_))) | None
            ));
            task.await.unwrap();
        });
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}
