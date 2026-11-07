mod support;
#[path = "support/sandbox.rs"]
mod test_sandbox;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, time::Duration};
static ADB: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[test]
fn connected_adb_contract() {
    let _lock = ADB.lock().unwrap_or_else(|e| e.into_inner());
    let server = support::start(|_| {});
    let bin = server.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let adb = bin.join("adb");
    let log = server.dir.join("adb.log");
    std::fs::write(
        &adb,
        format!(
            r#"#!/bin/sh
echo "$*" >> '{}'
case "$*" in
"devices -l") printf 'List of devices attached
phone	device model:Pixel
adb-proof (2)._adb-tls-connect._tcp	device model:Wireless
locked unauthorized
';;
"-s adb-proof (2)._adb-tls-connect._tcp get-state") echo device;;
"-s phone get-state") echo device;;
"-s phone shell wm size") echo 'Physical size: 1080x1920';;
"-s phone exec-out screencap -p") printf '\211PNG\r\n\032\nFAKE';;
"-s phone shell input keyevent 3") ;;
"-s phone shell input tap 539 959") ;;
*) exit 1;;
esac
"#,
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), old.to_string_lossy()),
    );
    let c = server.connect();
    t3_protocol::api::device::configure(&c, Some(true), None, None).unwrap();
    let list = t3_protocol::api::device::list(&c, None, Some(true), None).unwrap();
    assert_eq!(list["devices"].as_array().unwrap().len(), 2);
    assert_eq!(list["devices"][0]["id"], "phone");
    assert_eq!(
        list["devices"][1]["id"],
        "adb-proof (2)._adb-tls-connect._tcp"
    );
    assert_eq!(
        c.rpc(
            "device.detail",
            json!({"deviceId":"adb-proof (2)._adb-tls-connect._tcp"})
        )
        .unwrap()["deviceId"],
        "adb-proof (2)._adb-tls-connect._tcp"
    );
    // The pinned lib's hub URL builder rejects spaces; exercise hub transport with its supported serial.
    let mut stream = support::subscribe(server.connect(), "subscribeDeviceState", json!({}));
    assert_eq!(stream.next(Duration::from_secs(15))["hostStatus"], "ready");
    t3_protocol::api::device::configure(&c, None, Some(true), Some(true)).unwrap();
    let session =
        t3_protocol::api::device::open(&c, "thread", "phone", "android", None, Some(false))
            .unwrap();
    assert_eq!(session["deviceId"], "phone");
    assert_eq!(
        t3_protocol::api::device::detail(&c, "phone", None).unwrap()["deviceId"],
        "phone"
    );
    let access = c.device_hub("/api/device-hub", "local").unwrap();
    let png = c
        .device_screenshot(&access, t3_protocol::device_hub::Platform::Android, "phone")
        .unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    let mut ws = c
        .device_socket(&access, t3_protocol::device_hub::Platform::Android, "phone")
        .unwrap();
    ws.send(
        t3_protocol::device_hub::input_message(
            t3_protocol::device_hub::Platform::Android,
            &json!({"type":"key","keycode":3}),
        )
        .unwrap(),
    )
    .unwrap();
    ws.close(None).unwrap();
    let grant = c
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let ro = support::pair(&server.base, grant["credential"].as_str().unwrap()).unwrap();
    let ro_access = ro.device_hub("/api/device-hub", "local").unwrap();
    assert!(ro
        .device_screenshot(
            &ro_access,
            t3_protocol::device_hub::Platform::Android,
            "phone"
        )
        .is_err());
    t3_protocol::api::device::configure(&c, Some(false), Some(false), None).unwrap();
    assert!(c
        .device_screenshot(&access, t3_protocol::device_hub::Platform::Android, "phone")
        .is_err());
    assert!(c
        .rpc(
            "device.action",
            json!({"deviceId":"phone","type":"launchApp","appId":"app.test"})
        )
        .is_err());
    t3_protocol::api::device::close(&c, "thread", None, None, Some(false)).unwrap();
    assert!(c.rpc("device.easAccount", json!({})).is_err());
    drop(stream);
    std::thread::sleep(Duration::from_millis(300));
    let before = std::fs::read(&log).unwrap();
    std::thread::sleep(Duration::from_millis(2300));
    assert_eq!(
        std::fs::read(&log).unwrap(),
        before,
        "poller must stop on unsubscribe"
    );
    assert!(String::from_utf8(before)
        .unwrap()
        .contains("shell input keyevent 3"));
    std::env::set_var("PATH", old);
}

