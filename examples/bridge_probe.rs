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
        let root = root.to_path_buf();
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
            if let Ok(tid) = std::env::var("O3_RSS_THREAD") {
                use serde_json::json;
                use std::time::Instant;
                use std::sync::atomic::{AtomicBool, Ordering};
                let cancel = Arc::new(AtomicBool::new(false));
                let mut streams = Vec::new();
                for (stage, tag, payload) in [
                    ("shell", "orchestration.subscribeShell", json!({})),
                    ("thread", "orchestration.subscribeThread", json!({"threadId":tid,"acceptBoundedSnapshot":true,"acceptCompactIds":true,"acceptDedupedBodies":true,"acceptCompactTurnItems":true})),
                ] {
                    let conn = conn.clone();
                    let root = root.clone();
                    let cancel = cancel.clone();
                    streams.push(std::thread::spawn(move || {
                        let mut seen = false;
                        let start = Instant::now();
                        conn.subscribe(tag, payload, || cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(120), |v| {
                            if v.is_some() && !seen {
                                std::fs::write(root.join(stage), "snapshot received").unwrap();
                                seen = true;
                            }
                            Ok(())
                        }).unwrap();
                        assert!(seen, "missing snapshot");
                    }));
                }
                let deadline = Instant::now() + Duration::from_secs(90);
                while !root.join("shell").exists() || !root.join("thread").exists() {
                    assert!(Instant::now() < deadline, "snapshot timeout");
                    std::thread::sleep(Duration::from_millis(20));
                }
                // Each real protocol RPC opens a connection: exercise retirement,
                // including repeated embedded serve_io initialization.
                for _ in 0..100 {
                    conn.rpc("server.getConfig", json!({})).unwrap();
                }
                std::fs::write(root.join("churn"), "100 RPC completed").unwrap();
                let cwd = root.join("workspace");
                std::fs::create_dir_all(&cwd).unwrap();
                conn.rpc("projects.mutate",json!({"type":"project.create","commandId":"rss-project","projectId":"rss-project","title":"RSS","workspaceRoot":cwd})).unwrap();
                conn.rpc("orchestration.launchThread",json!({"commandId":"rss-launch","threadId":"rss-turn","projectId":"rss-project","title":"RSS","modelSelection":{"instanceId":"claudeAgent","model":"haiku","options":[]},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"Reply with exactly O3_OK. Do not use tools.","attachments":[]}})).unwrap();
                let deadline = Instant::now() + Duration::from_secs(90);
                loop {
                    let p = conn.rpc("orchestration.getThreadProjection",json!({"threadId":"rss-turn"})).unwrap();
                    assert_ne!(p["runs"][0]["status"], "failed", "native turn failed");
                    if p["runs"][0]["status"] == "completed" {
                        assert!(p["messages"].as_array().unwrap().iter().any(|m|m["role"]=="assistant" && m["text"].as_str().is_some_and(|t|t.contains("O3_OK"))));
                        break;
                    }
                    assert!(Instant::now() < deadline, "turn timeout");
                    std::thread::sleep(Duration::from_millis(100));
                }
                std::fs::write(root.join("turn"), "completed").unwrap();
                std::thread::sleep(Duration::from_secs(3));
                cancel.store(true, Ordering::Relaxed);
                for stream in streams { stream.join().unwrap(); }
            } else {
                std::thread::sleep(Duration::from_secs(12));
            }
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
