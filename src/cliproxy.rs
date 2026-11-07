//! Native management API validation and reset-credit redemption through a configured hub.
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::sync::LazyLock;

static PROGRAM: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("cliproxy-codec-4288.json"))
        .expect("native hub schema program")
});
type Decoded = Result<Option<Value>, ()>;
fn index(value: &Value) -> usize {
    value.as_u64().unwrap() as usize
}
fn decode_node(id: usize, input: Option<&Value>) -> Decoded {
    let node = &PROGRAM["nodes"][id];
    if input.is_none() && node["optional"] == true {
        return Ok(None);
    }
    let mut input = input.cloned();
    if let Some(codecs) = node["codecs"].as_array() {
        for codec in codecs.iter().rev() {
            input = decode_node(index(&codec["from"]), input.as_ref())?;
            if codec["transform"] == "trim" {
                input = Some(json!(crate::native_goal::trim(
                    input.as_ref().and_then(Value::as_str).ok_or(())?
                )));
            } else if codec["transform"] != "identity" {
                return Err(());
            }
        }
    }
    let output = match node["tag"].as_str().ok_or(())? {
        "Unknown" => input,
        "Undefined" if input.is_none() => None,
        "Null" if input.as_ref().is_some_and(Value::is_null) => Some(Value::Null),
        "String" if input.as_ref().is_some_and(Value::is_string) => input,
        "Number" if input.as_ref().is_some_and(Value::is_number) => input,
        "Boolean" if input.as_ref().is_some_and(Value::is_boolean) => input,
        "Literal" if input.as_ref() == Some(&node["literal"]) => input,
        "Union" => {
            let mut found = None;
            for child in node["types"].as_array().ok_or(())? {
                if let Ok(value) = decode_node(index(child), input.as_ref()) {
                    found = Some(value);
                    break;
                }
            }
            found.ok_or(())?
        }
        "Arrays" => {
            let values = input.as_ref().and_then(Value::as_array).ok_or(())?;
            if node["elements"].as_array().is_none_or(|v| !v.is_empty())
                || node["rest"].as_array().is_none_or(|v| v.len() != 1)
            {
                return Err(());
            }
            let child = index(&node["rest"][0]);
            Some(Value::Array(
                values
                    .iter()
                    .map(|value| decode_node(child, Some(value))?.ok_or(()))
                    .collect::<Result<_, ()>>()?,
            ))
        }
        "Objects" => {
            let values = input.as_ref().and_then(Value::as_object).ok_or(())?;
            let mut output = serde_json::Map::new();
            for field in node["properties"].as_array().ok_or(())? {
                let name = field["name"].as_str().ok_or(())?;
                if let Some(value) = decode_node(index(&field["type"]), values.get(name))? {
                    output.insert(name.into(), value);
                }
            }
            Some(Value::Object(output))
        }
        _ => return Err(()),
    };
    if let Some(checks) = node["checks"].as_array() {
        for check in checks {
            let value = output.as_ref().and_then(Value::as_str).ok_or(())?;
            let valid = match check["kind"].as_str() {
                Some("nonBlank") => !crate::native_goal::trim(value).is_empty(),
                Some("isMaxLength") => value.encode_utf16().count() <= index(&check["maxLength"]),
                Some("isPattern") if check["flags"] == "" => {
                    regex::Regex::new(check["source"].as_str().ok_or(())?)
                        .map_err(|_| ())?
                        .is_match(value)
                }
                _ => false,
            };
            if !valid {
                return Err(());
            }
        }
    }
    Ok(output)
}
fn decode(root: &str, value: &Value) -> Result<Value, ()> {
    decode_node(index(&PROGRAM["roots"][root]), Some(value))?.ok_or(())
}
pub(crate) fn decode_reset_input(value: &Value) -> Result<Value, ()> {
    decode("ResetInput", value)
}

