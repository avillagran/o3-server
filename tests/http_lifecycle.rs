use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn public_http_churn_does_not_start_repository_identity_polling() {
    let root = std::env::temp_dir().join(format!(
        "o3-poller-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = Arc::new(o3_server::store::State::open(&root).unwrap());
    o3_server::projects::mutate(
        &state,
        &serde_json::json!({
            "type":"project.create", "projectId":"p", "commandId":"p",
            "title":"Polling", "workspaceRoot":root,
        }),
    )
    .unwrap();
    for _ in 0..24 {
        let (mut client, server) = tokio::io::duplex(4096);
        let state = state.clone();
        let serving =
            tokio::spawn(async move { o3_server::serve_io(server, state).await.unwrap() });
        client.write_all(b"GET /.well-known/t3/environment HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        serving.await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let count: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE shell LIKE '%repository-identities-resolved%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "public HTTP requests must not trigger discovery");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
