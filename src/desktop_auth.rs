//! Listener-local desktop authentication; bootstrap credentials never enter SQLite.
use crate::{desktop_bootstrap_grant::Credentials, managed_limits::AuthPolicy, store::State};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Mutex;

pub(crate) struct Listener {
    pub policy: AuthPolicy,
    pub desktop: bool,
    pub credentials: Mutex<Credentials>,
}
pub(crate) enum Exchange {
    Unmatched,
    Rejected,
    ScopeNotGranted,
    Session(Value),
}
pub(crate) enum ExchangeError {
    ScopeNotGranted,
    Internal(String),
}
impl From<String> for ExchangeError {
    fn from(value: String) -> Self {
        Self::Internal(value)
    }
}

pub(crate) fn parse_requested_scopes(value: &str) -> Option<Vec<String>> {
    let mut scopes = Vec::new();
    for scope in value.split(' ') {
        if scope.is_empty()
            || !scope.bytes().all(|byte| {
                byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
            })
        {
            return None;
        }
        if crate::store::STANDARD_SCOPES.contains(&scope)
            && !scopes.iter().any(|existing| existing == scope)
        {
            scopes.push(scope.to_owned());
        }
    }
    (!scopes.is_empty()).then_some(scopes)
}

pub(crate) fn select_scopes(granted: &str, requested: Option<&[String]>) -> Option<String> {
    let Some(requested) = requested else {
        return Some(granted.into());
    };
    let mut scopes = Vec::new();
    for scope in requested {
        if granted.split_whitespace().any(|granted| granted == scope)
            && !scopes.contains(&scope.as_str())
        {
            scopes.push(scope.as_str());
        }
    }
    (!scopes.is_empty()).then(|| scopes.join(" "))
}

impl State {
    pub fn configure_listener_auth(
        &self,
        host: Option<&str>,
        desktop: bool,
        token: Option<String>,
        secret: Option<String>,
    ) -> Result<(), String> {
        let policy = crate::desktop_startup::auth_policy(host, desktop);
        self.listener_auth
            .set(Listener {
                policy,
                desktop,
                credentials: Mutex::new(Credentials::new(token, secret, crate::store::now_ms())),
            })
            .map_err(|_| "Listener authentication is already configured".into())
    }

