//! New SSH leaf tests remain isolated while runtime wiring is held for the parent checkpoint.
#[path = "support/sandbox.rs"]
pub mod test_sandbox;

#[test]
fn ssh_reference_requires_private_sandbox() {
    test_sandbox::assert_isolated();
}

#[test]
fn verified_installed_remote_lifecycle_is_private_and_non_destructive() {
    test_sandbox::assert_isolated();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("/usr/bin/node")
        .arg(root.join("tests/fixtures/device/verified-remote-proof.cjs"))
        .arg(root.join("src/device_native_remote.cjs"))
        .output()
        .expect("private test image contains Node");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let proof: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(proof["count"], 12);
}

#[path = "support/device_native_ssh_owner.rs"]
mod owner_tests;
#[path = "support/device_native_ssh.rs"]
mod plan_tests;

#[path = "support/device_native_ssh_fixture.rs"]
pub mod ssh_fixture;
