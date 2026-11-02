mod support;
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
    let _lock = ADB.lock().unwrap_or_else(|e| e.into_inner());
    use t3_protocol::api::device::{self, Action, DeviceAction};
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
    std::env::set_var("PATH", old);
}
