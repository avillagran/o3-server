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
    const CHILD: &str = "O3_PRIVATE_DEVICE_STACK_CHILD";
    const CACHE_OFF: &str = "glibc.pthread.stack_cache_size=0";
    if std::env::var_os(CHILD).is_none() {
        // glibc may reuse a cached 2 MiB stack for a 512 KiB request. Its
        // tunable is read at process startup, so isolate this same test by exec.
        // The private-container assertion above applies before either branch.
        let executable = std::env::current_exe().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = runtime.block_on(crate::proc::run(
            executable.to_str().unwrap(),
            &std::env::current_dir().unwrap(),
            &[
                "--exact",
                "device::device_native_registry_tests::ssh_rpc_media_consumers_fit_production_512kib_worker_stack",
                "--nocapture",
                "--test-threads=1",
            ],
            crate::proc::Opts {
                timeout: std::time::Duration::from_secs(90),
                cap: 256 * 1024,
                allow_truncate: false,
            },
            &[
                (CHILD, std::path::Path::new("1")),
                ("GLIBC_TUNABLES", std::path::Path::new(CACHE_OFF)),
            ],
        )).unwrap();
        assert!(
            out.ok(),
            "Private stack child failed: {}\n{}",
            out.text(),
            out.stderr
        );
        assert!(
            out.text()
                .contains("Private device consumer Tokio worker stack: 524288 bytes"),
            "Child must execute and measure the real 512 KiB worker flow: {}",
            out.text()
        );
        // Keep the actual measurements and wire stages without printing a
        // second libtest summary that a full-suite reporter could double count.
        for line in out.text().lines() {
            if let Some(start) = line.find("Private ") {
                println!("{}", &line[start..]);
            }
        }
        return;
    }
    assert_eq!(std::env::var(CHILD).unwrap(), "1");
    assert_eq!(std::env::var("GLIBC_TUNABLES").unwrap(), CACHE_OFF);
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
        Self::with_scopes(state, &crate::store::STANDARD_SCOPES.map(str::to_owned)).await
    }
    async fn over_http(state: &Arc<State>, scopes: &[String]) -> Self {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let token = state
            .issue_session("private-http-wire-peer", scopes, 60_000, "Private fixture")
            .unwrap();
        let (client, server) = tokio::io::duplex(256 * 1024);
        let serving = state.clone();
        let task = tokio::spawn(async move {
            crate::http::serve_io(server, serving).await.unwrap();
        });
        let mut request = "ws://localhost/ws?orchestrationProtocol=2"
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (socket, _) = tokio_tungstenite::client_async(request, client)
            .await
            .unwrap();
        Self {
            socket,
            task,
            next: 0,
        }
    }
    async fn with_scopes(state: &Arc<State>, scopes: &[String]) -> Self {
        let token = state
            .issue_session("private-wire-peer", scopes, 60_000, "Private fixture")
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

#[tokio::test]
async fn manual_pinned_updates_preserve_permissions_and_do_not_start_helpers() {
    let (fixture, state) = prepare_ssh_flow().await;
    let (_, npm) = crate::device_native_acquire::tests::fixture("ok");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(npm);
    let mut peer = RpcPeer::new(&state).await;
    let before = crate::settings::read(&state).unwrap();
    let installed = rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"updateTool":"agent","inspectOnly":true}),
    )
    .await
    .unwrap();
    assert_eq!(installed["hosts"][0]["agentDeviceInstalled"], true);
    assert_eq!(
        installed["hosts"][0]["tools"]["agent"],
        json!({"requiredVersion":"0.21.31","installedVersions":["0.21.31"],"runningVersion":null})
    );
    assert_eq!(
        installed["hosts"][0]["tools"]["hub"]["runningVersion"],
        Value::Null
    );
    assert_eq!(crate::settings::read(&state).unwrap(), before);
    assert!(
        fixture.trace().iter().all(|entry| entry["mode"] == "probe"),
        "Read-only post-update inspection cannot start configured SSH helpers"
    );
    // The independent agent tree is reused; failure to fetch the hub leaves it untouched.
    let installed = state.asset_profile_root.join("tools/agent-device/0.21.31");
    let (_, failed_npm) = crate::device_native_acquire::tests::fixture("failure");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(failed_npm);
    let failed = rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"updateTool":"hub"}),
    )
    .await
    .unwrap_err();
    assert_eq!(failed["_tag"], "DeviceToolchainInstallError");
    crate::device_native_tools::InstalledTool::inspect(
        &installed,
        crate::device_native_tools::Tool::Agent,
    )
    .unwrap();
    assert_eq!(crate::settings::read(&state).unwrap(), before);
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
    let (_, retry_npm) = crate::device_native_acquire::tests::fixture("ok");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(retry_npm);
    rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"updateTool":"hub"}),
    )
    .await
    .unwrap();
    let root = state
        .asset_profile_root
        .join("tools/expo-device-hub/0.19.1");
    crate::device_native_tools::InstalledTool::inspect(
        &root,
        crate::device_native_tools::Tool::Hub,
    )
    .unwrap();
    assert_eq!(crate::settings::read(&state).unwrap(), before);
    assert!(
        state
            .streams
            .p2
            .device
            .native
            .lock()
            .await
            .as_ref()
            .unwrap()
            .origin()
            .is_none(),
        "Read-only update inspection cannot start the newly acquired hub"
    );
    assert!(!fixture.trace().iter().any(|entry| entry["mode"] == "start"));
    peer.task.abort();
}