#[derive(Debug, PartialEq)]
pub(crate) enum ApiError {
    Invalid,
    Detail(String),
}
impl ApiError {
    pub(crate) fn detail(self) -> String {
        match self {
            Self::Invalid => "The hub returned an unexpected reset-credit response.".into(),
            Self::Detail(detail) => detail,
        }
    }
}
fn redemption_id(account: &str, credit: &str) -> String {
    let mut hash = Sha1::new();
    hash.update([
        0x6f, 0x1c, 0x2a, 0x9e, 0x2d, 0x4b, 0x4c, 0x1e, 0x9a, 0x7f, 0x3b, 0x8d, 0x5e, 0x0c, 0x1a,
        0x42,
    ]);
    hash.update(format!("{account}:{credit}"));
    let mut bytes = hash.finalize();
    bytes[6] = (bytes[6] & 15) | 0x50;
    bytes[8] = (bytes[8] & 63) | 0x80;
    let hex = bytes[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}
fn management_url(base: &str, path: &str) -> Result<url::Url, ApiError> {
    let mut base =
        url::Url::parse(base).map_err(|_| ApiError::Detail("The hub URL is not valid.".into()))?;
    let normalized = regex::Regex::new(r"(?i)(/management\.html)?/?$")
        .unwrap()
        .replace(base.path(), "/")
        .into_owned();
    base.set_path(&normalized);
    base.join(&format!("v0/management/{path}"))
        .map_err(|_| ApiError::Detail("The hub URL is not valid.".into()))
}
fn transport_detail(code: Option<&str>) -> String {
    let code = code.filter(|code| {
        code.len() <= 64
            && !code.is_empty()
            && code
                .bytes()
                .all(|v| v.is_ascii_uppercase() || v.is_ascii_digit() || v == b'_')
    });
    match code{
        Some(code @ ("DEPTH_ZERO_SELF_SIGNED_CERT"|"SELF_SIGNED_CERT_IN_CHAIN"|"UNABLE_TO_GET_ISSUER_CERT_LOCALLY"|"UNABLE_TO_VERIFY_LEAF_SIGNATURE"))=>format!("The hub's TLS certificate is not trusted ({code}). If it comes from a private CA, add that CA to NODE_EXTRA_CA_CERTS."),
        Some(code)=>format!("The hub request failed ({code})."),
        None=>"The hub management request failed.".into(),
    }
}
fn request_error(error: &reqwest::Error) -> ApiError {
    use std::error::Error;
    if error.is_timeout() {
        return ApiError::Detail("The hub did not answer within 15 seconds.".into());
    }
    let mut cause = error.source();
    let mut code = None;
    while let Some(error) = cause {
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            code = match error.kind() {
                std::io::ErrorKind::ConnectionRefused => Some("ECONNREFUSED"),
                std::io::ErrorKind::ConnectionReset => Some("ECONNRESET"),
                std::io::ErrorKind::ConnectionAborted => Some("ECONNABORTED"),
                std::io::ErrorKind::NetworkUnreachable => Some("ENETUNREACH"),
                std::io::ErrorKind::HostUnreachable => Some("EHOSTUNREACH"),
                _ => None,
            };
            if code.is_some() {
                break;
            }
        }
        cause = error.source();
    }
    ApiError::Detail(transport_detail(code))
}
async fn management(
    base: &str,
    key: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, ApiError> {
    let url = management_url(base, path)?;
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15));
    if let Some(certificates) = std::env::var_os("NODE_EXTRA_CA_CERTS")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| reqwest::Certificate::from_pem_bundle(&bytes).ok())
    {
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    let client = builder
        .build()
        .map_err(|_| ApiError::Detail(transport_detail(None)))?;
    let request = if body.is_some() {
        client.post(url)
    } else {
        client.get(url)
    };
    let request = request.bearer_auth(key);
    let request = if let Some(body) = body {
        request.json(&body)
    } else {
        request
    };
    let mut response = request
        .send()
        .await
        .map_err(|error| request_error(&error))?;
    if !response.status().is_success() {
        return Err(ApiError::Detail(format!(
            "The hub answered with HTTP {}.",
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| request_error(&error))?
    {
        if bytes.len() + chunk.len() > 1_048_576 {
            return Err(ApiError::Detail(transport_detail(None)));
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).map_err(|_| ApiError::Detail(transport_detail(None)))
}
async fn auth_files(base: &str, key: &str) -> Result<Vec<Value>, ApiError> {
    let raw = management(base, key, "auth-files", None).await?;
    let decoded = decode("AuthFiles", &raw).map_err(|_| ApiError::Invalid)?;
    Ok(decoded["files"].as_array().unwrap().clone())
}
pub(crate) async fn list_accounts(base: &str, key: &str) -> Result<Vec<Value>, ApiError> {
    auth_files(base, key).await.map_err(|error| match error {
        ApiError::Invalid => ApiError::Detail("The hub could not list accounts.".into()),
        error => error,
    })
}

fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.abs() >= 1e21 || value.abs() < 1e-6 {
        let exponential = format!("{value:e}");
        let (mantissa, exponent) = exponential.split_once('e').unwrap();
        let exponent: i32 = exponent.parse().unwrap();
        format!(
            "{mantissa}e{}{exponent}",
            if exponent >= 0 { "+" } else { "" }
        )
    } else {
        value.to_string()
    }
}

async fn api_call(
    base: &str,
    key: &str,
    account: &Value,
    url: &str,
    data: Option<Value>,
) -> Result<String, ApiError> {
    let mut header = if account["provider"] == "codex" {
        json!({"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","OpenAI-Beta":"codex-1","Originator":"Codex Desktop"})
    } else {
        json!({"Authorization":"Bearer $TOKEN$","anthropic-beta":"oauth-2025-04-20","User-Agent":"claude-code/2.1.273"})
    };
    if let Some(id) = (account["provider"] == "codex")
        .then(|| account["id_token"]["chatgpt_account_id"].as_str())
        .flatten()
        .filter(|id| !id.is_empty())
    {
        header["Chatgpt-Account-Id"] = json!(id);
    }
    let mut request = json!({"auth_index":account["auth_index"],"method":if data.is_some(){"POST"}else{"GET"},"url":url,"header":header});
    if let Some(data) = data {
        request["data"] = json!(data.to_string());
    }
    let raw = management(base, key, "api-call", Some(request)).await?;
    let response = decode("ApiResponse", &raw).map_err(|_| ApiError::Invalid)?;
    let status = response["status_code"].as_f64().unwrap();
    if !(200.0..300.0).contains(&status) {
        return Err(ApiError::Detail(format!(
            "The provider refused the hub request (HTTP {}).",
            js_number(status)
        )));
    }
    Ok(response["body"].as_str().unwrap().into())
}
pub(crate) async fn usage(base: &str, key: &str, account: &Value) -> Result<Value, ApiError> {
    let codex = account["provider"] == "codex";
    let body = api_call(
        base,
        key,
        account,
        if codex {
            "https://chatgpt.com/backend-api/wham/usage"
        } else {
            "https://api.anthropic.com/api/oauth/usage"
        },
        None,
    )
    .await?;
    let raw: Value = serde_json::from_str(&body).map_err(|_| ApiError::Invalid)?;
    decode(if codex { "CodexUsage" } else { "ClaudeUsage" }, &raw).map_err(|_| ApiError::Invalid)
}

fn expiry(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(value))
        .ok()
        .map(|value| value.timestamp_millis())
}
pub(crate) async fn credits(
    base: &str,
    key: &str,
    account: &Value,
    now: i64,
) -> Result<Vec<Value>, ApiError> {
    let body = api_call(
        base,
        key,
        account,
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits",
        None,
    )
    .await?;
    let raw: Value = serde_json::from_str(&body).map_err(|_| ApiError::Invalid)?;
    let list = decode("CreditList", &raw).map_err(|_| ApiError::Invalid)?;
    let mut available = list["credits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|credit| {
            let at = expiry(credit["expires_at"].as_str().unwrap())?;
            (credit["status"] == "available"
                && credit["reset_type"] == "codex_rate_limits"
                && at > now)
                .then(|| (at, credit.clone()))
        })
        .collect::<Vec<_>>();
    available.sort_by_key(|(at, _)| *at);
    Ok(available.into_iter().map(|(_, credit)| credit).collect())
}

pub(crate) async fn consume(
    base: &str,
    key: &str,
    account_id: &str,
    credit_id: &str,
) -> Result<Value, ApiError> {
    let files = auth_files(base, key).await?;
    let account = files.iter().find(|account| account["id"] == account_id);
    let account = account
        .filter(|account| account["disabled"] != true && account["provider"] == "codex")
        .ok_or_else(|| ApiError::Detail("The Codex hub account is missing or disabled.".into()))?;
    let owner = account["id_token"]["chatgpt_account_id"]
        .as_str()
        .unwrap_or(account_id);
    let body = api_call(
        base,
        key,
        account,
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume",
        Some(json!({"redeem_request_id":redemption_id(owner,credit_id),"credit_id":credit_id})),
    )
    .await?;
    let response: Value = serde_json::from_str(&body).map_err(|_| ApiError::Invalid)?;
    let response = decode("ConsumeResponse", &response).map_err(|_| ApiError::Invalid)?;
    let outcome = match response["code"].as_str().unwrap() {
        "reset" => "reset",
        "nothing_to_reset" => "nothingToReset",
        "no_credit" => "noCredit",
        "already_redeemed" => "alreadyRedeemed",
        _ => return Err(ApiError::Invalid),
    };
    let mut result = json!({"outcome":outcome});
    if matches!(outcome, "reset" | "alreadyRedeemed")
        && management(
            base,
            key,
            "reset-quota",
            Some(json!({"auth_index":account["auth_index"]})),
        )
        .await
        .is_err()
    {
        result["warning"]=json!("Credit redeemed, but the hub cooldown could not be cleared. Routing may resume after its cooldown expires.");
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/support/cliproxy.rs"]
mod tests;
