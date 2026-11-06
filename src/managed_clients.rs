//! Managed client identities and topology, based on the native authentication service.
use crate::{managed_limits::AuthPolicy, managed_policy::Policy};
use serde_json::Value;
use std::net::IpAddr;

pub const LINK_SUBJECT: &str = "one-time-token";
pub const CONNECT_SUBJECT: &str = "cloud-connect";
pub const STARTUP_SUBJECT: &str = "administrative-bootstrap";

pub fn web_auth_policy(host: Option<&str>) -> AuthPolicy {
    if crate::managed_limits::loopback_bind_address(host).is_some() {
        AuthPolicy::LocalBrowser
    } else {
        AuthPolicy::Remote
    }
}
pub fn refusal(policy: &Policy, action: &str, detail: Option<&str>) -> String {
    let organization = policy
        .document
        .as_ref()
        .map(|document| document.organization.as_str())
        .unwrap_or("Your organization");
    format!(
        "{organization}'s policy does not allow {action}{}.",
        detail
            .map(|detail| format!(", {detail}"))
            .unwrap_or_default()
    )
}
pub fn remote_pairing_refusal(policy: &Policy) -> String {
    refusal(
        policy,
        "remote clients",
        Some("so this server cannot create pairing links"),
    )
}
pub fn local_peer(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
    }
}
pub fn canonical_peer(peer: IpAddr) -> String {
    match peer {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or_else(|| ip.to_string(), |ip| ip.to_string()),
        IpAddr::V4(ip) => ip.to_string(),
    }
}
fn native_local_host(host: &str) -> bool {
    let lower = host.to_lowercase();
    let address = lower.strip_prefix('[').unwrap_or(&lower);
    let address = address
        .strip_suffix(']')
        .unwrap_or(address)
        .trim_end_matches('.');
    if matches!(address, "localhost" | "::1") {
        return true;
    }
    let parts = address
        .split('.')
        .filter_map(crate::managed_tailscale::js_integer)
        .collect::<Vec<_>>();
    address.split('.').count() == 4
        && parts.len() == 4
        && parts.iter().all(|number| (0.0..=255.0).contains(number))
        && parts[0] == 127.0
}
pub fn subject_disallowed(policy: &Policy, subject: &str) -> bool {
    (subject == LINK_SUBJECT && !policy.allows_remote())
        || (subject == CONNECT_SUBJECT && !policy.allows_connect())
}
pub fn session_disallowed(policy: &Policy, subject: &str, client: &Value) -> bool {
    subject_disallowed(policy, subject)
        || (!policy.allows_remote()
            && subject == STARTUP_SUBJECT
            && client
                .get("ipAddress")
                .is_some_and(|ip| ip.as_str().is_none_or(|ip| !native_local_host(ip))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::State;
    use serde_json::json;
    use std::{path::Path, sync::Arc};
    fn state() -> State {
        crate::test_sandbox::assert_isolated();
        State::open(&std::env::temp_dir().join(format!(
            "managed-client-auth-{}",
            crate::store::random_id().unwrap()
        )))
        .unwrap()
    }
    fn policy(limits: Value) -> Policy {
        Policy::parse(
            Path::new("/fixture/policy.json"),
            &json!({"schemaVersion":1,"organization":"Fixture Corp","limits":limits}).to_string(),
        )
        .unwrap()
    }
    #[test]
    fn revocation_is_selective_and_invalidates_existing_sessions_and_links() {
        for (limits, expected_sessions, expected_links) in [
            (json!({"clients":["local-browser"]}), 3, 2),
            (json!({"connect":false}), 1, 1),
        ] {
            let mut state = state();
            let mut tokens = Vec::new();
            let mut grants = Vec::new();
            for (subject, ip) in [
                (LINK_SUBJECT, None),
                (CONNECT_SUBJECT, None),
                (STARTUP_SUBJECT, Some("192.0.2.4")),
                (STARTUP_SUBJECT, Some("127.0.0.1")),
                (STARTUP_SUBJECT, None),
                ("owner", Some("192.0.2.4")),
            ] {
                let token = state
                    .issue_session(subject, &["orchestration:read".into()], 60000, "fixture")
                    .unwrap();
                let claims = state.session(&token).unwrap();
                let client = ip.map_or_else(|| json!({}), |ip| json!({"ipAddress":ip}));
                state
                    .db
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE auth_sessions SET client=? WHERE session_id=?",
                        rusqlite::params![client.to_string(), claims["sid"].as_str().unwrap()],
                    )
                    .unwrap();
                tokens.push((subject, ip, token));
            }
            for subject in [LINK_SUBJECT, CONNECT_SUBJECT, STARTUP_SUBJECT, "owner"] {
                grants.push((
                    subject,
                    state
                        .issue_pairing_scoped(Some("fixture"), 60000, "orchestration:read", subject)
                        .unwrap(),
                ));
            }
            state.managed_policy = policy(limits);
            let revoked = state.revoke_policy_clients().unwrap();
            assert_eq!(revoked["sessions"], expected_sessions);
            assert_eq!(revoked["pairingLinks"], expected_links);
            for (subject, ip, token) in tokens {
                assert_eq!(
                    state.session(&token).is_none(),
                    session_disallowed(
                        &state.managed_policy,
                        subject,
                        &ip.map_or_else(|| json!({}), |ip| json!({"ipAddress":ip}))
                    ),
                    "{subject} {ip:?}"
                );
            }
            for (subject, grant) in grants {
                let revoked: Option<i64> = state
                    .db
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT revoked_at FROM auth_pairing_links WHERE id=?",
                        [grant["id"].as_str().unwrap()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    revoked.is_some(),
                    subject_disallowed(&state.managed_policy, subject)
                );
            }
            assert_eq!(
                state.revoke_policy_clients().unwrap(),
                json!({"sessions":0,"pairingLinks":0})
            );
        }
    }
    #[test]
    fn only_trusted_peer_metadata_can_redeem_a_local_startup_grant() {
        let mut state = state();
        let grant = state
            .issue_pairing_scoped(None, 60000, "orchestration:read", STARTUP_SUBJECT)
            .unwrap();
        let credential = grant["credential"].as_str().unwrap();
        state.managed_policy = policy(json!({"clients":["local-browser"]}));
        let forged = json!({"ipAddress":"127.0.0.1","client_label":"local"});
        assert!(state
            .exchange_pairing(credential, &forged)
            .unwrap()
            .is_none());
        assert!(state
            .exchange_pairing_from(credential, &forged, Some("192.0.2.9".parse().unwrap()))
            .unwrap()
            .is_none());
        let exchanged = state
            .exchange_pairing_from(
                credential,
                &json!({"ipAddress":"192.0.2.9"}),
                Some("::ffff:127.0.0.1".parse().unwrap()),
            )
            .unwrap()
            .unwrap();
        let claims = state
            .session(exchanged["access_token"].as_str().unwrap())
            .unwrap();
        let client: String = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT client FROM auth_sessions WHERE session_id=?",
                [claims["sid"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&client).unwrap()["ipAddress"],
            "127.0.0.1"
        );
        assert!(state
            .issue_pairing(None, 60000)
            .unwrap_err()
            .contains("cannot create pairing links"));
        assert!(state
            .exchange_pairing_from(credential, &json!({}), Some("127.0.0.1".parse().unwrap()))
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn embedded_remote_transport_is_refused_but_connect_false_keeps_independent_remote_access(
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut local = state();
        local.managed_policy = policy(json!({"clients":["local-browser"]}));
        let (server, mut client) = tokio::io::duplex(4096);
        assert!(crate::serve_io(server, Arc::new(local)).await.is_err());
        let mut byte = [0];
        assert_eq!(client.read(&mut byte).await.unwrap(), 0);
        let mut remote = state();
        remote.managed_policy = policy(json!({"connect":false}));
        let (server, mut client) = tokio::io::duplex(8192);
        let task = tokio::spawn(crate::serve_io(server, Arc::new(remote)));
        client.write_all(b"GET /.well-known/t3/environment HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        task.await.unwrap().unwrap();
    }
    #[test]
    fn advertised_web_auth_follows_actual_host_classification_and_is_immutable() {
        let state = state();
        state.configure_web_auth(Some("0.0.0.0")).unwrap();
        assert_eq!(
            crate::settings::auth_descriptor_for(&state)["policy"],
            "remote-reachable"
        );
        assert!(state.configure_web_auth(Some("127.0.0.1")).is_err());
        for ip in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            assert!(local_peer(ip.parse().unwrap()));
        }
        for ip in ["192.0.2.1", "100.64.0.1", "::ffff:192.0.2.1"] {
            assert!(!local_peer(ip.parse().unwrap()));
        }
    }
    #[test]
    #[ignore = "Official nightly in a private machine-policy and auth-profile container only"]
    fn official_native_revocation_and_pairing_refusals_match() {
        assert!(Path::new("/run/.containerenv").is_file());
        assert_eq!(
            std::env::var("O3_POLICY_FIXED_PATH_CONTAINER").as_deref(),
            Ok("1")
        );
        let binary = std::env::var("T3_REFERENCE_BIN").unwrap();
        let version = std::process::Command::new(&binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).contains("20261106.4262"));
        let policy_path = Path::new("/etc/t3code/policy.json");
        let run = |profile: &Path, args: &[&str]| {
            std::process::Command::new(&binary)
                .env("T3CODE_HOME", profile)
                .args(args)
                .output()
                .unwrap()
        };
        for limits in [
            json!({"clients":["local-browser"]}),
            json!({"connect":false}),
        ] {
            std::fs::write(
                policy_path,
                r#"{"schemaVersion":1,"organization":"Fixture Corp"}"#,
            )
            .unwrap();
            let profile = std::env::temp_dir().join(format!(
                "native-client-revocation-{}",
                crate::store::random_id().unwrap()
            ));
            for subject in [
                LINK_SUBJECT,
                CONNECT_SUBJECT,
                STARTUP_SUBJECT,
                STARTUP_SUBJECT,
                STARTUP_SUBJECT,
                "owner",
            ] {
                let result = run(
                    &profile,
                    &[
                        "auth",
                        "session",
                        "issue",
                        "--subject",
                        subject,
                        "--ttl",
                        "1h",
                        "--json",
                    ],
                );
                assert!(
                    result.status.success(),
                    "Native fixture session issuance failed"
                );
            }
            let result = run(&profile, &["auth", "pairing", "create", "--json"]);
            assert!(
                result.status.success(),
                "Native fixture grant issuance failed"
            );
            let db_path = profile.join("userdata/statev2.sqlite");
            let db = rusqlite::Connection::open(&db_path).unwrap();
            let sessions = {
                let mut query = db
                    .prepare("SELECT session_id,subject FROM auth_sessions ORDER BY rowid")
                    .unwrap();
                let rows = query
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                rows
            };
            assert_eq!(sessions.len(), 6);
            let mut bootstrap = 0;
            let mut expected = Vec::new();
            let managed = policy(limits.clone());
            for (id, subject) in &sessions {
                let ip = if subject == STARTUP_SUBJECT {
                    let ip = match bootstrap {
                        0 => Some("192.0.2.9"),
                        1 => Some("127.0.0.1"),
                        _ => None,
                    };
                    bootstrap += 1;
                    ip
                } else if subject == "owner" {
                    Some("192.0.2.9")
                } else {
                    None
                };
                db.execute(
                    "UPDATE auth_sessions SET client_ip_address=? WHERE session_id=?",
                    rusqlite::params![ip, id],
                )
                .unwrap();
                expected.push((
                    id.clone(),
                    session_disallowed(
                        &managed,
                        subject,
                        &ip.map_or_else(|| json!({}), |ip| json!({"ipAddress":ip})),
                    ),
                ));
            }
            let template: String = db
                .query_row(
                    "SELECT id FROM auth_pairing_links WHERE subject=? LIMIT 1",
                    [LINK_SUBJECT],
                    |row| row.get(0),
                )
                .unwrap();
            for subject in [CONNECT_SUBJECT, STARTUP_SUBJECT, "owner"] {
                db.execute("INSERT INTO auth_pairing_links(id,credential,method,scopes,subject,label,proof_key_thumbprint,created_at,expires_at,consumed_at,revoked_at) SELECT ?1,?2,method,scopes,?3,label,proof_key_thumbprint,created_at,expires_at,NULL,NULL FROM auth_pairing_links WHERE id=?4",rusqlite::params![crate::store::random_id().unwrap(),crate::store::random_id().unwrap(),subject,template]).unwrap();
            }
            drop(db);
            std::fs::write(
                policy_path,
                json!({"schemaVersion":1,"organization":"Fixture Corp","limits":limits})
                    .to_string(),
            )
            .unwrap();
            let listed = run(&profile, &["auth", "session", "list", "--json"]);
            assert!(listed.status.success(), "Native policy session list failed");
            let db = rusqlite::Connection::open(&db_path).unwrap();
            for (id, denied) in expected {
                let revoked: Option<String> = db
                    .query_row(
                        "SELECT revoked_at FROM auth_sessions WHERE session_id=?",
                        [id],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    revoked.is_some(),
                    denied,
                    "Native persisted session revocation"
                );
            }
            let grants = {
                let mut query = db
                    .prepare("SELECT subject,revoked_at FROM auth_pairing_links")
                    .unwrap();
                let rows = query
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                rows
            };
            for (subject, revoked) in grants {
                assert_eq!(
                    revoked.is_some(),
                    subject_disallowed(&managed, &subject),
                    "Native persisted grant revocation: {subject}"
                );
            }
            let created = run(&profile, &["auth", "pairing", "create", "--json"]);
            assert_eq!(created.status.success(), managed.allows_remote());
            if !managed.allows_remote() {
                let diagnostic = format!(
                    "{}{}",
                    String::from_utf8_lossy(&created.stdout),
                    String::from_utf8_lossy(&created.stderr)
                );
                assert!(
                    diagnostic.contains(&remote_pairing_refusal(&managed)),
                    "Native refusal presentation: {diagnostic}"
                );
            }
        }
        println!("Official native selective auth revocation and pairing refusal verified for local-only and Connect-disabled policies; no inference");
    }
}
