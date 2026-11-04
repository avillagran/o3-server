//! o3-server's own SQLite state. T3's database is never written: its schema changes every nightly.
use crate::auth;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const STANDARD_SCOPES: [&str; 16] = [
    "orchestration:read",
    "orchestration:operate",
    "settings:write",
    "providers:manage",
    "environment:maintain",
    "preview:operate",
    "diagnostics:read",
    "terminal:read",
    "terminal:operate",
    "source-control:write",
    "filesystem:read",
    "filesystem:write",
    "relay:read",
    "access:read",
    "access:write",
    "relay:write",
];

pub struct State {
    pub(crate) rollback_pending: std::sync::atomic::AtomicBool,
    pub(crate) http: crate::http::Limits,
    pub(crate) mcp: crate::mcp::Sessions,
    #[cfg(feature = "bridge")]
    pub bridge_endpoints: Mutex<Vec<Value>>,
    pub db: Mutex<Connection>,
    pub(crate) worktree_gate: std::sync::RwLock<()>,
    pub(crate) reader: Mutex<Connection>,
    pub(crate) identities_started: std::sync::atomic::AtomicBool,
    pub(crate) rpc_clients: std::sync::atomic::AtomicUsize,
    checkpoint_started: std::sync::atomic::AtomicBool,
    checkpoint_wake: std::sync::Arc<(Mutex<bool>, std::sync::Condvar)>,
    pub signing_key: Vec<u8>,
    pub environment_id: String,
    pub label: String,
    pub dir: PathBuf,
    pub asset_profile_root: PathBuf,
    /// How long a finished run's live requests stay answerable, like T3's session idle window.
    pub request_release_ms: std::sync::atomic::AtomicU64,
    pub(crate) settings_cache: Mutex<crate::settings::Cache>,
    pub provider: Mutex<Option<std::sync::Arc<dyn crate::provider::Provider>>>,
    pub active: Mutex<
        std::collections::BTreeMap<
            String,
            tokio::sync::mpsc::Sender<crate::provider::ProviderControl>,
        >,
    >,
    pub settings_lock: Mutex<()>,
    pub launch_lock: Mutex<()>,
    pub changed: tokio::sync::watch::Sender<u64>,
    pub activity: tokio::sync::watch::Sender<u64>,
    /// Live terminal and command-output sources, with their own wake-up channel.
    pub streams: crate::streams::Hub,
}

pub fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

pub fn iso(ms: i64) -> String {
    let at = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .expect("timestamp in range");
    let format = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    );
    at.format(&format).expect("fixed format")
}

pub fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

