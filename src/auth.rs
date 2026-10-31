//! Signed credentials, byte-compatible with T3's SessionStore:
//! `base64url(json claims).base64url(hmac-sha256(encoded claims, server-signing-key))`.
//! Revocation and session rows live in the store; this module only proves integrity and expiry.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{fs, io, path::Path};

pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
pub const WEBSOCKET_TTL_MS: i64 = 5 * 60 * 1000;

fn mac(secret: &[u8]) -> Hmac<Sha256> {
    Hmac::new_from_slice(secret).expect("hmac accepts any key length")
}

/// The 32-byte key, created with owner-only permissions on first use.
pub fn load_or_create_key(path: &Path) -> io::Result<Vec<u8>> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    match fs::read(path) {
        Ok(key) if key.len() == 32 => Ok(key),
        Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "signing key must be 32 bytes")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut key = vec![0u8; 32];
            getrandom::fill(&mut key).map_err(io::Error::other)?;
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?.write_all(&key)?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

pub fn sign(claims: &Value, secret: &[u8]) -> String {
    let payload = B64.encode(claims.to_string());
    let mut hmac = mac(secret);
    hmac.update(payload.as_bytes());
    format!("{payload}.{}", B64.encode(hmac.finalize().into_bytes()))
}

/// Claims of a token signed with `secret`, of this `kind`, not expired at `now_ms`.
pub fn verify(token: &str, kind: &str, secret: &[u8], now_ms: i64) -> Option<Value> {
    let (payload, signature) = token.split_once('.')?;
    let mut hmac = mac(secret);
    hmac.update(payload.as_bytes());
    hmac.verify_slice(&B64.decode(signature).ok()?).ok()?;
    let claims: Value = serde_json::from_slice(&B64.decode(payload).ok()?).ok()?;
    (claims["kind"] == kind && claims["exp"].as_i64()? > now_ms).then_some(claims)
}

pub fn websocket_ticket(session: &Value, secret: &[u8], now_ms: i64, usage: Option<&str>) -> String {
    let mut claims = json!({ "v": 1, "kind": "websocket", "sid": session["sid"], "iat": now_ms, "exp": now_ms + WEBSOCKET_TTL_MS });
    if let Some(usage) = usage {
        claims["aud"] = usage.into();
    }
    if !session["jkt"].is_null() {
        claims["jkt"] = session["jkt"].clone();
    }
    sign(&claims, secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &[u8] = &[7; 32];

    #[test]
    fn matches_node_hmac_vector() {
        // Produced by Node's createHmac("sha256", key).update(payload).digest("base64url").
        let expected = "eyJleHAiOjEwLCJraW5kIjoic2Vzc2lvbiJ9.Vvxxfz3ecI3mln6fdU_gRtpD7Pjv_8rYYojyBoadWbM";
        assert_eq!(sign(&json!({ "exp": 10, "kind": "session" }), KEY), expected);
    }

    #[test]
    fn rejects_tamper_kind_and_expiry() {
        let token = sign(&json!({ "kind": "session", "exp": 10 }), KEY);
        assert!(verify(&token, "session", KEY, 9).is_some());
        assert!(verify(&token, "session", KEY, 10).is_none());
        assert!(verify(&token, "websocket", KEY, 9).is_none());
        assert!(verify(&token, "session", &[8; 32], 9).is_none());
        let (payload, signature) = token.split_once('.').unwrap();
        assert!(verify(&format!("{payload}x.{signature}"), "session", KEY, 9).is_none());
    }

    #[test]
    fn websocket_ticket_keeps_session_and_proof_key() {
        let session = json!({ "sid": "s1", "jkt": "thumb" });
        let claims = verify(&websocket_ticket(&session, KEY, 0, Some("media")), "websocket", KEY, 1).unwrap();
        assert_eq!((&claims["sid"], &claims["jkt"], &claims["aud"]), (&json!("s1"), &json!("thumb"), &json!("media")));
    }
}
