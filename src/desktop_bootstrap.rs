//! Native desktop bootstrap input and rotating credentials. Secrets remain process-local.
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::{fmt, time::Duration};

pub const WINDOW_MS: i64 = 12 * 60 * 60 * 1000;
pub const LEGACY_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Deliberately has no Serialize implementation; startup credentials never enter projections.
#[derive(Clone)]
pub struct Envelope(Value);
impl fmt::Debug for Envelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DesktopBootstrap")
            .field("port", &self.port())
            .finish_non_exhaustive()
    }
}
impl Envelope {
    pub fn decode(mut value: Value) -> Result<Self, &'static str> {
        let object = value
            .as_object_mut()
            .ok_or("Invalid desktop bootstrap envelope")?;
        if object.get("mode").and_then(Value::as_str) != Some("desktop") {
            return Err("Invalid desktop bootstrap envelope");
        }
        for name in ["noBrowser", "tailscaleServeEnabled"] {
            if !object.get(name).is_some_and(Value::is_boolean) {
                return Err("Invalid desktop bootstrap envelope");
            }
        }
        for name in ["host", "desktopBootstrapToken"] {
            if !object.get(name).is_some_and(Value::is_string) {
                return Err("Invalid desktop bootstrap envelope");
            }
        }
        for name in ["port", "tailscaleServePort"] {
            if !object.get(name).is_some_and(|value| {
                integer(value).is_some_and(|number| (1.0..=65535.0).contains(&number))
            }) {
                return Err("Invalid desktop bootstrap envelope");
            }
        }
        for name in [
            "t3Home",
            "desktopBootstrapSecret",
            "otlpTracesUrl",
            "otlpMetricsUrl",
            "otlpLogsUrl",
        ] {
            if object.get(name).is_some_and(|value| !value.is_string()) {
                return Err("Invalid desktop bootstrap envelope");
            }
        }
        if object
            .get("shellEnvironmentPrepared")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err("Invalid desktop bootstrap envelope");
        }
        for name in [
            "desktopTelemetryFd",
            "desktopTelemetryControlFd",
            "desktopBrowserFd",
            "desktopBrowserControlFd",
        ] {
            if object
                .get(name)
                .is_some_and(|value| !integer(value).is_some_and(|number| number > 0.0))
            {
                return Err("Invalid desktop bootstrap envelope");
            }
        }
        if let Some(path) = object.get_mut("resourceMonitorPath") {
            let text = path.as_str().ok_or("Invalid desktop bootstrap envelope")?;
            let text = crate::native_goal::trim(text);
            if text.is_empty() {
                return Err("Invalid desktop bootstrap envelope");
            }
            *path = Value::String(text.into());
        }
        object.retain(|name, _| {
            matches!(
                name.as_str(),
                "mode"
                    | "noBrowser"
                    | "port"
                    | "t3Home"
                    | "host"
                    | "desktopBootstrapToken"
                    | "desktopBootstrapSecret"
                    | "shellEnvironmentPrepared"
                    | "tailscaleServeEnabled"
                    | "tailscaleServePort"
                    | "otlpTracesUrl"
                    | "otlpMetricsUrl"
                    | "otlpLogsUrl"
                    | "desktopTelemetryFd"
                    | "desktopTelemetryControlFd"
                    | "desktopBrowserFd"
                    | "desktopBrowserControlFd"
                    | "resourceMonitorPath"
            )
        });
        Ok(Self(value))
    }
    pub fn port(&self) -> u16 {
        self.0["port"].as_f64().unwrap() as u16
    }
    pub fn host(&self) -> &str {
        self.0["host"].as_str().unwrap()
    }
    pub fn home(&self) -> Option<&str> {
        self.0["t3Home"].as_str()
    }
    pub fn token(&self) -> &str {
        self.0["desktopBootstrapToken"].as_str().unwrap()
    }
    pub fn secret(&self) -> Option<&str> {
        self.0["desktopBootstrapSecret"].as_str()
    }
    pub fn value(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}
fn integer(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && value.fract() == 0.0)
}

pub fn token_at(secret: &str, now_ms: i64) -> Result<String, &'static str> {
    if secret.is_empty() {
        return Err("InvalidKey");
    }
    Ok(token_for_window(secret, now_ms.div_euclid(WINDOW_MS)))
}
fn token_for_window(secret: &str, window: i64) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(format!("t3-desktop-bootstrap:{window}").as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub fn valid_token(secret: &str, token: &str, now_ms: i64) -> Result<bool, &'static str> {
    if secret.is_empty() {
        return Err("InvalidKey");
    }
    // The native comparison is over the lowercase encoded text, not decoded hex bytes.
    if token.len() != 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(false);
    }
    let signature = crate::auth::decode_hex(token).expect("validated lowercase hex");
    let current = now_ms.div_euclid(WINDOW_MS);
    for window in [current - 1, current, current + 1] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(format!("t3-desktop-bootstrap:{window}").as_bytes());
        if mac.verify_slice(&signature).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_envelope_and_rotating_token_oracle() {
        fn native_result<T: serde::Serialize>(result: Result<T, &'static str>) -> Value {
            match result {
                Ok(value) => serde_json::json!({"value":value}),
                Err(error) => serde_json::json!({"error":error}),
            }
        }
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/desktop-bootstrap-4288.json"
        ))
        .unwrap();
        for case in fixture["tokens"].as_array().unwrap() {
            assert_eq!(
                native_result(token_at(
                    case["secret"].as_str().unwrap(),
                    case["nowMs"].as_i64().unwrap()
                )),
                case["expected"],
                "{case}"
            );
        }
        for case in fixture["verification"].as_array().unwrap() {
            assert_eq!(
                native_result(valid_token(
                    case["secret"].as_str().unwrap(),
                    case["token"].as_str().unwrap(),
                    case["nowMs"].as_i64().unwrap()
                )),
                case["expected"],
                "{case}"
            );
        }
        for case in fixture["envelopes"].as_array().unwrap() {
            let actual = Envelope::decode(case["input"].clone())
                .map(|envelope| envelope.0)
                .unwrap_or(Value::Null);
            assert_eq!(actual, case["expected"], "{case}");
        }
    }
}
