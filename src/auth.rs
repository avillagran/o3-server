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
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "signing key must be 32 bytes",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut key = vec![0u8; 32];
            getrandom::fill(&mut key).map_err(io::Error::other)?;
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?
                .write_all(&key)?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

const TRAILING_BITS_OK: base64::engine::GeneralPurposeConfig =
    base64::engine::GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_allow_trailing_bits(true);
const URL_LENIENT: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    TRAILING_BITS_OK.with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
);
const STANDARD_LENIENT: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    TRAILING_BITS_OK.with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireCanonical),
);

fn strip_crlf(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\r', '\n']) {
        s.replace(['\r', '\n'], "").into()
    } else {
        s.into()
    }
}

/// Effect's `Base64Url.decode`, which T3's PayloadSigner and token payloads use: padded or
/// unpadded, CR/LF stripped, non-zero trailing bits ignored.
pub fn decode_b64url(s: &str) -> Option<Vec<u8>> {
    let s = strip_crlf(s);
    let data = s.trim_end_matches('=');
    let pad = s.len() - data.len();
    let shape_ok = if pad > 0 {
        pad <= 2 && s.len().is_multiple_of(4)
    } else {
        s.len() % 4 != 1
    };
    let alphabet_ok = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'_';
    if !shape_ok || !data.bytes().all(alphabet_ok) {
        return None;
    }
    URL_LENIENT.decode(data).ok()
}

/// Effect's `Base64.decode` (webhook signatures): canonical padding, CR/LF stripped, non-zero
/// trailing bits ignored.
pub fn decode_b64(s: &str) -> Option<Vec<u8>> {
    STANDARD_LENIENT.decode(strip_crlf(s).as_bytes()).ok()
}

/// Effect's `Hex.decode`: even length, ASCII hex digits of either case only.
pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) || !b.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).unwrap() as u8;
    Some(
        b.chunks(2)
            .map(|p| digit(p[0]) << 4 | digit(p[1]))
            .collect(),
    )
}

/// T3's `const [payload, signature] = token.split(".")`: both non-empty, later parts ignored.
pub fn token_parts(token: &str) -> Option<(&str, &str)> {
    let mut parts = token.split('.');
    let (payload, signature) = (parts.next()?, parts.next()?);
    (!payload.is_empty() && !signature.is_empty()).then_some((payload, signature))
}

pub fn sign(claims: &Value, secret: &[u8]) -> String {
    let payload = B64.encode(claims.to_string());
    let mut hmac = mac(secret);
    hmac.update(payload.as_bytes());
    format!("{payload}.{}", B64.encode(hmac.finalize().into_bytes()))
}

/// Claims of a token signed with `secret`, of this `kind`, not expired at `now_ms`.
pub fn verify(token: &str, kind: &str, secret: &[u8], now_ms: i64) -> Option<Value> {
    let (payload, signature) = token_parts(token)?;
    let mut hmac = mac(secret);
    hmac.update(payload.as_bytes());
    hmac.verify_slice(&decode_b64url(signature)?).ok()?;
    let claims: Value = serde_json::from_slice(&decode_b64url(payload)?).ok()?;
    (claims["kind"] == kind && claims["exp"].as_i64()? > now_ms).then_some(claims)
}

pub fn websocket_ticket(
    session: &Value,
    secret: &[u8],
    now_ms: i64,
    usage: Option<&str>,
) -> String {
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
        let expected =
            "eyJleHAiOjEwLCJraW5kIjoic2Vzc2lvbiJ9.Vvxxfz3ecI3mln6fdU_gRtpD7Pjv_8rYYojyBoadWbM";
        assert_eq!(
            sign(&json!({ "exp": 10, "kind": "session" }), KEY),
            expected
        );
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

    fn hex_of(bytes: Option<Vec<u8>>) -> Option<String> {
        bytes.map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
    }

    #[test]
    fn decoders_match_effect_4_1_2() {
        // Oracle: effect@4.1.2 dist Base64Url/Base64/Hex.decode, which T3 nightly 4192 uses.
        let url = [
            ("AAAA", Some("000000")),
            ("AQ", Some("01")),
            ("AQ==", Some("01")),
            ("AQ=", None),
            ("AR", Some("01")),
            ("AQI", Some("0102")),
            ("AQI=", Some("0102")),
            ("A", None),
            ("AQ===", None),
            ("A=Q", None),
            ("+/", None),
            ("-_", Some("fb")),
            ("AQ\r\n", Some("01")),
            ("", Some("")),
            ("AQ.", None),
            ("AAAAAA==", Some("00000000")),
        ];
        for (input, want) in url {
            assert_eq!(
                hex_of(decode_b64url(input)).as_deref(),
                want,
                "url {input:?}"
            );
        }
        let std = [
            ("AQ==", Some("01")),
            ("AR==", Some("01")),
            ("AQI=", Some("0102")),
            ("AQ", None),
            ("AQ=", None),
            ("A===", None),
            ("-_8=", None),
            ("+/8=", Some("fbff")),
            ("AA=A", None),
            ("", Some("")),
            ("AQ==\n", Some("01")),
        ];
        for (input, want) in std {
            assert_eq!(hex_of(decode_b64(input)).as_deref(), want, "std {input:?}");
        }
        let hex = [
            ("0aFF", Some("0aff")),
            ("+f", None),
            ("0", None),
            ("zz", None),
            ("", Some("")),
            ("ÿÿ", None),
        ];
        for (input, want) in hex {
            assert_eq!(hex_of(decode_hex(input)).as_deref(), want, "hex {input:?}");
        }
    }

    #[test]
    fn verify_accepts_what_t3_payload_signer_accepts() {
        let token = sign(&json!({ "kind": "session", "exp": 10 }), KEY);
        let (payload, signature) = token.split_once('.').unwrap();
        // 32-byte tag: 43 chars unpadded, so one '=' completes it. Trailing segments are ignored.
        assert!(verify(&format!("{payload}.{signature}="), "session", KEY, 9).is_some());
        assert!(verify(&format!("{token}.ignored"), "session", KEY, 9).is_some());
        assert!(verify(&format!("{payload}."), "session", KEY, 9).is_none());
        assert!(verify(&format!(".{signature}"), "session", KEY, 9).is_none());
    }

    #[test]
    fn websocket_ticket_keeps_session_and_proof_key() {
        let session = json!({ "sid": "s1", "jkt": "thumb" });
        let claims = verify(
            &websocket_ticket(&session, KEY, 0, Some("media")),
            "websocket",
            KEY,
            1,
        )
        .unwrap();
        assert_eq!(
            (&claims["sid"], &claims["jkt"], &claims["aud"]),
            (&json!("s1"), &json!("thumb"), &json!("media"))
        );
    }
}
