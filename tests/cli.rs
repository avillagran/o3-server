//! Subprocess tests use isolated profiles and never install a service.
use base64::Engine;
use serde_json::{json, Value};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct Profile(std::path::PathBuf);
impl Profile {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "o3-cli-{}-{}",
            std::process::id(),
            o3_server::store::random_id().unwrap()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_o3-server"));
        c.env("T3CODE_HOME", &self.0)
            .env("HOME", &self.0)
            .env_remove("T3CODE_PORT")
            .env_remove("T3CODE_HOST");
        c
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn cli_tokens_profile_runtime_helpers_and_graceful_shutdown() {
    let p = Profile::new();
    let out = p
        .cmd()
        .args([
            "auth",
            "session",
            "issue",
            "--scope",
            "settings:write",
            "--scope",
            "filesystem:read",
            "--ttl",
            "30d",
            "--label",
            "desktop",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let token = String::from_utf8(out.stdout).unwrap();
    assert!(!token.trim().contains(char::is_whitespace));
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.split('.').next().unwrap())
        .unwrap();
    let claims: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(
        claims["scopes"],
        json!(["settings:write", "filesystem:read"])
    );
    assert!(claims["exp"].as_i64().unwrap() > o3_server::store::now_ms() + 29 * 86400000);
    let mut child = p
        .cmd()
        .args(["serve", "--port", "0", "--host", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let runtime = p.0.join("userdata/server-runtime.json");
    let start = Instant::now();
    while !runtime.exists() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    let v: Value = serde_json::from_slice(&std::fs::read(&runtime).unwrap()).unwrap();
    assert_eq!(v["pid"], child.id());
    let port = v["port"].as_u64().unwrap().to_string();
    let helper = p
        .cmd()
        .args(["__ssh-helper", "runtime-port", runtime.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(String::from_utf8(helper.stdout)
        .unwrap()
        .trim()
        .ends_with(&format!(" {port}")));
    assert!(p
        .cmd()
        .args(["__ssh-helper", "wait-ready", &port, "2000", "10"])
        .status()
        .unwrap()
        .success());
    assert!(!p
        .cmd()
        .args(["serve", "--port", "0"])
        .output()
        .unwrap()
        .status
        .success());
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(!runtime.exists());
    assert!(p.0.join("userdata/o3.sqlite").exists());
    assert!(!p.0.join("userdata/statev2.sqlite").exists());
}
#[test]
fn live_pid_refusal_and_stale_recovery() {
    let p = Profile::new();
    let dir = p.0.join("userdata");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("server-runtime.json"),
        json!({"pid":std::process::id()}).to_string(),
    )
    .unwrap();
    assert!(!p
        .cmd()
        .args(["serve", "--port", "0"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::write(
        dir.join("server-runtime.json"),
        json!({"pid":u32::MAX}).to_string(),
    )
    .unwrap();
    assert!(o3_server::profile::refuse_live(&dir).is_ok());
}
#[test]
fn service_install_dry_run_is_reviewable_and_has_no_side_effects() {
    let p = Profile::new();
    let out = p
        .cmd()
        .args(["service", "install", "--dry-run"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let unit = String::from_utf8(out.stdout).unwrap();
    assert!(unit.contains("T3CODE_HOME="));
    assert!(unit.contains(" serve --base-dir "));
    assert!(!p.0.join(".config").exists());
}
#[test]
fn sqlite_and_receipts_survive_server_restart() {
    use t3_protocol::{Connection, Credential};
    let p = Profile::new();
    let out = p
        .cmd()
        .args([
            "auth",
            "session",
            "issue",
            "--scope",
            "orchestration:read",
            "--scope",
            "orchestration:operate",
            "--token-only",
        ])
        .output()
        .unwrap();
    let token = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    let command = json!({"type":"thread.create","commandId":"persist","threadId":"t1","projectId":"p1","title":"Persistent","modelSelection":{"instanceId":"codex","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null});
    let mut first = None;
    for _ in 0..2 {
        let mut child = p
            .cmd()
            .args(["serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let runtime = p.0.join("userdata/server-runtime.json");
        let start = Instant::now();
        while !runtime.exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let v: Value = serde_json::from_slice(&std::fs::read(&runtime).unwrap()).unwrap();
        let c = Connection::new(
            v["origin"].as_str().unwrap(),
            None,
            Credential::bearer(&token).unwrap(),
        )
        .unwrap();
        let result = c
            .rpc("orchestration.dispatchCommand", command.clone())
            .unwrap();
        if let Some(first) = first.as_ref() {
            assert_eq!(&result, first)
        } else {
            first = Some(result)
        }
        let shell = c
            .request_json("GET", "/api/orchestration/shell", None, true)
            .unwrap();
        assert_eq!(shell["threads"][0]["title"], "Persistent");
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(child.wait().unwrap().success());
    }
}