    /// This path is reached only after the exchange's trusted-peer managed-policy check.
    pub(crate) fn exchange_desktop(
        &self,
        credential: &str,
        client: &Value,
        peer: Option<std::net::IpAddr>,
        requested: Option<&[String]>,
    ) -> Result<Exchange, String> {
        let Some(listener) = self.listener_auth.get() else {
            return Ok(Exchange::Unmatched);
        };
        let now = crate::store::now_ms();
        let grant = listener
            .credentials
            .lock()
            .unwrap()
            .consume(credential, now, requested, None);
        let grant = match grant {
            Ok(grant) => grant,
            Err(crate::desktop_bootstrap_grant::Error::InvalidKey) => {
                return Err("Failed to consume desktop bootstrap credential".into())
            }
            Err(crate::desktop_bootstrap_grant::Error::ScopeNotGranted) => {
                return Ok(Exchange::ScopeNotGranted)
            }
            Err(crate::desktop_bootstrap_grant::Error::Expired) => return Ok(Exchange::Rejected),
        };
        let Some(_grant) = grant else {
            return Ok(Exchange::Unmatched);
        };
        let mut client = if client.is_object() {
            client.clone()
        } else {
            json!({})
        };
        client.as_object_mut().unwrap().remove("ipAddress");
        if let Some(peer) = peer {
            client["ipAddress"] = json!(crate::managed_clients::canonical_peer(peer));
        }
        let sid = crate::store::random_id()?;
        let exp = now + crate::auth::SESSION_TTL_MS;
        let Some(scopes) = select_scopes(&crate::store::STANDARD_SCOPES.join(" "), requested)
        else {
            return Ok(Exchange::ScopeNotGranted);
        };
        let mut db = self.db.lock().unwrap();
        let transaction = db.transaction().map_err(|error| error.to_string())?;
        transaction.execute("UPDATE auth_sessions SET revoked_at=? WHERE subject='desktop-bootstrap' AND method='bearer-access-token' AND revoked_at IS NULL",[now]).map_err(|error|error.to_string())?;
        transaction.execute("INSERT INTO auth_sessions(session_id,subject,scopes,method,client,issued_at,expires_at) VALUES(?,?,?,?,?,?,?)", params![sid,"desktop-bootstrap",scopes,"bearer-access-token",client.to_string(),now,exp]).map_err(|error|error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        let claims = json!({"v":1,"kind":"session","sid":sid,"sub":"desktop-bootstrap","scopes":scopes.split(' ').collect::<Vec<_>>(),"method":"bearer-access-token","iat":now,"exp":exp});
        Ok(Exchange::Session(json!({
            "access_token":crate::auth::sign(&claims,&self.signing_key),
            "issued_token_type":"urn:ietf:params:oauth:token-type:access_token",
            "token_type":"Bearer","expires_in":crate::auth::SESSION_TTL_MS/1000,"scope":scopes
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn state() -> State {
        crate::test_sandbox::assert_isolated();
        State::open(&std::env::temp_dir().join(format!(
            "desktop-auth-{}",
            crate::store::random_id().unwrap()
        )))
        .unwrap()
    }
    async fn exchange(state: Arc<State>, credential: &str) -> (u16, Value) {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("subject_token", credential)
            .finish();
        exchange_form(state, &body).await
    }
    async fn exchange_form(state: Arc<State>, body: &str) -> (u16, Value) {
        exchange_with_headers(state, body, "").await
    }
    async fn exchange_with_headers(state: Arc<State>, body: &str, headers: &str) -> (u16, Value) {
        let (server, mut client) = tokio::io::duplex(16384);
        let task = tokio::spawn(crate::http::serve_io(server, state));
        client.write_all(format!("POST /oauth/token HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\n{headers}Content-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        task.await.unwrap().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        (
            headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    }

    #[tokio::test]
    async fn proof_bound_exchange_is_not_downgraded_or_spent() {
        let state = state();
        let grant = state.issue_pairing(None, 60000).unwrap();
        let state = Arc::new(state);
        let credential = grant["credential"].as_str().unwrap();
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("subject_token", credential)
            .finish();
        assert_eq!(
            exchange_with_headers(state.clone(), &body, "DPoP: private-invalid-proof\r\n")
                .await
                .0,
            401
        );
        assert_eq!(exchange(state, credential).await.0, 200);
    }

    #[test]
    fn native_oauth_scope_parser_and_grant_contract_oracle() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/oauth-scopes-4288.json")).unwrap();
        let mut native = fixture["grantScopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        let mut local = crate::store::STANDARD_SCOPES.to_vec();
        native.sort_unstable();
        local.sort_unstable();
        assert_eq!(native, local);
        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                json!(parse_requested_scopes(case["input"].as_str().unwrap())),
                case["expected"],
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn requested_scopes_are_intersected_without_spending_rejected_pairing_grants() {
        let state = state();
        let grant = state
            .issue_pairing_scoped(None, 60000, "orchestration:read", "scope-fixture")
            .unwrap();
        state
            .configure_listener_auth(
                Some("127.0.0.1"),
                true,
                Some("private-scope-seed".into()),
                None,
            )
            .unwrap();
        let state = Arc::new(state);
        async fn scoped(state: Arc<State>, credential: &str, scope: &str) -> (u16, Value) {
            let body = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("subject_token", credential)
                .append_pair("scope", scope)
                .finish();
            exchange_form(state, &body).await
        }
        let (status, token) = scoped(
            state.clone(),
            "private-scope-seed",
            "filesystem:read orchestration:read filesystem:read unknown",
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(token["scope"], "filesystem:read orchestration:read");
        let access = token["access_token"].as_str().unwrap();
        assert_eq!(
            state.session(access).unwrap()["scopes"],
            json!(["filesystem:read", "orchestration:read"])
        );
        for scope in [
            "",
            " ",
            "unknown",
            "review:write",
            "orchestration:read\tfilesystem:read",
            "\u{feff}orchestration:read",
        ] {
            let (status, error) = scoped(state.clone(), "private-scope-seed", scope).await;
            assert_eq!(status, 400);
            assert_eq!(error["code"], "invalid_scope");
            assert!(state.session(access).is_some());
        }
        let credential = grant["credential"].as_str().unwrap();
        let (status, error) = scoped(state.clone(), credential, "filesystem:read").await;
        assert_eq!(status, 400);
        assert_eq!(error["code"], "scope_not_granted");
        let (status, issued) = scoped(state.clone(), credential, "orchestration:read").await;
        assert_eq!(status, 200);
        assert_eq!(issued["scope"], "orchestration:read");
        assert_eq!(scoped(state, credential, "orchestration:read").await.0, 401);
    }

    #[tokio::test]
    async fn real_http_desktop_exchange_reuses_seed_and_replaces_only_desktop_sessions() {
        for rotating in [false, true] {
            let state = state();
            let secret = "private-synthetic-desktop-key";
            state
                .configure_listener_auth(
                    Some("127.0.0.1"),
                    true,
                    Some("private-seed".into()),
                    rotating.then(|| secret.into()),
                )
                .unwrap();
            let other = state
                .issue_session(
                    "unrelated",
                    &["orchestration:read".into()],
                    60000,
                    "private fixture",
                )
                .unwrap();
            let state = Arc::new(state);
            let credential = if rotating {
                crate::desktop_bootstrap::token_at(secret, crate::store::now_ms()).unwrap()
            } else {
                "private-seed".into()
            };
            let (status, first) = exchange(state.clone(), &credential).await;
            assert_eq!(status, 200);
            let first_token = first["access_token"].as_str().unwrap();
            assert!(state.session(first_token).is_some());
            let (status, second) = exchange(state.clone(), &credential).await;
            assert_eq!(status, 200);
            assert!(state.session(first_token).is_none());
            let claims = state
                .session(second["access_token"].as_str().unwrap())
                .unwrap();
            assert_eq!(claims["sub"], "desktop-bootstrap");
            assert_eq!(claims["scopes"], json!(crate::store::STANDARD_SCOPES));
            assert_eq!(
                claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
                crate::auth::SESSION_TTL_MS
            );
            assert!(state.session(&other).is_some());
            assert_eq!(exchange(state.clone(), "not-the-seed").await.0, 401);
            if rotating {
                assert_eq!(exchange(state.clone(), "private-seed").await.0, 401);
            }
            let db = state.db.lock().unwrap();
            let persisted_seeds: i64 = db
                .query_row("SELECT count(*) FROM auth_pairing_links", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(persisted_seeds, 0);
            let sessions: String = db
                .query_row(
                    "SELECT group_concat(client) FROM auth_sessions",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!sessions.contains(secret) && !sessions.contains("private-seed"));
        }
    }

    #[test]
    fn descriptor_preserves_remote_desktop_and_web_bootstrap_methods() {
        for (host, desktop, policy, methods) in [
            (
                "127.0.0.1",
                true,
                "desktop-managed-local",
                json!(["desktop-bootstrap"]),
            ),
            (
                "0.0.0.0",
                true,
                "remote-reachable",
                json!(["desktop-bootstrap", "one-time-token"]),
            ),
            (
                "127.0.0.1",
                false,
                "loopback-browser",
                json!(["one-time-token"]),
            ),
            (
                "0.0.0.0",
                false,
                "remote-reachable",
                json!(["one-time-token"]),
            ),
        ] {
            let state = state();
            state
                .configure_listener_auth(Some(host), desktop, None, None)
                .unwrap();
            let descriptor = crate::settings::auth_descriptor_for(&state);
            assert_eq!(descriptor["policy"], policy);
            assert_eq!(descriptor["bootstrapMethods"], methods);
            assert!(state.configure_web_auth(Some(host)).is_err());
        }
    }
}
