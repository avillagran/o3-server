//! Whole native API method observations, without running provider transports.
#[path = "../src/cliproxy_projection.rs"]
mod cliproxy_projection;
use serde_json::Value;
#[test]
fn actual_native_read_accounts_projection() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cliproxy-projection-4288.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let actual = cliproxy_projection::project(
            &case["account"],
            case.get("decodedUsage"),
            case["available"].as_array().map(Vec::as_slice),
            fixture["checkedAt"].as_str().unwrap(),
        );
        assert_eq!(actual, case["result"], "{}", case["label"]);
    }
}
