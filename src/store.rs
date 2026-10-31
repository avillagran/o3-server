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
    pub db: Mutex<Connection>,
    pub signing_key: Vec<u8>,
    pub environment_id: String,
    pub label: String,
    pub dir: PathBuf,
    pub provider: Mutex<Option<std::sync::Arc<dyn crate::provider::Provider>>>,
    pub active: Mutex<
        std::collections::BTreeMap<
            String,
            tokio::sync::mpsc::Sender<crate::provider::ProviderControl>,
        >,
    >,
    pub settings_lock: Mutex<()>,
    pub changed: tokio::sync::watch::Sender<u64>,
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
PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON; PRAGMA journal_size_limit=33554432; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
CREATE TABLE IF NOT EXISTS auth_sessions(
  session_id TEXT PRIMARY KEY, subject TEXT NOT NULL, scopes TEXT NOT NULL, method TEXT NOT NULL,
  client TEXT NOT NULL, issued_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, revoked_at INTEGER);
CREATE TABLE IF NOT EXISTS auth_pairing_links(
  id TEXT PRIMARY KEY, credential TEXT NOT NULL UNIQUE, scopes TEXT NOT NULL, subject TEXT NOT NULL,
  label TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, consumed_at INTEGER, revoked_at INTEGER);
";

impl State {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let db = Connection::open(dir.join("o3.sqlite")).map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
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
        Ok(Self {
            settings_lock: Mutex::new(()),
            provider: Mutex::new(None),
            active: Mutex::new(std::collections::BTreeMap::new()),
            changed,
            db: Mutex::new(db),
            signing_key,
            environment_id,
            label,
            dir: dir.into(),
        })
    }

    /// A one-use pairing secret, as `/api/auth/pairing-token` hands out.
    pub fn issue_pairing(&self, label: Option<&str>, ttl_ms: i64) -> Result<Value, String> {
        let (id, credential, now) = (random_id()?, random_id()?, now_ms());
        self.db.lock().unwrap().execute(
            "INSERT INTO auth_pairing_links(id,credential,scopes,subject,label,created_at,expires_at) VALUES(?,?,?,?,?,?,?)",
            params![id, credential, STANDARD_SCOPES.join(" "), "owner", label, now, now + ttl_ms],
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
        let grant: Option<(String, String)> = db.query_row(
            "UPDATE auth_pairing_links SET consumed_at=?1 WHERE credential=?2 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>?1 RETURNING scopes,subject",
            params![now, credential], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((scopes, subject)) = grant else {
            return Ok(None);
        };
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
        self.live(claims, now)
    }

    /// The owning session of a websocket ticket; revoking a session voids its tickets.
    pub fn ticket_session(&self, ticket: &str, usage: &str) -> Option<Value> {
        let now = now_ms();
        let claims = auth::verify(ticket, "websocket", &self.signing_key, now)?;
        if !claims["aud"].is_null() && claims["aud"] != usage {
            return None;
        }
        self.live(claims, now)
    }

    fn live(&self, claims: Value, now: i64) -> Option<Value> {
        let sid = claims["sid"].as_str()?;
        self.db.lock().unwrap().query_row(
            "SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?",
            params![sid, now], |_| Ok(()),
        ).optional().ok()??;
        let (scopes, subject): (String, String) = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT scopes,subject FROM auth_sessions WHERE session_id=?",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()?;
        let mut claims = claims;
        claims["scopes"] = json!(scopes.split_whitespace().collect::<Vec<_>>());
        claims["sub"] = subject.into();
        Some(claims)
    }

    pub fn issue_session(
        &self,
        subject: &str,
        scopes: &[String],
        ttl: i64,
        label: &str,
    ) -> Result<String, String> {
        let sid = random_id()?;
        let now = now_ms();
        self.db.lock().unwrap().execute("INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at) VALUES(?,?,?,?,?,?,?)",params![sid,subject,scopes.join(" "),"bearer-access-token",json!({"client_label":label}).to_string(),now,now+ttl]).map_err(|e|e.to_string())?;
        Ok(auth::sign(
            &json!({"v":1,"kind":"session","sid":sid,"sub":subject,"scopes":scopes,"method":"bearer-access-token","iat":now,"exp":now+ttl}),
            &self.signing_key,
        ))
    }
    pub fn session_live(&self, session: &Value) -> bool {
        self.live(session.clone(), now_ms()).is_some()
    }
    pub fn notify(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
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
