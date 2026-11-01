#[test]
#[ignore = "installed Codex schema generator, no inference or authentication change"]
fn live_codex_app_server_schema_argv() {
    let dir = std::env::temp_dir().join(o3_server::store::random_id().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let cli = o3_server::executable::resolve(std::path::Path::new("codex")).unwrap();
    let result = std::process::Command::new(cli)
        .args(["app-server", "generate-json-schema", "--out"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(std::fs::read_dir(&dir).unwrap().next().is_some());
    std::fs::remove_dir_all(dir).unwrap();
}
