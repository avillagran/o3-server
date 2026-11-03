#![cfg(feature = "bridge")]
use omabridge::{Client, NetOptions, Store};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[test]
fn real_client_pairs_and_runs_rpc_through_embedded_bridge() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(45), async {
            let root = std::env::temp_dir().join(format!(
                "o3-bridge-{}",
                o3_server::store::random_id().unwrap()
            ));
            let state =
                Arc::new(o3_server::store::State::open(&root.join("profile/userdata")).unwrap());
            let host_store = Store::open(&root.join("host"), "host").unwrap();
            let guest_store = Store::open(&root.join("guest"), "guest").unwrap();
            let host =
                o3_server::bridge::start(host_store.root(), state.clone(), NetOptions::loopback())
                    .await
                    .unwrap();
            let now = omabridge::unix_time().unwrap();
            let invite = host_store
                .create_invite(&["t3".into()], Duration::from_secs(60), now)
                .unwrap();
            let invite = omabridge::Invitation {
                version: 1,
                address: host.addr(),
                token: invite,
                name: "host".into(),
                expires: now + 60,
                manage: false,
            };
            let client = Arc::new(
                Client::new(guest_store.clone(), NetOptions::loopback())
                    .await
                    .unwrap(),
            );
            client.pair(&invite, "guest", now).await.unwrap();
            host_store.approve(&guest_store.id().unwrap(), now).unwrap();
            client.pair(&invite, "guest", now).await.unwrap();
            client.close().await;
            let guest_state = Arc::new(
                o3_server::store::State::open(&root.join("guest-profile/userdata")).unwrap(),
            );
            let guest =
                o3_server::bridge::start(guest_store.root(), guest_state.clone(), NetOptions::loopback())
                    .await
                    .unwrap();
            let pairing =
                omabridge::t3_pair_through_server(&guest_store, &host_store.id().unwrap())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(pairing.environment_id, state.environment_id);
            assert_eq!(
                pairing.label,
                format!("omabridge:{}", guest_store.id().unwrap())
            );
            let (forward, listen) = omabridge::forward_through_server(
                &guest_store,
                &host_store.id().unwrap(),
                "t3",
                "127.0.0.1:0".parse().unwrap(),
                Some(&host.addr()),
            )
            .await
            .unwrap()
            .unwrap();
            let base = format!("http://{listen}");
            let environment = state.environment_id.clone();
            tokio::task::spawn_blocking(move || {
                let options = t3_protocol::ConnectionOptions::default();
                let paired = t3_protocol::pair_bearer(
                    &base,
                    &pairing.credential,
                    None,
                    &t3_protocol::PairingClient::default(),
                    &options,
                )
                .unwrap();
                assert!(t3_protocol::pair_bearer(
                    &base,
                    &pairing.credential,
                    None,
                    &Default::default(),
                    &options
                )
                .is_err());
                let conn = paired.connection(&options).unwrap();
                let config = conn.rpc("server.getConfig", json!({})).unwrap();
                assert_eq!(config["environment"]["environmentId"], environment);
                drop(conn);
            })
            .await
            .unwrap();
            // Host-owned forward survives the desktop command and a full Node restart.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let listen = listener.local_addr().unwrap();
            drop(listener);
            let request = json!({"action":"save","peer":host_store.id().unwrap(),"service":"t3","listen":listen.to_string()});
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
                .args(["--state-dir", guest_store.root().to_str().unwrap(), "forwards"])
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
            let mut output = output;
            use std::io::Write;
            output.stdin.take().unwrap().write_all(request.to_string().as_bytes()).unwrap();
            assert!(output.wait().unwrap().success());
            guest.shutdown().await;
            let guest = o3_server::bridge::start(guest_store.root(), guest_state, NetOptions::loopback()).await.unwrap();
            let base = format!("http://{listen}");
            let environment = state.environment_id.clone();
            tokio::task::spawn_blocking(move || {
                let options = t3_protocol::ConnectionOptions::default().with_request_timeout(Duration::from_secs(2));
                let until = std::time::Instant::now() + Duration::from_secs(10);
                loop {
                    if t3_protocol::verify_environment(&base, &environment, &options).is_ok() { break; }
                    assert!(std::time::Instant::now() < until);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }).await.unwrap();
            omabridge::write_document(&guest_store.root().join("forwards.json"), &json!({}), true).unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(std::net::TcpStream::connect(listen).is_err());
            host_store
                .set_admin(&guest_store.id().unwrap(), true)
                .unwrap();
            // Exercise the same control JSON the desktop's CLI uses for mobile admin.
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            for enabled in [false, true] {
                let mut control =
                    tokio::net::UnixStream::connect(guest_store.root().join("control.sock"))
                        .await
                        .unwrap();
                let command = if enabled {
                    json!({"action":"publish","t3":true,"confirmed":true})
                } else {
                    json!({"action":"unpublish","confirmed":true})
                };
                control
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"peer":host_store.id().unwrap(),"command":command})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut answer = String::new();
                tokio::io::BufReader::new(control)
                    .read_line(&mut answer)
                    .await
                    .unwrap();
                let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
                assert!(answer["error"].is_null(), "{answer}");
                assert!(
                    answer["outcome"]["error"].is_null() || answer["outcome"]["error"] == "",
                    "{answer}"
                );
                assert_eq!(
                    answer["outcome"]["published"]["enabled"], enabled,
                    "{answer}"
                );
                if !enabled {
                    assert!(omabridge::t3_pair_through_server(
                        &guest_store,
                        &host_store.id().unwrap()
                    )
                    .await
                    .unwrap()
                    .is_err());
                }
            }
            // The host has no TCP target: every successful request above reached serve_io via duplex.
            assert!(host_store.root().join("control.sock").exists());
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(host_store.root().join("control.sock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            drop(forward);
            guest.shutdown().await;
            host.shutdown().await;
            drop(state);
            std::fs::remove_dir_all(root).unwrap();
        })
        .await
        .unwrap();
    });
}

#[test]
fn no_bridge_flag_serves_without_an_embedded_node() {
    let base = std::env::temp_dir().join(format!(
        "o3-no-bridge-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&base).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
        .args([
            "serve",
            "--base-dir",
            base.to_str().unwrap(),
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--no-bridge",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut line = String::new();
    std::io::BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let listening = line
        .trim()
        .strip_prefix("o3-server listening on ")
        .map(str::to_owned);
    let bridge_dir = base.join("omabridge").exists();
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_dir_all(&base).unwrap();
    assert!(listening.is_some(), "{line}");
    assert!(
        !bridge_dir,
        "--no-bridge must not create an embedded identity"
    );
}
