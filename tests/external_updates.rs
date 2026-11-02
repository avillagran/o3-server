mod support;
use serde_json::json;
#[test]
fn unknown_external_install_is_not_updatable() {
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(std::sync::Arc::new(
            o3_server::provider::ConfiguredProviders::new(
                std::sync::Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            ),
        ));
    });
    let c = server.connect();
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/input/instance.py");
    c.rpc("server.updateSettings",json!({"patch":{},"providerInstanceMutation":{"operation":"create","instanceId":"external","instance":{"driver":"claudeAgent","config":{"binaryPath":binary}}}})).unwrap();
    let config = t3_protocol::api::server::config(&c).unwrap();
    let rows = t3_protocol::provider_instances::projection(&config["settings"], &config, true);
    let row = rows["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "external")
        .unwrap();
    assert_eq!(
        row["canUpdate"], false,
        "unowned external executable must never self-update"
    );
    assert!(t3_protocol::api::provider::update(&c, "claudeAgent", None, Some("external")).is_err());
}

#[test]
fn npm_updates_only_the_original_user_prefix_and_verifies_version() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn script(path: &std::path::Path, body: &str) {
        std::fs::write(
            path,
            format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 10.1.0; exit; fi\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let root = std::env::temp_dir().join(format!("o3-external-npm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let package = root.join("lib/node_modules/@openai/codex");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"@openai/codex","version":"1.0.0"}"#,
    )
    .unwrap();
    let version = root.join("version");
    std::fs::write(&version, "codex-cli 1.0.0\n").unwrap();
    let binary = package.join("bin/codex");
    script(
        &binary,
        &format!(
            "if [ \"$1\" = --version ]; then /bin/cat '{}'; else exit 0; fi",
            version.display()
        ),
    );
    symlink(&binary, root.join("bin/codex")).unwrap();
    script(&root.join("bin/npm"),&format!("printf '%s\\n' \"$@\" > '{}/argv'; echo 'codex-cli 1.1.0' > '{}'; echo credential-must-never-be-published",root.display(),version.display()));
    std::env::set_var("PATH", root.join("bin"));
    let server = support::start(|s| {
        *s.provider.lock().unwrap() = Some(std::sync::Arc::new(
            o3_server::provider::ConfiguredProviders::new(
                std::sync::Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            ),
        ));
    });
    let c = server.connect();
    c.rpc("server.updateSettings",json!({"patch":{},"providerInstanceMutation":{"operation":"create","instanceId":"npmFixture","instance":{"driver":"codex","config":{"binaryPath":root.join("bin/codex")}}}})).unwrap();
    let before = t3_protocol::api::server::config(&c).unwrap();
    assert!(
        !root.join("argv").exists(),
        "config must not trigger installation"
    );
    let row = before["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["instanceId"] == "npmFixture")
        .unwrap();
    // A root-run test cannot establish a user-owned install and must refuse it.
    if unsafe { libc::geteuid() } == 0 {
        assert_eq!(row["versionAdvisory"]["canUpdate"], false);
        return;
    }
    assert_eq!(row["versionAdvisory"]["canUpdate"], true, "{row}");
    t3_protocol::api::provider::update(&c, "codex", None, Some("npmFixture")).unwrap();
    let argv = std::fs::read_to_string(root.join("argv")).unwrap();
    assert_eq!(argv,format!("install\n--global\n--prefix\n{}\n--ignore-scripts\n--no-audit\n--no-fund\n@openai/codex@latest\n",root.display()));
    let after = t3_protocol::api::server::config(&c).unwrap();
    let row = after["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["instanceId"] == "npmFixture")
        .unwrap();
    assert_eq!(row["updateState"]["status"], "succeeded");
    assert!(row["updateState"]["output"].is_null());
    assert!(!after
        .to_string()
        .contains("credential-must-never-be-published"));
    script(&root.join("bin/npm"), "exit 1");
    assert!(t3_protocol::api::provider::update(&c, "codex", None, Some("npmFixture")).is_err());
    script(&root.join("bin/npm"), "exec /bin/sleep 30");
    let updating = server.connect();
    let job = std::thread::spawn(move || {
        t3_protocol::api::provider::update(&updating, "codex", None, Some("npmFixture"))
    });
    let mut progress = support::subscribe(
        server.connect(),
        "provider.install.subscribe",
        json!({"instanceId":"npmFixture"}),
    );
    let updates = progress.until(std::time::Duration::from_secs(5), |s| {
        s["phase"] == "installing"
    });
    let operation = updates.last().unwrap()["operationId"].as_str().unwrap();
    t3_protocol::api::provider::install_cancel(&c, "npmFixture", operation).unwrap();
    assert!(
        job.join().unwrap().is_err(),
        "cancelled updater must not report success"
    );
    std::fs::remove_dir_all(root).unwrap();
}
