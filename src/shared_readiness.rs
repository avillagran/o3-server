//! Read-only legacy webhook handover checks. This does not certify the whole profile.
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{path::Path, time::Duration};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub version: u8,
    pub legacy_webhook_ready: bool,
    pub pending_legacy_deliveries: u64,
    pub retained_legacy_replay_claims: u64,
}
impl Report {
    fn counts(pending: u64, replay: u64) -> Self {
        Self {
            version: 1,
            legacy_webhook_ready: pending == 0 && replay == 0,
            pending_legacy_deliveries: pending,
            retained_legacy_replay_claims: replay,
        }
    }
}

fn count(db: &Connection, table: &str) -> Result<u64, String> {
    let kind: Option<String> = db
        .query_row(
            "SELECT type FROM sqlite_master WHERE name=?1",
            [table],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match kind.as_deref() {
        None => Ok(0),
        Some("table") => db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| error.to_string()),
        _ => Err(format!(
            "Unexpected legacy {table} object; handover cannot be verified."
        )),
    }
}

/// Never opens a missing private database with CREATE and never migrates or drains it.
/// Every retained replay row matters: the legacy engine never consulted its expires column.
pub fn inspect(data_dir: &Path) -> Result<Report, String> {
    let path = data_dir.join("o3.sqlite");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err("The private database must be a regular file.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for name in ["o3.sqlite-wal", "o3.sqlite-shm"] {
                match std::fs::symlink_metadata(data_dir.join(name)) {
                    Ok(_) => return Err("The private database is missing while a sidecar remains; preserve and repair the profile before handover.".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            return Ok(Report::counts(0, 0));
        }
        Err(error) => return Err(error.to_string()),
    }
    let mut db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| error.to_string())?;
    db.busy_timeout(Duration::from_millis(250))
        .map_err(|error| error.to_string())?;
    db.execute_batch("PRAGMA query_only=ON")
        .map_err(|error| error.to_string())?;
    let snapshot = db.transaction().map_err(|error| error.to_string())?;
    let result = Report::counts(
        count(&snapshot, "hook_queue")?,
        count(&snapshot, "hook_replay")?,
    );
    snapshot.commit().map_err(|error| error.to_string())?;
    Ok(result)
}

/// The local diagnostic runs before startup policy, profile ownership or migrations.
pub fn run_cli(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("shared-profile-readiness") {
        return None;
    }
    let report =
        if args.len() == 1 || args.len() == 3 && args[1] == "--base-dir" && !args[2].is_empty() {
            crate::profile::base_dir(args)
                .map_err(|error| error.to_string())
                .and_then(|base| inspect(&base.join("userdata")))
        } else {
            Err("usage: o3-server shared-profile-readiness [--base-dir PROFILE]".into())
        };
    match report {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("readiness report serializes")
            );
            Some(0)
        }
        Err(error) => {
            eprintln!("Legacy webhook handover cannot be verified: {error}");
            Some(2)
        }
    }
}
