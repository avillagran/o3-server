//! Isolated measurement client. Only run against a freshly created measurement profile.
#[cfg(feature = "bridge")]
fn main() {
    use std::{sync::Arc, time::Duration};
    let args: Vec<_> = std::env::args().collect();
    let root = std::path::Path::new(&args[1]);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let host = omabridge::Store::open(&root.join("server/omabridge"), "host").unwrap();
        let guest = omabridge::Store::open(&root.join("guest"), "guest").unwrap();
        let now = omabridge::unix_time().unwrap();
        let invite = host
            .create_invite(&["t3".into()], Duration::from_secs(60), now)
            .unwrap();
        let invite = omabridge::Invitation {
            version: 1,
            address: host.active_endpoint().unwrap().0,
            token: invite,
            name: "host".into(),
            expires: now + 60,
            manage: false,
        };
        let client = Arc::new(
            omabridge::Client::new(guest.clone(), omabridge::NetOptions::loopback())
                .await
                .unwrap(),
        );
        client.pair(&invite, "guest", now).await.unwrap();
        host.approve(&guest.id().unwrap(), now).unwrap();
        client.pair(&invite, "guest", now).await.unwrap();
        let (addr, _) = host.active_endpoint().unwrap();
        let pairing = client.t3_pair(&addr).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let forward = tokio::spawn(client.clone().forward(addr, "t3".into(), listener));
        let ready = root.join("ready");
        tokio::task::spawn_blocking(move || {
            let options = t3_protocol::ConnectionOptions::default();
            let paired = t3_protocol::pair_bearer(
                &base,
                &pairing.credential,
                None,
                &Default::default(),
                &options,
            )
            .unwrap();
            let conn = paired.connection(&options).unwrap();
            conn.rpc("server.getConfig", serde_json::json!({})).unwrap();
            std::fs::write(ready, "rpc succeeded").unwrap();
            std::thread::sleep(Duration::from_secs(12));
            drop(conn);
        })
        .await
        .unwrap();
        forward.abort();
        client.close().await;
    });
}
#[cfg(not(feature = "bridge"))]
fn main() {}
