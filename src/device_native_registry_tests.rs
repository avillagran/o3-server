//! Real device RPC and media consumers, using only a private SSH-shaped CLI and loopback hub.
use super::*;
#[path = "../tests/support/device_native_ssh_fixture.rs"]
mod ssh_fixture;
use ssh_fixture::Fixture;
async fn profile(fixture: &Fixture, enabled: bool) -> Arc<State> {
    let state =
        Arc::new(State::open(&fixture.executable.parent().unwrap().join("profile")).unwrap());
    state
        .streams
        .p2
        .device
        .register_ssh_executable_test(fixture.executable.clone());
    crate::settings::update(&state,&json!({"patch":{"enableDeviceSupport":enabled,"enableAgentDeviceAccess":true,"deviceHosts":[fixture.cli.config]}})).unwrap();
    state
}
#[tokio::test]
async fn registered_ssh_inventory_open_and_authenticated_media_use_only_owned_forward() {
    let (fixture, state) = prepare_ssh_flow().await;
    exercise_ssh_flow(fixture, state).await;
}
async fn prepare_ssh_flow() -> (Fixture, Arc<State>) {
    let fixture = Fixture::make(0).await;
    *fixture.devices.lock().unwrap() = json!({"simulators":[{"id":"sim-1","name":"Private SSH simulator","platform":"ios","version":"26","booted":true,"physical":false}],"emulators":[]});
    let state = profile(&fixture, false).await;
    (fixture, state)
}
async fn exercise_ssh_flow(fixture: Fixture, state: Arc<State>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut peer = RpcPeer::new(&state).await;
    println!("Private SSH consumer stage: disabled list");
    let disabled = rpc_call(&mut peer, &state, "device.list", &json!({}))
        .await
        .unwrap();
    assert_eq!(disabled["devices"], json!([]));
    assert!(
        !fixture.trace.exists(),
        "Disabled support must not probe or start SSH"
    );
    println!("Private SSH consumer stage: read-only host probe");
    let tested = rpc_call(
        &mut peer,
        &state,
        "device.testHost",
        &serde_json::to_value(&fixture.cli.config).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(tested["hubInstalled"], true);
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
    assert_eq!(
        crate::settings::descriptor(&state)["capabilities"]["devices"],
        true
    );
    println!("Private SSH consumer stage: configure and owned startup");
    let listed = rpc_call(
        &mut peer,
        &state,
        "device.configure",
        &json!({"enabled":true}),
    )
    .await
    .unwrap();
    assert_eq!(listed["hostStatuses"]["mini"]["status"], "ready");
    assert!(listed["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|device| device["hostId"] == "mini" && device["id"] == "sim-1"));
    let opened=rpc_call(&mut peer,&state,"device.open",&json!({"hostId":"mini","deviceId":"sim-1","platform":"ios","threadId":"private-ssh-thread"})).await.unwrap();
    assert_eq!(opened["hostId"], "mini");
    println!("Private SSH consumer stage: authenticated media");
    let token = state
        .issue_session(
            "private-peer",
            &["orchestration:operate".into()],
            60_000,
            "Private fixture",
        )
        .unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let serving = tokio::spawn(crate::http::serve_io(server, state.clone()));
    client.write_all(format!("POST /api/device-hub/vendor/serve-sim/api/screenshot?device=sim-1&hostId=mini&wsTicket=private-ticket HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    serving.await.unwrap().unwrap();
    let head = String::from_utf8_lossy(&response).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: image/png"));
    assert!(!head.contains("set-cookie"));
    assert!(response
        .windows(19)
        .any(|bytes| bytes == b"private-ssh-fixture"));
    assert!(hub_origin(&state, "http://unowned.invalid").await.is_err());
    println!("Private SSH consumer stage: close and disable");
    rpc_call(
        &mut peer,
        &state,
        "device.close",
        &json!({"hostId":"mini","deviceId":"sim-1","threadId":"private-ssh-thread"}),
    )
    .await
    .unwrap();
    let disabled = rpc_call(
        &mut peer,
        &state,
        "device.configure",
        &json!({"enabled":false}),
    )
    .await
    .unwrap();
    assert!(disabled["sessions"].as_array().unwrap().is_empty());
    assert!(fixture
        .trace()
        .iter()
        .any(|entry| entry["mode"] == "stop" && entry["receipt"] == "c".repeat(48)));
}
#[tokio::test]
async fn changed_or_removed_ssh_configuration_retains_owner_until_stop_is_confirmed() {
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, true).await;
    call(&state, "device.list", &json!({})).await.unwrap();
    fixture.settings(json!({"failStop":true}));
    let mut changed = serde_json::to_value(&fixture.cli.config).unwrap();
    changed["target"] = json!("new@private.invalid");
    crate::settings::update(&state, &json!({"patch":{"deviceHosts":[changed]}})).unwrap();
    assert!(call(&state, "device.list", &json!({})).await.is_err());
    assert_eq!(
        state.streams.p2.device.ssh.lock().await["mini"]
            .config
            .target,
        fixture.cli.config.target
    );
    assert!(hub_origin(&state, "mini").await.is_err());
    assert_eq!(
        retained_state(&state).await.unwrap()["hostStatuses"]["mini"]["status"],
        "failed"
    );
    assert_eq!(
        fixture
            .trace()
            .iter()
            .filter(|entry| entry["mode"] == "start")
            .count(),
        1
    );
    crate::settings::update(&state, &json!({"patch":{"deviceHosts":[]}})).unwrap();
    assert!(call(&state, "device.list", &json!({})).await.is_err());
    assert_eq!(state.streams.p2.device.ssh.lock().await.len(), 1);
    fixture.settings(json!({"failStop":false}));
    let listed = call(&state, "device.list", &json!({})).await.unwrap();
    assert!(state.streams.p2.device.ssh.lock().await.is_empty());
    assert!(listed["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|host| host["id"] != "mini"));
}
#[tokio::test]
async fn unverified_remote_tools_never_create_a_forward_or_session() {
    let fixture = Fixture::make(0).await;
    fixture.settings(json!({"verifiedInstalled":false}));
    fixture
        .healthy
        .store(false, std::sync::atomic::Ordering::Release);
    let state = profile(&fixture, true).await;
    let listed = call(&state, "device.list", &json!({})).await.unwrap();
    assert_eq!(listed["hostStatuses"]["mini"]["status"], "failed");
    assert!(!fixture
        .trace()
        .iter()
        .any(|entry| entry["mode"] == "start" || entry["mode"] == "forward"));
    assert!(call(
        &state,
        "device.open",
        &json!({"hostId":"mini","deviceId":"sim-1","platform":"ios","threadId":"private"})
    )
    .await
    .is_err());
    assert!(hub_origin(&state, "mini").await.is_err());
    call(&state, "device.configure", &json!({"enabled":false}))
        .await
        .unwrap();
    assert!(
        !fixture.trace().iter().any(|entry| entry["mode"] == "stop"),
        "No acquired receipt means no invented teardown request"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ssh_rpc_media_consumers_fit_production_512kib_worker_stack() {
    crate::test_sandbox::assert_isolated();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // Production opens its profile before starting server worker tasks.
        let (fixture, state) = prepare_ssh_flow().await;
        tokio::spawn(async move {
            let stack = unsafe {
                let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
                assert_eq!(
                    libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()),
                    0
                );
                let mut attributes = attributes.assume_init();
                let mut address = std::ptr::null_mut();
                let mut size = 0;
                assert_eq!(
                    libc::pthread_attr_getstack(&attributes, &mut address, &mut size),
                    0
                );
                libc::pthread_attr_destroy(&mut attributes);
                size
            };
            assert!(
                stack > 0 && stack <= 512 * 1024,
                "actual Tokio worker stack: {stack}"
            );
            println!("Private device consumer Tokio worker stack: {stack} bytes");
            exercise_ssh_flow(fixture, state).await;
        })
        .await
        .unwrap();
    });
}

#[tokio::test]
async fn inspect_only_probes_without_starting_reconciling_or_tearing_down_hosts() {
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, true).await;
    let inspected = call(&state, "device.list", &json!({"inspectOnly":true}))
        .await
        .unwrap();
    assert!(inspected["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|host| host["id"] == "mini" && host["hubInstalled"] == true));
    assert!(state.streams.p2.device.ssh.lock().await.is_empty());
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
    call(&state, "device.list", &json!({})).await.unwrap();
    let mut changed = serde_json::to_value(&fixture.cli.config).unwrap();
    changed["target"] = json!("new@private.invalid");
    crate::settings::update(&state, &json!({"patch":{"deviceHosts":[changed]}})).unwrap();
    let before = fixture.trace().len();
    let inspected = call(&state, "device.list", &json!({"inspectOnly":true}))
        .await
        .unwrap();
    assert_eq!(
        fixture.trace().len(),
        before,
        "Inspection must not stop or replace an owned connection"
    );
    assert!(inspected["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|host| host["id"] == "mini")
        .unwrap()["toolInspectionError"]
        .is_string());
    crate::settings::update(&state, &json!({"patch":{"deviceHosts":[]}})).unwrap();
    call(&state, "device.list", &json!({})).await.unwrap();
}

struct RpcPeer {
    socket: tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
    task: tokio::task::JoinHandle<()>,
    next: u64,
}
impl Drop for RpcPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl RpcPeer {
    async fn new(state: &Arc<State>) -> Self {
        let token = state
            .issue_session(
                "private-wire-peer",
                &crate::store::STANDARD_SCOPES.map(str::to_owned),
                60_000,
                "Private fixture",
            )
            .unwrap();
        let session = state.session(&token).unwrap();
        let (client, server) = tokio::io::duplex(256 * 1024);
        let server = tokio_tungstenite::WebSocketStream::from_raw_socket(
            server,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let task = tokio::spawn(crate::rpc::run(server, state.clone(), session));
        Self {
            socket,
            task,
            next: 0,
        }
    }
}
async fn rpc_call(peer: &mut RpcPeer, _state: &Arc<State>, tag: &str, payload: &Value) -> Reply {
    use futures_util::{SinkExt, StreamExt};
    peer.next += 1;
    let id = peer.next;
    peer.socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"_tag":"Request","id":id,"tag":tag,"payload":payload})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = peer
                .socket
                .next()
                .await
                .expect("private RPC remains connected")
                .unwrap();
            let Ok(text) = message.to_text() else {
                continue;
            };
            let value: Value = serde_json::from_str(text).unwrap();
            if value["_tag"] != "Exit" || value["requestId"] != id {
                continue;
            }
            return if value["exit"]["_tag"] == "Success" {
                Ok(value["exit"]["value"].clone())
            } else {
                Err(value["exit"]["cause"][0]["error"].clone())
            };
        }
    })
    .await
    .expect("private RPC completes within its budget")
}

#[tokio::test]
async fn verified_ssh_capability_survives_an_unavailable_local_toolchain() {
    struct UnsupportedLocal;
    impl crate::device_native_owner::HubDriver for UnsupportedLocal {
        fn spawn(
            &self,
        ) -> crate::device_native_owner::HostFuture<
            '_,
            Result<crate::device_native_owner::SpawnedHub, Value>,
        > {
            Box::pin(async { panic!("Unavailable local toolchain must never spawn") })
        }
    }
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, true).await;
    call(&state, "device.list", &json!({})).await.unwrap();
    let local = Arc::new(crate::device_native_runtime::NativeRuntime::new(
        crate::device_native_owner::LocalOwner::new(Arc::new(UnsupportedLocal)),
        json!({"id":"local","label":"This machine","kind":"local","platforms":[{"platform":"ios","available":false},{"platform":"android","available":false}],"hubInstalled":true,"agentDeviceInstalled":false}),
        state.streams.changed.clone(),
    ));
    state.streams.p2.device.register_native_test(local).await;
    assert_eq!(
        crate::settings::descriptor(&state)["capabilities"]["devices"],
        true
    );
    call(&state, "device.configure", &json!({"enabled":false}))
        .await
        .unwrap();
}