#[test]
fn android_settings_actions_use_real_readers_and_fixture_adb() {
    test_sandbox::assert_isolated();
    #[cfg(target_os = "linux")]
    if run_adb_stack_child() {
        return;
    }
    let _lock = ADB.lock().unwrap_or_else(|e| e.into_inner());
    use t3_protocol::api::device::{self, Action, DeviceAction};
    #[cfg(target_os = "linux")]
    let (server, measured_stacks) = measured_adb_server();
    #[cfg(not(target_os = "linux"))]
    let server = support::start(|_| {});
    let bin = server.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let adb = bin.join("adb");
    let log = server.dir.join("actions.log");
    std::fs::write(&adb, format!(r#"#!/bin/sh
echo "$*" >> '{}'
case "$*" in
"-s fixture get-state") echo device;;
"-s fixture shell cmd uimode night") echo yes;;
"-s fixture shell settings get system font_scale") echo 1.15;;
"-s fixture shell settings get global animator_duration_scale") echo 0;;
"-s fixture shell settings get global wifi_on") echo 1;;
"-s fixture shell dumpsys window") echo 'mCurrentFocus=Window{{abc u0 test.app/.Main}}';;
"-s fixture shell cmd uimode night no"|"-s fixture shell settings put system font_scale 1.15"|"-s fixture shell svc wifi disable"|"-s fixture shell svc data disable"|"-s fixture shell settings put global animator_duration_scale 0"|"-s fixture shell settings put global transition_animation_scale 0"|"-s fixture shell settings put global window_animation_scale 0") ;;
*) exit 1;;
esac
"#,log.display())).unwrap();
    std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), old.to_string_lossy()),
    );
    let c = server.connect();
    t3_protocol::api::device::configure(&c, Some(true), None, None).unwrap();
    for action in [
        Action::SetAppearance("light".into()),
        Action::SetTextSize("large".into()),
        Action::SetToggle {
            setting: "networkEnabled".into(),
            value: false,
        },
        Action::SetToggle {
            setting: "reduceMotion".into(),
            value: true,
        },
    ] {
        let detail = device::action(
            &c,
            &DeviceAction {
                device_id: "fixture".into(),
                host_id: None,
                action,
            },
        )
        .unwrap();
        assert_eq!(
            detail["settings"],
            json!({"appearance":"dark","textSize":"large","reduceMotion":true,"networkEnabled":true})
        );
        assert_eq!(detail["foregroundApp"]["id"], "test.app");
    }
    let calls = std::fs::read_to_string(log).unwrap();
    assert!(calls.contains("shell svc data disable"));
    assert!(calls.contains("shell settings put global window_animation_scale 0"));
    #[cfg(target_os = "linux")]
    {
        let stacks = measured_stacks.lock().unwrap();
        assert!(
            stacks.len() >= 2,
            "Both production-sized workers must start"
        );
        assert!(stacks.iter().all(|size| *size == 512 * 1024));
        println!(
            "Private ADB actions and five settings readers completed on measured 512 KiB workers"
        );
    }
    std::env::set_var("PATH", old);
}