#[tokio::test]
async fn failed_manual_update_preserves_working_remote_forward_inventory_and_sessions() {
    let fixture = Fixture::make(0).await;
    *fixture.devices.lock().unwrap() = json!({"simulators":[{"id":"sim-1","name":"Private simulator","platform":"ios","version":"26","booted":true,"physical":false}],"emulators":[]});
    let state = profile(&fixture, true).await;
    call(&state, "device.list", &json!({})).await.unwrap();
    call(
        &state,
        "device.open",
        &json!({"hostId":"mini","deviceId":"sim-1","platform":"ios","threadId":"private-update-thread"}),
    )
    .await
    .unwrap();
    let runtime = state
        .streams
        .p2
        .device
        .ssh
        .lock()
        .await
        .get("mini")
        .unwrap()
        .runtime
        .clone();
    let origin = runtime.origin().unwrap();
    let before = runtime.snapshot();
    let (_, npm) = crate::device_native_acquire::tests::fixture("failure");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(npm);
    assert!(call(&state, "device.list", &json!({"updateTool":"hub"}))
        .await
        .is_err());
    assert_eq!(runtime.origin(), Some(origin));
    assert_eq!(runtime.snapshot(), before);
    call(&state, "device.configure", &json!({"enabled":false}))
        .await
        .unwrap();
}

#[tokio::test]
async fn enabled_local_readiness_acquires_verified_payload_before_owning_a_synthetic_hub() {
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, false).await;
    let (_, npm) = crate::device_native_acquire::tests::fixture("hub-server");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(npm);
    let sdk_root = state.dir.join("private-absent-sdk");
    *state.streams.p2.device.test_local_tools.lock().unwrap() =
        Some(Arc::new(crate::device_native_platform::LocalTools {
            sdk: crate::device_native_platform::AndroidSdk {
                root: Some(sdk_root),
                adb: true,
                emulator: true,
                avdmanager: true,
                legacy_avdmanager: false,
            },
            platform: "linux".into(),
            environment: BTreeMap::new(),
            cwd: state.dir.clone(),
        }));
    // The SDK is a discovery-only test value; no executable exists or is run there.
    assert!(!state.dir.join("private-absent-sdk").exists());
    let before = call(&state, "device.list", &json!({})).await.unwrap();
    assert_eq!(before["hostStatus"], "disabled");
    assert!(!state
        .asset_profile_root
        .join("tools/expo-device-hub/0.19.1")
        .exists());
    call(&state, "device.configure", &json!({"enabled":true}))
        .await
        .unwrap();
    let runtime = state
        .streams
        .p2
        .device
        .native
        .lock()
        .await
        .as_ref()
        .unwrap()
        .clone();
    let origin = runtime
        .origin()
        .expect("The verified synthetic Node hub has passed readiness");
    let inspected = call(&state, "device.list", &json!({"inspectOnly":true}))
        .await
        .unwrap();
    let local = inspected["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|host| host["id"] == "local")
        .unwrap();
    assert_eq!(
        local["tools"]["hub"],
        json!({"requiredVersion":"0.19.1","installedVersions":["0.19.1"],"runningVersion":"0.19.1"})
    );
    assert_eq!(local["tools"]["agent"]["runningVersion"], Value::Null);
    assert!(origin.starts_with("http://127.0.0.1:"));
    crate::device_native_tools::InstalledTool::inspect(
        &state
            .asset_profile_root
            .join("tools/expo-device-hub/0.19.1"),
        crate::device_native_tools::Tool::Hub,
    )
    .unwrap();
    assert!(!state.dir.join("private-absent-sdk").exists());
    call(&state, "device.configure", &json!({"enabled":false}))
        .await
        .unwrap();
    assert!(runtime.origin().is_none());
    let stopped = call(&state, "device.list", &json!({"inspectOnly":true}))
        .await
        .unwrap();
    assert_eq!(
        stopped["hosts"][0]["tools"]["hub"]["runningVersion"],
        Value::Null
    );
}

