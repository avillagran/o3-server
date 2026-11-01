//! `tools/pilot` against a throwaway T3 profile: import, start, pair, refuse overwrite, stop.
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("o3-pilot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn pilot(dir: &PathBuf, t3: &PathBuf, extra: &[&str]) -> std::process::Output {
    Command::new("python3")
        .arg("-I")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tools/pilot"))
        .args(["--bin", env!("CARGO_BIN_EXE_o3-server"), "--t3-home"])
        .arg(t3)
        .args(extra)
        .env("O3_PILOT_DIR", dir)
        .env("HOME", dir.parent().unwrap())
        .env_remove("T3CODE_HOME")
        .output()
        .unwrap()
}
fn alive(pid: u64) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| !s.rsplit(") ").next().unwrap_or("").starts_with('Z'))
}

#[test]
fn pilot_imports_read_only_starts_pairs_refuses_overwrite_and_stops() {
    let root = scratch("run");
    let t3 = root.join("t3");
    std::fs::create_dir_all(t3.join("userdata")).unwrap();
    let source = t3.join("userdata/statev2.sqlite");
    let db = rusqlite::Connection::open(&source).unwrap();
    db.execute_batch(include_str!("fixtures/t3-import.sql")).unwrap();
    db.execute(
        "INSERT INTO projection_projects VALUES('p','Project','/var/tmp','','[]','now','now',NULL)",
        [],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&source).unwrap();
    let dir = root.join("pilot");

    let out = pilot(&dir, &t3, &[]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let state: Value = serde_json::from_slice(&std::fs::read(dir.join("pilot.json")).unwrap()).unwrap_or(json!({}));
    let pid = state["pid"].as_u64();
    // Always stop what we started, whatever the asserts below say.
    let _guard = Stop(dir.clone(), t3.clone());
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("imported T3 history"), "{stdout}");
    assert!(stdout.contains("/pair#token="), "{stdout}");
    let pid = pid.unwrap();
    assert!(alive(pid));
    assert_eq!(std::fs::read(&source).unwrap(), before, "T3 database must not change");
    assert!(!t3.join("userdata/o3.sqlite").exists());
    assert!(dir.join("t3home/userdata/o3.sqlite").exists());

    // The printed origin answers on its own port, and the link pairs a real client.
    let origin = state["origin"].as_str().unwrap();
    let link = stdout.lines().find(|l| l.contains("/pair#token=")).unwrap().trim();
    let token = link.split("#token=").nth(1).unwrap();
    let conn = t3_protocol::pair_bearer(
        origin,
        token,
        None,
        &t3_protocol::PairingClient::default(),
        &t3_protocol::ConnectionOptions::default(),
    )
    .unwrap()
    .connection(&t3_protocol::ConnectionOptions::default())
    .unwrap();
    let config = conn.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(config["environment"]["label"], "o3-server");

    let again = pilot(&dir, &t3, &[]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("refusing to overwrite"));
    assert!(alive(pid));

    let stop = pilot(&dir, &t3, &["--stop"]);
    assert!(stop.status.success(), "{}", String::from_utf8_lossy(&stop.stderr));
    assert!(!alive(pid));
    assert!(dir.join("t3home/userdata/o3.sqlite").exists(), "stop keeps the dir");

    let resumed = pilot(&dir, &t3, &["--resume"]);
    assert!(resumed.status.success(), "{}", String::from_utf8_lossy(&resumed.stderr));
    assert!(!String::from_utf8_lossy(&resumed.stdout).contains("imported"));
    drop(_guard);
    let state: Value = serde_json::from_slice(&std::fs::read(dir.join("pilot.json")).unwrap()).unwrap();
    assert!(!alive(state["pid"].as_u64().unwrap()));
    std::fs::remove_dir_all(root).unwrap();
}

struct Stop(PathBuf, PathBuf);
impl Drop for Stop {
    fn drop(&mut self) {
        let _ = pilot(&self.0, &self.1, &["--stop"]);
    }
}

#[test]
fn pilot_refuses_a_dir_inside_the_t3_profile_and_never_creates_it() {
    let root = scratch("nest");
    let t3 = root.join("t3");
    std::fs::create_dir_all(&t3).unwrap();
    let dir = t3.join("pilot");
    let out = pilot(&dir, &t3, &["--no-import"]);
    assert!(!out.status.success());
    assert!(!dir.exists());
    std::fs::remove_dir_all(root).unwrap();
}