const SCHEMA: &str = "
PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON; PRAGMA journal_size_limit=33554432; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0; PRAGMA cache_size=-512; PRAGMA mmap_size=0;
CREATE TABLE IF NOT EXISTS auth_sessions(
  session_id TEXT PRIMARY KEY, subject TEXT NOT NULL, scopes TEXT NOT NULL, method TEXT NOT NULL,
  client TEXT NOT NULL, issued_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS auth_pairing_links(
  id TEXT PRIMARY KEY, credential TEXT NOT NULL UNIQUE, scopes TEXT NOT NULL, subject TEXT NOT NULL,
  label TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, consumed_at INTEGER, revoked_at INTEGER);
";

/// `sql-profile` builds: attach a statement profiler to a connection. No-op otherwise.
#[allow(unused_variables)]
pub fn profiled(conn: &mut Connection) {
    #[cfg(feature = "sql-profile")]
    sql_profile::attach(conn);
}
#[cfg(feature = "sql-profile")]
mod sql_profile {
    use std::{collections::HashMap, sync::Mutex, time::Duration};
    static STATS: Mutex<Option<HashMap<String, (u64, Duration)>>> = Mutex::new(None);
    static STARTED: std::sync::Once = std::sync::Once::new();
    fn record(sql: &str, took: Duration) {
        let key: String = sql
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        let mut g = STATS.lock().unwrap();
        let e = g.get_or_insert_with(HashMap::new).entry(key).or_default();
        e.0 += 1;
        e.1 += took;
    }
    pub fn attach(conn: &mut rusqlite::Connection) {
        #[allow(deprecated)]
        conn.profile(Some(record));
        STARTED.call_once(|| {
            std::thread::spawn(|| loop {
                std::thread::sleep(Duration::from_secs(60));
                let Some(stats) = STATS.lock().unwrap().take() else {
                    continue;
                };
                let mut rows: Vec<_> = stats.into_iter().collect();
                rows.sort_by_key(|row| std::cmp::Reverse(row.1 .1));
                eprintln!("sql-profile: {} statement shapes in 60 s", rows.len());
                for (sql, (n, took)) in rows.into_iter().take(15) {
                    eprintln!(
                        "sql-profile: {n:>7} calls {:>9.1} ms  {sql}",
                        took.as_secs_f64() * 1000.0
                    );
                }
            });
        });
    }
}

impl State {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut db = Connection::open(dir.join("o3.sqlite")).map_err(|e| e.to_string())?;
        profiled(&mut db);
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        // A deferred read-to-write upgrade fails instantly, ignoring busy_timeout, when the
        // checkpointer connection has moved the WAL. Taking the write lock up front waits.
        db.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        let reader = Connection::open_with_flags(
            dir.join("o3.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        reader
            .execute_batch("PRAGMA cache_size=-512; PRAGMA mmap_size=0;")
            .map_err(|e| e.to_string())?;
        let mut reader = reader;
        profiled(&mut reader);
        let signing_key = auth::load_or_create_key(&dir.join("secrets/server-signing-key.bin"))
            .map_err(|e| e.to_string())?;
        let id_path = dir.join("environment-id");
        let environment_id = match std::fs::read_to_string(&id_path) {
            Ok(id) => id.trim().to_owned(),
            Err(_) => {
                let id = random_id()?;
                std::fs::write(&id_path, &id).map_err(|e| e.to_string())?;
                id
            }
        };
        let label = std::env::var("O3_LABEL").unwrap_or_else(|_| "o3-server".into());
        let (changed, _) = tokio::sync::watch::channel(0);
        crate::events::initialize(&db)?;
        let streams = crate::streams::Hub::new();
        streams.terminals.restore(&dir.join("terminal-history"))?;
        Ok(Self {
            rollback_pending: std::sync::atomic::AtomicBool::new(false),
            http: crate::http::Limits::default(),
            mcp: crate::mcp::Sessions::default(),
            #[cfg(feature = "bridge")]
            bridge_endpoints: Mutex::new(Vec::new()),
            settings_lock: Mutex::new(()),
            launch_lock: Mutex::new(()),
            provider: Mutex::new(None),
            active: Mutex::new(std::collections::BTreeMap::new()),
            changed,
            activity: tokio::sync::watch::channel(0).0,
            streams,
            reader: Mutex::new(reader),
            identities_started: std::sync::atomic::AtomicBool::new(false),
            rpc_clients: std::sync::atomic::AtomicUsize::new(0),
            checkpoint_started: std::sync::atomic::AtomicBool::new(false),
            checkpoint_wake: std::sync::Arc::new((Mutex::new(true), std::sync::Condvar::new())),
            db: Mutex::new(db),
            worktree_gate: std::sync::RwLock::new(()),
            signing_key,
            environment_id,
            label,
            dir: dir.into(),
            asset_profile_root: dir.into(),
            request_release_ms: std::sync::atomic::AtomicU64::new(30 * 60 * 1000),
            settings_cache: Mutex::new(None),
        })
    }

    /// Backpressure on blocking workers, outside the receipt transaction.
    pub fn wait_for_wal_budget(&self) {
        // Backpressure before opening a write transaction. A single transaction can
        // exceed the budget; pinned readers delay reclamation rather than grow WAL forever.
        let wal = self.dir.join("o3.sqlite-wal");
        while std::fs::metadata(&wal).is_ok_and(|m| m.len() > 32 * 1024 * 1024) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Separate connection and OS thread: checkpoint I/O never runs in a commit or Tokio worker.
    pub fn start_checkpointer(self: &std::sync::Arc<Self>) {
        if self
            .checkpoint_started
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let weak = std::sync::Arc::downgrade(self);
        let path = self.dir.join("o3.sqlite");
        let wake = self.checkpoint_wake.clone();
        std::thread::Builder::new().name("sqlite-checkpoint".into()).stack_size(256 * 1024).spawn(move || {
            let Ok(mut db) = Connection::open(path) else { return };
            profiled(&mut db);
            let _ = db.execute_batch("PRAGMA busy_timeout=0; PRAGMA journal_size_limit=33554432; PRAGMA wal_autocheckpoint=0;");
            while weak.strong_count() != 0 {
                let (dirty,signal)=&*wake;
                let mut pending=dirty.lock().unwrap();
                while !*pending {
                    pending=signal.wait(pending).unwrap();
                }
                *pending=false;
                drop(pending);
                let _ = db.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
                // PASSIVE never waits for readers. At the budget, try reclaiming
                // the file; busy_timeout=0 keeps pinned snapshots nonblocking.
                if db.path().is_some_and(|p| std::fs::metadata(format!("{p}-wal")).is_ok_and(|m| m.len() > 32 * 1024 * 1024)) {
                    let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }).expect("start SQLite checkpointer");
    }

    /// A one-use pairing secret, as `/api/auth/pairing-token` hands out.
    pub fn issue_pairing(&self, label: Option<&str>, ttl_ms: i64) -> Result<Value, String> {
        self.issue_pairing_scoped(label, ttl_ms, &STANDARD_SCOPES.join(" "), "owner")
    }
    pub fn issue_pairing_scoped(
        &self,
        label: Option<&str>,
        ttl_ms: i64,
        scopes: &str,
        subject: &str,
    ) -> Result<Value, String> {
        let (id, credential, now) = (random_id()?, random_id()?, now_ms());
        self.db.lock().unwrap().execute(
            "INSERT INTO auth_pairing_links(id,credential,scopes,subject,label,created_at,expires_at) VALUES(?,?,?,?,?,?,?)",
            params![id, credential, scopes, subject, label, now, now + ttl_ms],
        ).map_err(|e| e.to_string())?;
        let mut result =
            json!({ "id": id, "credential": credential, "expiresAt": iso(now + ttl_ms) });
        if let Some(label) = label {
            result["label"] = label.into();
        }
        Ok(result)
    }

    /// Consumes `credential` atomically and opens a bearer session. `None` if it is unusable.
    pub fn exchange_pairing(
        &self,
        credential: &str,
        client: &Value,
    ) -> Result<Option<Value>, String> {
        let now = now_ms();
        let db = self.db.lock().unwrap();
        let grant: Option<(String, String, Option<String>)> = db.query_row(
            "UPDATE auth_pairing_links SET consumed_at=?1 WHERE credential=?2 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>?1 RETURNING scopes,subject,label",
            params![now, credential], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((scopes, subject, label)) = grant else {
            return Ok(None);
        };
        let mut client = client.clone();
        if let Some(label) = label {
            client["client_label"] = label.into();
        }
        let sid = random_id()?;
        let exp = now + auth::SESSION_TTL_MS;
        db.execute(
            "INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at) VALUES(?,?,?,?,?,?,?)",
            params![sid, subject, scopes, "bearer-access-token", client.to_string(), now, exp],
        ).map_err(|e| e.to_string())?;
        let list: Vec<&str> = scopes.split(' ').collect();
        let claims = json!({ "v": 1, "kind": "session", "sid": sid, "sub": subject, "scopes": list, "method": "bearer-access-token", "iat": now, "exp": exp });
        Ok(Some(json!({
            "access_token": auth::sign(&claims, &self.signing_key),
            "issued_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "token_type": "Bearer",
            "expires_in": auth::SESSION_TTL_MS / 1000,
            "scope": scopes,
        })))
    }

    /// Session claims when `token` verifies and its row is neither revoked nor expired.
    pub fn session(&self, token: &str) -> Option<Value> {
        let now = now_ms();
        let claims = auth::verify(token, "session", &self.signing_key, now)?;
        self.live(claims, now).filter(|v| v["sub"] != "mcp-client")
    }

    /// Dedicated MCP sessions never authenticate ordinary HTTP or WebSocket RPC.
    pub fn mcp_session(&self, token: &str) -> Option<Value> {
        let now = now_ms();
        let claims = auth::verify(token, "session", &self.signing_key, now)?;
        self.live(claims, now).filter(|v| {
            v["sub"] == "mcp-client"
                && v["jkt"].is_null()
                && v["method"] == "bearer-access-token"
                && v["scopes"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s == "orchestration:read"))
        })
    }

    /// The owning session of a websocket ticket; revoking a session voids its tickets.
    pub fn ticket_session(&self, ticket: &str, usage: &str) -> Option<Value> {
        let now = now_ms();
        let claims = auth::verify(ticket, "websocket", &self.signing_key, now)?;
        if !claims["aud"].is_null() && claims["aud"] != usage {
            return None;
        }
        self.live(claims, now).filter(|v| v["sub"] != "mcp-client")
    }

    fn live(&self, claims: Value, now: i64) -> Option<Value> {
        let sid = claims["sid"].as_str()?;
        let (scopes, subject, client): (String, String, String) = self.reader.lock().unwrap()
            .query_row("SELECT scopes,subject,client FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?", params![sid,now], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().ok()??;
        let mut claims = claims;
        claims["scopes"] = json!(scopes.split_whitespace().collect::<Vec<_>>());
        claims["sub"] = subject.into();
        let client: Value = serde_json::from_str(&client).ok()?;
        claims["runtimeModeCeiling"] = client["runtimeModeCeiling"].clone();
        Some(claims)
    }

    pub fn issue_session(
        &self,
        subject: &str,
        scopes: &[String],
        ttl: i64,
        label: &str,
    ) -> Result<String, String> {
        self.issue_session_client(subject, scopes, ttl, &json!({"client_label":label}))
    }
    pub fn issue_mcp_session(&self, access: &str, ttl: i64, label: &str) -> Result<String, String> {
        let scopes: &[&str] = match access {
            "read-only" => &["orchestration:read"],
            "approval-required" | "auto-accept-edits" => {
                &["orchestration:read", "orchestration:operate"]
            }
            "full-access" => &[
                "orchestration:read",
                "orchestration:operate",
                "settings:write",
                "source-control:write",
            ],
            _ => return Err("invalid MCP access level".into()),
        };
        self.issue_session_client("mcp-client", &scopes.iter().map(|s|(*s).to_owned()).collect::<Vec<_>>(),ttl,
            &json!({"client_label":label,"runtimeModeCeiling":if access=="read-only"{Value::Null}else{json!(access)}}))
    }
    fn issue_session_client(
        &self,
        subject: &str,
        scopes: &[String],
        ttl: i64,
        client: &Value,
    ) -> Result<String, String> {
        let sid = random_id()?;
        let now = now_ms();
        self.db.lock().unwrap().execute("INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at) VALUES(?,?,?,?,?,?,?)",params![sid,subject,scopes.join(" "),"bearer-access-token",client.to_string(),now,now+ttl]).map_err(|e|e.to_string())?;
        Ok(auth::sign(
            &json!({"v":1,"kind":"session","sid":sid,"sub":subject,"scopes":scopes,"method":"bearer-access-token","iat":now,"exp":now+ttl}),
            &self.signing_key,
        ))
    }
    pub fn session_live(&self, session: &Value) -> bool {
        self.live(session.clone(), now_ms()).is_some()
    }
    pub fn background_needed(&self) -> bool {
        self.rpc_clients.load(std::sync::atomic::Ordering::Relaxed) > 0
            || !self.active.lock().unwrap().is_empty()
    }
    pub fn notify(&self) {
        let (dirty, wake) = &*self.checkpoint_wake;
        *dirty.lock().unwrap() = true;
        wake.notify_one();
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
    /// CLI view used by desktop bridge revocation, retaining issuer labels.
    pub fn list_sessions(&self) -> Result<Value, String> {
        let db = self.db.lock().unwrap();
        let mut query = db.prepare("SELECT session_id,client FROM auth_sessions WHERE revoked_at IS NULL AND expires_at>?").map_err(|e| e.to_string())?;
        let rows = query.query_map([now_ms()], |r| {
            let client: String = r.get(1)?;
            let client: Value = serde_json::from_str(&client).unwrap_or(json!({}));
            Ok(json!({"sessionId":r.get::<_,String>(0)?, "client":{"label":client["client_label"]}}))
        }).map_err(|e| e.to_string())?;
        Ok(Value::Array(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?,
        ))
    }
    pub fn revoke_session(&self, sid: &str) -> Result<bool, String> {
        self.notify();
        self.db
            .lock()
            .unwrap()
            .execute(
                "UPDATE auth_sessions SET revoked_at=? WHERE session_id=? AND revoked_at IS NULL",
                params![now_ms(), sid],
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let (dirty, wake) = &*self.checkpoint_wake;
        *dirty.lock().unwrap() = true;
        wake.notify_one();
    }
}
