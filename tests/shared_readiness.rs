//! Legacy handover evidence without changing or migrating the private database.
use o3_server::shared_readiness::inspect;
use rusqlite::Connection;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    assert!(std::path::Path::new("/run/.containerenv").is_file());
    let path = std::env::temp_dir().join(format!(
        "o3-readiness-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
fn database(path: &std::path::Path) -> Connection {
    let db = Connection::open(path.join("o3.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE hook_queue(id TEXT PRIMARY KEY,task TEXT NOT NULL,coalesce TEXT NOT NULL,due INTEGER NOT NULL,expires INTEGER NOT NULL,prompt TEXT NOT NULL);CREATE TABLE hook_replay(task TEXT NOT NULL,key TEXT NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(task,key));").unwrap();
    db
}

#[test]
fn absent_database_is_ready_without_creating_any_files() {
    let path = fixture();
    let report = inspect(&path).unwrap();
    assert!(report.legacy_webhook_ready);
    assert_eq!(report.pending_legacy_deliveries, 0);
    assert_eq!(report.retained_legacy_replay_claims, 0);
    assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
}

#[test]
fn reads_are_non_mutating_and_every_retained_legacy_claim_blocks_handover() {
    let path = fixture();
    let db = database(&path);
    db.execute_batch("INSERT INTO hook_queue VALUES('old','task','',1,1,'Retained prepared prompt');INSERT INTO hook_replay VALUES('task','past-expiry',1),('task','permanent',9223372036854775807);").unwrap();
    drop(db);
    let before = std::fs::read(path.join("o3.sqlite")).unwrap();
    let report = inspect(&path).unwrap();
    assert!(!report.legacy_webhook_ready);
    assert_eq!(report.pending_legacy_deliveries, 1);
    assert_eq!(report.retained_legacy_replay_claims, 2);
    assert_eq!(std::fs::read(path.join("o3.sqlite")).unwrap(), before);
}

#[test]
fn draining_the_queue_does_not_discard_replay_suppression() {
    let path = fixture();
    let db = database(&path);
    db.execute_batch("INSERT INTO hook_replay VALUES('task','permanent',9223372036854775807);")
        .unwrap();
    let report = inspect(&path).unwrap();
    assert_eq!(report.pending_legacy_deliveries, 0);
    assert_eq!(report.retained_legacy_replay_claims, 1);
    assert!(!report.legacy_webhook_ready);
}

#[test]
fn live_wal_reads_observe_committed_state_without_replaying_uncommitted_work() {
    let path = fixture();
    let mut db = database(&path);
    db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;INSERT INTO hook_queue VALUES('first','task','',1,1,'First');INSERT INTO hook_replay VALUES('task','first',1);").unwrap();
    let writer = db.transaction().unwrap();
    writer.execute_batch("INSERT INTO hook_queue VALUES('second','task','',1,1,'Second');INSERT INTO hook_replay VALUES('task','second',1);").unwrap();
    let report = inspect(&path).unwrap();
    assert_eq!(
        (
            report.pending_legacy_deliveries,
            report.retained_legacy_replay_claims
        ),
        (1, 1)
    );
    writer.commit().unwrap();
    let report = inspect(&path).unwrap();
    assert_eq!(
        (
            report.pending_legacy_deliveries,
            report.retained_legacy_replay_claims
        ),
        (2, 2)
    );
}

#[test]
fn suspicious_or_unreadable_schema_never_reports_safe_handover() {
    let path = fixture();
    let db = Connection::open(path.join("o3.sqlite")).unwrap();
    db.execute_batch("CREATE VIEW hook_queue AS SELECT 1 AS id")
        .unwrap();
    assert!(inspect(&path)
        .unwrap_err()
        .contains("Unexpected legacy hook_queue object"));
    let corrupt = fixture();
    std::fs::write(corrupt.join("o3.sqlite"), b"Not a SQLite database").unwrap();
    assert!(inspect(&corrupt).is_err());
}

#[test]
fn missing_database_sidecars_and_file_links_are_retained_and_refused() {
    let path = fixture();
    let wal = path.join("o3.sqlite-wal");
    std::fs::write(&wal, b"Retained sidecar").unwrap();
    assert!(inspect(&path).unwrap_err().contains("sidecar remains"));
    assert_eq!(std::fs::read(&wal).unwrap(), b"Retained sidecar");
    let linked = fixture();
    let target = linked.join("fixture-target.sqlite");
    std::fs::write(&target, b"Retained target").unwrap();
    std::os::unix::fs::symlink(&target, linked.join("o3.sqlite")).unwrap();
    assert!(inspect(&linked).unwrap_err().contains("regular file"));
    assert_eq!(std::fs::read(target).unwrap(), b"Retained target");
}

#[test]
fn diagnostic_cli_uses_base_userdata_and_runs_before_foreign_owner_or_policy_startup() {
    let profile = fixture();
    let data = profile.join("userdata");
    std::fs::create_dir(&data).unwrap();
    let db = database(&data);
    db.execute_batch("INSERT INTO hook_replay VALUES('task','retained',1);")
        .unwrap();
    drop(db);
    std::fs::write(
        data.join("server-runtime.json"),
        serde_json::json!({"pid":std::process::id(),"serverVersion":"native-t3"}).to_string(),
    )
    .unwrap();
    let before = std::fs::read(data.join("o3.sqlite")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
        .args(["shared-profile-readiness", "--base-dir"])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report,
        serde_json::json!({"version":1,"legacyWebhookReady":false,"pendingLegacyDeliveries":0,"retainedLegacyReplayClaims":1})
    );
    assert_eq!(std::fs::read(data.join("o3.sqlite")).unwrap(), before);
    assert_eq!(std::fs::read_dir(data).unwrap().count(), 2);
    let invalid = std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
        .args(["shared-profile-readiness", "--base-dir"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
}
