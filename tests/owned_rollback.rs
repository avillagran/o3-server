//! Failure rollback retains only fresh owned staging; existing destinations stay untouched.
#[path = "support/sandbox.rs"]
mod sandbox;
mod support;
use serde_json::json;
#[test]
fn clone_uses_fresh_staging_and_retains_failures_without_touching_existing_paths() {
    sandbox::assert_isolated();
    let server = support::start(|_| {});
    let client = server.connect();
    let root = server.dir.join("private-clone-fixture");
    std::fs::create_dir(&root).unwrap();
    let existing = root.join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("kept"), "untouched").unwrap();
    let request = |id: &str, remote: &std::path::Path, destination: &std::path::Path| json!({"projectId":id,"title":"Private clone","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":remote,"destinationPath":destination});
    let absent = root.join("absent-origin");
    assert!(client
        .rpc(
            "projectClone.start",
            request("existing", &absent, &existing)
        )
        .is_err());
    assert_eq!(
        std::fs::read_to_string(existing.join("kept")).unwrap(),
        "untouched"
    );
    let empty = root.join("existing-empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(client
        .rpc("projectClone.start", request("empty", &absent, &empty))
        .is_err());
    assert!(empty.is_dir());
    let failed = root.join("failed");
    assert!(client
        .rpc("projectClone.start", request("failed", &absent, &failed))
        .is_err());
    assert!(!failed.exists());
    assert!(
        std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".o3-retained-")),
        "Fresh failed staging must be retained"
    );
    let origin = root.join("origin.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(&origin)
        .status()
        .unwrap();
    assert!(status.success());
    let destination = root.join("published");
    client
        .rpc(
            "projectClone.start",
            request("success", &origin, &destination),
        )
        .unwrap();
    assert!(destination.join(".git").is_dir());
    assert!(origin.join("HEAD").is_file());
    assert_eq!(
        std::fs::read_to_string(existing.join("kept")).unwrap(),
        "untouched"
    );
}
