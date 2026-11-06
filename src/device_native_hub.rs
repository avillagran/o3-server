//! Discovery and lifecycle decisions from the pinned native device hub adapter.
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
}
impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub version: String,
    pub platform: Platform,
    pub booted: bool,
    pub physical: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HubError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct HubList {
    pub simulators: Vec<Device>,
    pub emulators: Vec<Device>,
    #[serde(default)]
    pub errors: Option<Vec<HubError>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HubAction {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<HubError>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_discovery_incomplete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_platforms: Option<Vec<Platform>>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CommandResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub body: Option<Value>,
}
fn request(origin: &str, path: &str, body: Option<Value>) -> Request {
    Request {
        method: if body.is_some() { "POST" } else { "GET" },
        url: format!("{origin}{path}"),
        body,
    }
}
pub fn list_request(origin: &str) -> Request {
    request(origin, "/api/devices", None)
}
pub fn boot_request(origin: &str, device: &Device) -> Request {
    request(
        origin,
        "/api/devices/boot",
        Some(json!({"platform":device.platform,"id":device.id,"name":device.name})),
    )
}
pub fn attach_request(origin: &str, device: &Device) -> Option<Request> {
    (device.platform == Platform::Ios).then(|| {
        request(
            origin,
            "/vendor/serve-sim/grid/api/start",
            Some(json!({"udid":device.id})),
        )
    })
}
pub fn shutdown_request(origin: &str, device: &Device) -> Request {
    match device.platform {
        Platform::Ios => request(
            origin,
            "/vendor/serve-sim/grid/api/shutdown",
            Some(json!({"udid":device.id})),
        ),
        Platform::Android => request(
            origin,
            "/api/devices/shutdown",
            Some(json!({"platform":device.platform,"id":device.id})),
        ),
    }
}
/// The hub expects JavaScript encodeURIComponent, including its unescaped punctuation.
pub fn screenshot_request(origin: &str, device: &Device) -> Request {
    let mut encoded = String::new();
    for byte in device.id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    let vendor = if device.platform == Platform::Ios {
        "serve-sim"
    } else {
        "serve-emu"
    };
    Request {
        method: "POST",
        url: format!("{origin}/vendor/{vendor}/api/screenshot?device={encoded}"),
        body: None,
    }
}
fn js_space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn trim(value: &str) -> &str {
    value.trim_matches(js_space)
}
fn diagnostics(text: &str) -> String {
    let units = text.encode_utf16().collect::<Vec<_>>();
    if units.len() > 2000 {
        format!("{}…", String::from_utf16_lossy(&units[..1999]))
    } else {
        text.into()
    }
}
fn error_head(error: &str) -> String {
    error
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .take_while(|line| {
            let head = line.trim_start_matches(js_space);
            !(head.len() < line.len() && head.starts_with("at "))
        })
        .map(trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn optional_string(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .is_none_or(|value| value.is_null() || value.is_string())
}
fn valid_errors(value: &Value) -> bool {
    value.get("errors").is_none_or(|errors| {
        errors.is_null()
            || errors.as_array().is_some_and(|errors| {
                errors.iter().all(|error| {
                    error.is_object()
                        && error["message"].is_string()
                        && optional_string(error, "error")
                })
            })
    })
}
pub fn operation_error(operation: &str, reason: &str) -> Value {
    let explanation = match reason {
        "hub_rejected" => "The device hub could not complete the request.",
        "invalid_payload" => "The device request could not be encoded.",
        "settings_failed" => "Could not read or save device settings.",
        _ => "Could not communicate with device support. Try refreshing devices.",
    };
    json!({"_tag":"DeviceOperationError","operation":operation,"reason":reason,"message":format!("Device {operation} failed: {explanation}")})
}
pub fn parse_list(body: Value) -> Result<HubList, Value> {
    if !valid_errors(&body) {
        return Err(operation_error("list", "request_failed"));
    }
    serde_json::from_value(body).map_err(|_| operation_error("list", "request_failed"))
}
pub fn parse_action(body: Value, operation: &str) -> Result<HubAction, Value> {
    if !valid_errors(&body)
        || ["id", "serial", "error"]
            .iter()
            .any(|key| !optional_string(&body, key))
    {
        return Err(operation_error(operation, "request_failed"));
    }
    serde_json::from_value(body).map_err(|_| operation_error(operation, "request_failed"))
}
fn stale_plist(error: &HubError) -> bool {
    if !error
        .message
        .starts_with("[apple-utils] Failed to read CoreSimulator plist at ")
    {
        return false;
    }
    static PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/CoreSimulator/Devices/[^/]+/device\.plist:$").unwrap());
    if !PATH.is_match(&error.message) {
        return false;
    }
    let error = error.error.as_deref().unwrap_or("");
    if error
        .strip_prefix("Error:")
        .is_some_and(|tail| tail.trim_start_matches(js_space).starts_with("ENOENT:"))
    {
        return true;
    }
    error.match_indices("code:").any(|(index, _)| {
        let boundary = index == 0
            || !error.as_bytes()[index - 1].is_ascii_alphanumeric()
                && error.as_bytes()[index - 1] != b'_';
        let tail = error[index + 5..].trim_start_matches(js_space);
        boundary && (tail.starts_with("'ENOENT'") || tail.starts_with("\"ENOENT\""))
    })
}
fn simulator_list_failed(message: &str) -> bool {
    [
        "[apple-utils] Failed to run `xcrun simctl list devices",
        "[apple-utils] Failed to parse JSON output:",
        "[apple-utils] Failed to list devices:",
        "[apple-utils] `xcrun simctl list devices --json` did not respond within ",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
}
pub fn discovery(list: HubList, android_available: bool, avds: &CommandResult) -> Discovery {
    let ios_incomplete = list.simulators.is_empty()
        && list.errors.as_ref().is_some_and(|errors| {
            errors
                .iter()
                .any(|error| simulator_list_failed(&error.message))
        });
    let mut devices = list.simulators;
    devices.extend(list.emulators);
    let mut details = list
        .errors
        .unwrap_or_default()
        .into_iter()
        .filter(|error| {
            !(!android_available && error.message.starts_with("[android-utils] "))
                && !stale_plist(error)
        })
        .map(|error| {
            let head = error_head(error.error.as_deref().unwrap_or(""));
            diagnostics(&if head.is_empty() {
                error.message
            } else {
                format!("{} {head}", error.message)
            })
        })
        .collect::<Vec<_>>();
    let mut android_incomplete = false;
    if android_available {
        if avds.code != 0 {
            android_incomplete = true;
            details.push(format!("Could not list Android virtual devices (emulator -list-avds, exit code {}). Discovered devices remain available.",avds.code));
        }
        for name in if avds.code == 0 {
            avds.stdout.as_str()
        } else {
            ""
        }
        .lines()
        .map(trim)
        .filter(|name| !name.is_empty())
        {
            if !devices
                .iter()
                .any(|device| device.platform == Platform::Android && device.name == name)
            {
                devices.push(Device {
                    id: name.into(),
                    name: name.into(),
                    version: "Android".into(),
                    platform: Platform::Android,
                    booted: false,
                    physical: false,
                });
            }
        }
    }
    let detail = details.join("\n");
    Discovery {
        devices,
        detail: (!detail.is_empty()).then_some(detail),
        android_discovery_incomplete: Some(android_incomplete),
        incomplete_platforms: ios_incomplete.then_some(vec![Platform::Ios]),
    }
}
pub fn listing_timeout(ios_available: bool, android_available: bool) -> Discovery {
    let mut platforms = Vec::new();
    if ios_available {
        platforms.push(Platform::Ios);
    }
    if android_available {
        platforms.push(Platform::Android);
    }
    Discovery {
        devices: Vec::new(),
        detail: Some("The device hub did not answer within 40 seconds.".into()),
        android_discovery_incomplete: None,
        incomplete_platforms: Some(platforms),
    }
}
pub fn boot_result(host_id: &str, device: &Device, result: HubAction) -> Result<String, Value> {
    if result.ok {
        return Ok(result
            .serial
            .or(result.id)
            .unwrap_or_else(|| device.id.clone()));
    }
    let text = result
        .error
        .iter()
        .chain(
            result
                .errors
                .iter()
                .flatten()
                .flat_map(|error| std::iter::once(&error.message).chain(error.error.iter())),
        )
        .filter(|text| !trim(text).is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    static DISK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i-u:insufficient)[^\r\n\u{2028}\u{2029}]*(?i-u:disk|space)|(?i-u:not enough)[^\r\n\u{2028}\u{2029}]*(?i-u:disk|space)|(?i-u:no space left)").unwrap()
    });
    static TIMEOUT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i-u:timed? out|timeout)").unwrap());
    let (reason, explanation) = if DISK.is_match(&text) {
        (
            "disk_space",
            "There is not enough free disk space on its device host.",
        )
    } else if TIMEOUT.is_match(&text) {
        ("timeout", "The device did not become ready in time.")
    } else {
        (
            "launch_failed",
            "The simulator or emulator could not start.",
        )
    };
    let text = diagnostics(&text);
    let summary = format!("Device {} failed to boot: {explanation}", device.id);
    let message = if text.is_empty() {
        format!("{summary} The device host sent no details. Try again or open another device; if it keeps failing, check the device on that host.")
    } else {
        format!("{summary}\n\n{text}")
    };
    let mut error = json!({"_tag":"DeviceBootError","hostId":host_id,"deviceId":device.id,"reason":reason,"message":message,"cause":result});
    if !text.is_empty() {
        error["diagnostics"] = json!(text);
    }
    Err(error)
}
pub fn shutdown_confirmed_off(discovery: &Discovery, id: &str) -> bool {
    discovery
        .devices
        .iter()
        .find(|device| device.id == id)
        .is_some_and(|device| !device.booted)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evaluate(input: &Value) -> Value {
        let mut requests = Vec::new();
        let mut runs = Vec::new();
        let mut position = 0;
        let mut fetch = |request: Request, operation: &str| -> Result<Value, Value> {
            requests.push(serde_json::to_value(request).unwrap());
            if input["timeout"] == true {
                return Err(operation_error(operation, "request_failed"));
            }
            let responses = input["responses"].as_array().unwrap();
            let response = responses
                .get(position)
                .or_else(|| responses.last())
                .unwrap();
            position += 1;
            if response["status"]
                .as_u64()
                .is_some_and(|status| !(200..300).contains(&status))
            {
                Err(operation_error(operation, "request_failed"))
            } else {
                Ok(response["body"].clone())
            }
        };
        let available_android = input["available"]["android"] == true;
        let result = (|| -> Result<Value, Value> {
            let list = |fetch: &mut dyn FnMut(Request, &str) -> Result<Value, Value>,
                        runs: &mut Vec<Value>|
             -> Result<Discovery, Value> {
                let body = fetch(list_request("http://device.test"), "list")?;
                let parsed = parse_list(body)?;
                let avds = input
                    .get("avds")
                    .cloned()
                    .unwrap_or_else(|| json!({"code":0,"stdout":"","stderr":""}));
                let avds: CommandResult = serde_json::from_value(avds).unwrap();
                if available_android {
                    runs.push(json!({"command":"emulator","args":["-list-avds"]}));
                }
                Ok(discovery(parsed, available_android, &avds))
            };
            match input["operation"].as_str().unwrap() {
                "list" => {
                    if input["timeout"] == true {
                        let _ = fetch(list_request("http://device.test"), "list");
                        Ok(serde_json::to_value(listing_timeout(
                            input["available"]["ios"] == true,
                            available_android,
                        ))
                        .unwrap())
                    } else {
                        Ok(serde_json::to_value(list(&mut fetch, &mut runs)?).unwrap())
                    }
                }
                operation => {
                    let device: Device = serde_json::from_value(input["device"].clone()).unwrap();
                    match operation {
                        "boot" => {
                            let result = parse_action(
                                fetch(boot_request("http://device.test", &device), "boot")?,
                                "boot",
                            )?;
                            let id = boot_result("local", &device, result)?;
                            if let Some(request) = attach_request("http://device.test", &device) {
                                parse_action(fetch(request, "attach stream")?, "attach stream")?;
                            }
                            Ok(json!(id))
                        }
                        "attach" => {
                            if let Some(request) = attach_request("http://device.test", &device) {
                                parse_action(fetch(request, "attach stream")?, "attach stream")?;
                            }
                            Ok(Value::Null)
                        }
                        "shutdown" => {
                            let outcome =
                                fetch(shutdown_request("http://device.test", &device), "shutdown")
                                    .and_then(|body| parse_action(body, "shutdown"))
                                    .and_then(|result| {
                                        if result.ok {
                                            Ok(())
                                        } else {
                                            Err(operation_error("shutdown", "hub_rejected"))
                                        }
                                    });
                            if let Err(error) = outcome {
                                if device.platform != Platform::Ios
                                    || !shutdown_confirmed_off(
                                        &list(&mut fetch, &mut runs)?,
                                        &device.id,
                                    )
                                {
                                    return Err(error);
                                }
                            }
                            Ok(Value::Null)
                        }
                        _ => panic!("Unknown operation"),
                    }
                }
            }
        })();
        match result {
            Ok(value) => json!({"requests":requests,"runs":runs,"value":value}),
            Err(error) => json!({"requests":requests,"runs":runs,"error":error}),
        }
    }
    #[test]
    fn pinned_complete_native_hub_operations() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-hub-4288.json"
        ))
        .unwrap();
        for family in ["list", "boot"] {
            for (index, case) in fixture[family].as_array().unwrap().iter().enumerate() {
                assert_eq!(
                    evaluate(&case["input"]),
                    case["expected"],
                    "{family} case {index}: {}",
                    case["input"]
                );
            }
        }
    }
}