#[tokio::test]
async fn local_inventory_rpc_qualifies_unknown_trees_and_retains_last_verified_tools_on_failure() {
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, false).await;
    let (_, npm) = crate::device_native_acquire::tests::fixture("ok");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(npm);
    let mut peer = RpcPeer::new(&state).await;
    let initial = rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"updateTool":"agent"}),
    )
    .await
    .unwrap();
    let known = initial["hosts"][0]["tools"].clone();
    assert_eq!(known["agent"]["installedVersions"], json!(["0.21.31"]));
    let unknown = state.dir.join("tools/expo-device-hub/9.9.9");
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join(".install-complete"), "9.9.9\n").unwrap();
    std::fs::write(unknown.join("kept"), "unverified data").unwrap();
    let qualified = rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"inspectOnly":true}),
    )
    .await
    .unwrap();
    assert!(qualified["hosts"][0]["toolInspectionError"].is_string());
    assert_eq!(
        qualified["hosts"][0]["tools"]["hub"]["installedVersions"],
        json!([])
    );
    assert_eq!(qualified["hosts"][0]["hubInstalled"], false);
    assert!(!unknown.join(".o3-device-install.json").exists());
    let tools = state.dir.join("tools");
    let retained = state.dir.join("retained-owned-tools");
    std::fs::rename(&tools, &retained).unwrap();
    let target = state.dir.join("private-empty-target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &tools).unwrap();
    let failed = rpc_call(
        &mut peer,
        &state,
        "device.list",
        &json!({"inspectOnly":true}),
    )
    .await
    .unwrap();
    assert!(failed["hosts"][0]["toolInspectionError"].is_string());
    assert_eq!(failed["hosts"][0]["tools"], known);
    assert_eq!(failed["hosts"][0]["agentDeviceInstalled"], true);
    assert_eq!(
        failed["hosts"][0]["tools"]["agent"]["runningVersion"],
        Value::Null
    );
    assert_eq!(
        std::fs::read_to_string(retained.join("expo-device-hub/9.9.9/kept")).unwrap(),
        "unverified data"
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
    assert!(
        state.streams.p2.device.native.lock().await.is_none(),
        "Read-only tools inspection must not invent a local owner"
    );
    peer.task.abort();
}

#[tokio::test]
async fn real_wire_device_list_permissions_follow_the_effective_dispatch_action() {
    let fixture = Fixture::make(0).await;
    let state = profile(&fixture, false).await;
    let (_, npm) = crate::device_native_acquire::tests::fixture("ok");
    *state.streams.p2.device.test_npm_executable.lock().unwrap() = Some(npm);
    let mut reader = RpcPeer::over_http(&state, &["orchestration:read".into()]).await;
    rpc_call(&mut reader, &state, "device.list", &json!({}))
        .await
        .unwrap();
    let registered = state.streams.p2.device.ssh.lock().await.len();
    rpc_call(
        &mut reader,
        &state,
        "device.list",
        &json!({"inspectOnly":true,"retryHostId":"mini"}),
    )
    .await
    .unwrap();
    assert_eq!(state.streams.p2.device.ssh.lock().await.len(), registered);
    assert!(state
        .streams
        .p2
        .device
        .ssh
        .lock()
        .await
        .values()
        .all(|entry| entry.runtime.origin().is_none()));
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
    for payload in [
        json!({"updateTool":"agent"}),
        json!({"updateTool":"hub","inspectOnly":true,"retryHostId":"mini"}),
    ] {
        let denied = rpc_call(&mut reader, &state, "device.list", &payload)
            .await
            .unwrap_err();
        assert_eq!(denied["_tag"], "EnvironmentAuthorizationError");
        assert_eq!(denied["requiredPermission"], "orchestration:operate");
    }
    let denied = rpc_call(
        &mut reader,
        &state,
        "device.list",
        &json!({"retryHostId":"mini"}),
    )
    .await
    .unwrap_err();
    assert_eq!(denied["requiredPermission"], "orchestration:operate");
    assert!(
        !state.dir.join("tools").exists(),
        "Denied calls must not create installation state"
    );
    let invalid = rpc_call(
        &mut reader,
        &state,
        "device.list",
        &json!({"updateTool":null,"inspectOnly":true}),
    )
    .await
    .unwrap_err();
    assert!(invalid.is_null() || invalid["_tag"] == "RpcInvalidPayload");
    let invalid = rpc_call(
        &mut reader,
        &state,
        "device.list",
        &json!({"retryHostId":null}),
    )
    .await
    .unwrap_err();
    assert!(invalid.is_null() || invalid["_tag"] == "RpcInvalidPayload");
    assert!(!state.dir.join("tools").exists());
    let mut operate_only = RpcPeer::over_http(&state, &["orchestration:operate".into()]).await;
    let denied = rpc_call(
        &mut operate_only,
        &state,
        "device.list",
        &json!({"updateTool":"agent"}),
    )
    .await
    .unwrap_err();
    assert_eq!(denied["requiredPermission"], "orchestration:read");
    let mut operator = RpcPeer::over_http(
        &state,
        &["orchestration:read".into(), "orchestration:operate".into()],
    )
    .await;
    let updated = rpc_call(
        &mut operator,
        &state,
        "device.list",
        &json!({"updateTool":"agent","inspectOnly":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        updated["hosts"][0]["tools"]["agent"]["installedVersions"],
        json!(["0.21.31"])
    );
    assert_eq!(state.streams.p2.device.ssh.lock().await.len(), registered);
    assert!(state
        .streams
        .p2
        .device
        .ssh
        .lock()
        .await
        .values()
        .all(|entry| entry.runtime.origin().is_none()));
    assert!(state.streams.p2.device.native.lock().await.is_none());
    assert!(fixture.trace().iter().all(|entry| entry["mode"] == "probe"));
}
