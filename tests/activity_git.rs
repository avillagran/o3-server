mod support;
use serde_json::json;
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
fn git(root: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
#[test]
fn power_profile_suspends_and_resumes_actual_git_fetch() {
    let power = std::env::temp_dir().join(format!("o3-power-{}", std::process::id()));
    std::fs::create_dir_all(power.join("BAT0")).unwrap();
    for (k, v) in [
        ("type", "Battery"),
        ("status", "Discharging"),
        ("capacity", "15"),
    ] {
        std::fs::write(power.join("BAT0").join(k), v).unwrap();
    }
    std::env::set_var("O3_POWER_SUPPLY_DIRECTORY", &power);
    let s = support::start(|_| {});
    let c = s.connect();
    let _connected = support::keep_connected(s.connect());
    let origin = s.dir.join("origin");
    let seed = s.dir.join("seed");
    let repo = s.dir.join("repo");
    for p in [&origin, &seed, &repo] {
        std::fs::create_dir_all(p).unwrap();
    }
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(&seed, &["init", "-b", "main"]);
    git(&seed, &["config", "user.name", "Activity Test"]);
    git(&seed, &["config", "user.email", "activity@example.invalid"]);
    std::fs::write(seed.join("a"), "one").unwrap();
    git(&seed, &["add", "a"]);
    git(&seed, &["commit", "-m", "one"]);
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&seed, &["push", "origin", "main"]);
    git(&repo, &["clone", origin.to_str().unwrap(), "."]);
    let before = git(&repo, &["rev-parse", "origin/main"]);
    std::fs::write(seed.join("a"), "two").unwrap();
    git(&seed, &["commit", "-am", "two"]);
    git(&seed, &["push", "origin", "main"]);
    let expected = git(&seed, &["rev-parse", "HEAD"]);
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Fetch","workspaceRoot":repo})).unwrap();
    c.rpc("server.reportClientActivity",json!({"clientId":"fetch","clientKind":"web","visible":true,"focused":true,"recentlyInteracted":true,"observedAt":"2026-11-01T00:00:00Z","scopes":[{"type":"project","projectId":"p"}],"ttlMs":60000})).unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let patch = t3_protocol::native_settings::patch(
        &json!({"key":"backgroundActivityProfile","value":"battery-saver"}),
        Some(&config),
    )
    .unwrap();
    let saved = c.rpc("server.updateSettings", patch).unwrap();
    assert_eq!(saved["automaticGitFetchInterval"], 0);
    c.rpc("server.updateSettings",json!({"patch":{"backgroundActivity":{"schemaVersion":1,"profile":"custom","baseProfile":"balanced","overrides":{"automaticGitFetchInterval":1000,"hostPowerMonitorIdleInterval":1000,"hostPowerMonitorActiveInterval":1000,"pauseWhenOnBattery":true}}}})).unwrap();
    std::thread::sleep(Duration::from_millis(1400));
    assert_eq!(git(&repo, &["rev-parse", "origin/main"]), before);
    std::fs::write(power.join("BAT0/status"), "Charging").unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while git(&repo, &["rev-parse", "origin/main"]) != expected {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        git(&repo, &["rev-parse", "HEAD"]),
        before,
        "fetch must preserve the checkout"
    );
    let status = t3_protocol::api::vcs::refresh_status(&c, repo.to_str().unwrap()).unwrap();
    assert_eq!(status["behindCount"], 1);
    assert_eq!(status["aheadCount"], 0);
    let saved = c.rpc("server.getSettings", json!({})).unwrap();
    assert!(t3_protocol::native_settings::snapshot(&saved)["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["key"] == "backgroundActivityProfile"));
    // Fixture trees are retained for private container teardown.
}
