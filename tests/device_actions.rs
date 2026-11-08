//! Pinned native action traces executed through the real RPC client and a private ADB peer.
mod support;
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    server: support::Server,
    client: t3_protocol::Connection,
    case: PathBuf,
    trace: PathBuf,
    previous_path: std::ffi::OsString,
}
impl Fixture {
    fn new() -> Self {
        let server = support::start(|_| {});
        let bin = server.dir.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let adb = bin.join("adb");
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device/adb-actions.py"),
            &adb,
        )
        .unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o700)).unwrap();
        let case = server.dir.join("case.json");
        let trace = server.dir.join("trace.jsonl");
        std::fs::write(&case, "{}").unwrap();
        let previous_path = std::env::var_os("PATH").unwrap();
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), previous_path.to_string_lossy()),
        );
        std::env::set_var("O3_DEVICE_ACTION_CASE", &case);
        std::env::set_var("O3_DEVICE_ACTION_TRACE", &trace);
        let client = server.connect();
        t3_protocol::api::device::configure(&client, Some(true), None, None).unwrap();
        Self {
            server,
            client,
            case,
            trace,
            previous_path,
        }
    }
    fn select(&self, case: &Value) {
        std::fs::write(&self.case, serde_json::to_vec(case).unwrap()).unwrap();
        std::fs::write(&self.trace, "").unwrap();
    }
    fn actions(&self) -> Vec<Value> {
        let reads = [
            json!(["get-state"]),
            json!(["shell", "cmd", "uimode", "night"]),
            json!(["shell", "settings", "get", "system", "font_scale"]),
            json!([
                "shell",
                "settings",
                "get",
                "global",
                "animator_duration_scale"
            ]),
            json!(["shell", "settings", "get", "global", "wifi_on"]),
            json!(["shell", "dumpsys", "window"]),
        ];
        std::fs::read_to_string(&self.trace)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|entry| {
                let args = entry["args"].as_array().unwrap();
                args.first() != Some(&json!("devices"))
                    && !reads
                        .iter()
                        .any(|read| read.as_array().unwrap() == &args[2..])
            })
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.previous_path);
        std::env::remove_var("O3_DEVICE_ACTION_CASE");
        std::env::remove_var("O3_DEVICE_ACTION_TRACE");
    }
}

#[test]
fn native_android_actions_and_detail_readings_match_source_over_rpc() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/device-actions.json")).unwrap();
    assert_eq!(
        fixture["revision"],
        "c55db3e9c72fdf584f4b54192fb39beb88667b59"
    );
    assert_eq!(fixture["actionCases"].as_array().unwrap().len(), 95);
    assert_eq!(fixture["detailCases"].as_array().unwrap().len(), 37);
    let f = Fixture::new();
    let baseline = &fixture["detailCases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["label"] == "font:1.1")
        .unwrap()["expected"];
    for case in fixture["actionCases"].as_array().unwrap() {
        f.select(case);
        let result = f.client.rpc("device.action", case["input"].clone());
        if case["expected"]["ok"] == true {
            let detail = result.unwrap_or_else(|error| panic!("{}: {error}", case["label"]));
            assert_eq!(
                detail["settings"], baseline["settings"],
                "{}",
                case["label"]
            );
            assert_eq!(
                detail["foregroundApp"], baseline["foregroundApp"],
                "{}",
                case["label"]
            );
        } else {
            let error = result.unwrap_err();
            assert!(
                error.contains(case["expected"]["error"]["message"].as_str().unwrap()),
                "{}: {error}",
                case["label"]
            );
        }
        assert_eq!(
            json!(f.actions()),
            case["expected"]["calls"],
            "{}",
            case["label"]
        );
    }
    for case in fixture["detailCases"].as_array().unwrap() {
        f.select(&json!({"spec":{"reads":case["reads"]}}));
        let detail = f
            .client
            .rpc("device.detail", json!({"deviceId":"emulator-5554"}))
            .unwrap();
        assert_eq!(
            detail["settings"], case["expected"]["settings"],
            "{}",
            case["label"]
        );
        assert_eq!(
            detail["foregroundApp"], case["expected"]["foregroundApp"],
            "{}",
            case["label"]
        );
    }
    let reader = f.server.limited(&f.client, &["orchestration:read"]);
    assert!(reader
        .rpc(
            "device.action",
            json!({"deviceId":"emulator-5554","type":"clearLocation"})
        )
        .unwrap_err()
        .contains("orchestration:operate"));
}