#[cfg(target_os = "linux")]
fn run_adb_stack_child() -> bool {
    const CHILD: &str = "O3_PRIVATE_ADB_STACK_CHILD";
    const CACHE_OFF: &str = "glibc.pthread.stack_cache_size=0";
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(std::env::var(CHILD).unwrap(), "1");
        assert_eq!(std::env::var("GLIBC_TUNABLES").unwrap(), CACHE_OFF);
        return false;
    }
    let executable = std::env::current_exe().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = runtime
        .block_on(o3_server::proc::run(
            executable.to_str().unwrap(),
            &std::env::current_dir().unwrap(),
            &[
                "--exact",
                "android_settings_actions_use_real_readers_and_fixture_adb",
                "--nocapture",
                "--test-threads=1",
            ],
            o3_server::proc::Opts {
                timeout: Duration::from_secs(90),
                cap: 256 * 1024,
                allow_truncate: false,
            },
            &[
                (CHILD, std::path::Path::new("1")),
                ("GLIBC_TUNABLES", std::path::Path::new(CACHE_OFF)),
            ],
        ))
        .unwrap();
    assert!(
        out.ok(),
        "Private ADB worker child failed: {}\n{}",
        out.text(),
        out.stderr
    );
    assert!(out
        .text()
        .contains("Private ADB worker stack: 524288 bytes"));
    assert!(out
        .text()
        .contains("Private ADB actions and five settings readers completed"));
    for line in out.text().lines() {
        if let Some(start) = line.find("Private ADB ") {
            println!("{}", &line[start..]);
        }
    }
    true
}
#[cfg(target_os = "linux")]
fn measured_adb_server() -> (
    support::Server,
    std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
) {
    use std::sync::{Arc, Mutex};
    let dir = std::env::temp_dir().join(format!(
        "o3-adb-worker-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = Arc::new(o3_server::store::State::open(&dir).unwrap());
    let secret = state
        .issue_pairing(Some("private stack fixture"), 60000)
        .unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let measurements = Arc::new(Mutex::new(Vec::new()));
    let measured = measurements.clone();
    let serving = state.clone();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .thread_stack_size(512 * 1024)
            .on_thread_start(move || {
                let size = unsafe {
                    let mut attr = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
                    assert_eq!(
                        libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()),
                        0
                    );
                    let mut attr = attr.assume_init();
                    let mut address = std::ptr::null_mut();
                    let mut size = 0;
                    assert_eq!(
                        libc::pthread_attr_getstack(&attr, &mut address, &mut size),
                        0
                    );
                    assert_eq!(libc::pthread_attr_destroy(&mut attr), 0);
                    size
                };
                assert_eq!(size, 512 * 1024, "actual ADB server worker stack");
                println!("Private ADB worker stack: {size} bytes");
                measured.lock().unwrap().push(size);
            })
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                o3_server::serve(
                    tokio::net::TcpListener::from_std(listener).unwrap(),
                    serving,
                )
                .await
                .unwrap();
            });
    });
    support::signals::shield_client_thread_from_sigchld();
    (
        support::Server {
            base,
            secret,
            state,
            dir,
        },
        measurements,
    )
}

#[test]
fn detail_answers_within_the_client_timeout_when_optional_queries_are_slow() {
    let _lock = ADB.lock().unwrap_or_else(|e| e.into_inner());
    let server = support::start(|_| {});
    let bin = server.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let adb = bin.join("adb");
    std::fs::write(
        &adb,
        "#!/bin/sh\ncase \"$*\" in\n\"devices -l\") printf 'List of devices attached\\nphone\tdevice model:Pixel\\n';;\n\"-s phone get-state\") echo device;;\n*) sleep 3; echo 1;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), old.to_string_lossy()),
    );
    let c = server.connect();
    t3_protocol::api::device::configure(&c, Some(true), None, None).unwrap();
    let started = std::time::Instant::now();
    let detail = t3_protocol::api::device::detail(&c, "phone", None);
    std::env::set_var("PATH", old);
    assert!(detail.is_ok(), "{detail:?}");
    assert!(started.elapsed() < Duration::from_secs(11));
}

#[test]
fn configure_writes_the_settings_clients_read_back() {
    let server = support::start(|_| {});
    let c = server.connect();
    t3_protocol::api::device::configure(&c, Some(true), Some(true), None).unwrap();
    let settings = c.rpc("server.getSettings", json!({})).unwrap();
    assert_eq!(settings["enableDeviceSupport"], true);
    assert_eq!(settings["enableAgentDeviceAccess"], true);
}
