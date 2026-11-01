mod support;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, time::Duration};
#[test]
fn connected_adb_contract() {
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
phone device model:Pixel
locked unauthorized
';;
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
    assert_eq!(list["devices"].as_array().unwrap().len(), 1);
    assert_eq!(list["devices"][0]["id"], "phone");
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
    t3_protocol::api::device::close(&c, "thread", None, None, Some(false)).unwrap();
    assert!(c.rpc("device.easAccount",json!({})).is_err());
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
