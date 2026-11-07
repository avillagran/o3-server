//! Pinned GitCafe request policy. HTTP and credentials are injected independently.
use crate::source_credentials::{CafeCredential, CredentialError};
use serde_json::Value;
use std::{future::Future, pin::Pin};
type CredentialFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CafeCredential, CredentialError>> + Send + 'a>>;
type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<Response, ()>> + Send + 'a>>;
type InvalidateFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
#[derive(Clone, serde::Deserialize)]
pub struct Request {
    pub host: String,
    pub operation: String,
    pub path: String,
    pub method: Option<Method>,
    #[serde(default, deserialize_with = "body")]
    pub body: Option<Value>,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}
impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}
// Request bodies and response text deliberately do not implement Debug.
fn body<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<Value>, D::Error> {
    serde::Deserialize::deserialize(value).map(Some)
}
pub struct Outgoing {
    pub url: String,
    pub method: Method,
    pub body: Option<Value>,
    pub token: String,
}
pub struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub text: String,
}
pub trait Context: Sync {
    fn credential<'a>(&'a self, host: &'a str) -> CredentialFuture<'a>;
    fn invalidate<'a>(&'a self, host: &'a str) -> InvalidateFuture<'a>;
    fn send<'a>(&'a self, request: Outgoing) -> SendFuture<'a>;
}
pub struct ApiError {
    pub host: String,
    pub operation: String,
    pub status: Option<u16>,
    pub reason: &'static str,
    detail: Vec<u16>,
    pub credential: Option<CredentialError>,
}
impl std::fmt::Debug for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCafeApiError")
            .field("status", &self.status)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl ApiError {
    pub fn detail_units(&self) -> &[u16] {
        &self.detail
    }
    pub fn message_units(&self) -> Vec<u16> {
        format!("GitCafe {} failed on {}: ", self.operation, self.host)
            .encode_utf16()
            .chain(self.detail.iter().copied())
            .collect()
    }
    /// Rendering an isolated UTF-16 surrogate requires replacement in Rust's UTF-8 strings.
    pub fn detail_lossy(&self) -> String {
        String::from_utf16_lossy(&self.detail)
    }
}
fn whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn host(value: &str) -> Option<String> {
    let value = value.trim_matches(whitespace).to_lowercase();
    matches!(value.as_str(), "git.cafe" | "staging.git.cafe").then_some(value)
}
fn problem(text: &str) -> Option<Vec<u16>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    for key in ["title", "detail"] {
        if let Some(value) = object.get(key) {
            if !value.is_string() {
                return None;
            }
        }
    }
    let text = object
        .get("detail")
        .and_then(Value::as_str)
        .or_else(|| object.get("title").and_then(Value::as_str))?;
    let mut normalized = String::new();
    let mut space = false;
    for character in text.chars() {
        if whitespace(character) {
            space = !normalized.is_empty();
        } else {
            if space {
                normalized.push(' ');
                space = false;
            }
            normalized.push(character);
        }
    }
    if normalized.is_empty() {
        return None;
    }
    let mut units = normalized.encode_utf16().collect::<Vec<_>>();
    if units.len() > 300 {
        units.truncate(300);
        units.push(0x2026);
    }
    Some(units)
}
fn redirect(method: Method, origin: &str, from: &str, response: &Response) -> Option<String> {
    if !matches!(response.status, 301 | 302 | 303 | 307 | 308)
        || (!matches!(method, Method::Get) && !matches!(response.status, 307 | 308))
    {
        return None;
    }
    let target = url::Url::parse(from)
        .ok()?
        .join(response.location.as_deref()?)
        .ok()?;
    (target.origin().ascii_serialization() == origin
        && target.username().is_empty()
        && target.password().is_none()
        && target.path().starts_with("/api/"))
    .then(|| target.to_string())
}
pub async fn request(context: &impl Context, input: &Request) -> Result<String, ApiError> {
    let fail = |status, reason, detail: Vec<u16>, credential| ApiError {
        host: input.host.clone(),
        operation: input.operation.clone(),
        status,
        reason,
        detail,
        credential,
    };
    let Some(host) = host(&input.host) else {
        return Err(fail(
            None,
            "failed",
            format!(
                "GitCafe lives on git.cafe and staging.git.cafe, not {}.",
                input.host
            )
            .encode_utf16()
            .collect(),
            None,
        ));
    };
    let credential = context.credential(&host).await.map_err(|error| {
        let reason = match error {
            CredentialError::Missing(_) => "missing-tool",
            CredentialError::SignedOut(_) | CredentialError::Unresolved(_) => "unauthenticated",
            CredentialError::Failed(_) => "failed",
        };
        fail(
            None,
            reason,
            error.message().encode_utf16().collect(),
            Some(error),
        )
    })?;
    let origin = format!("https://{host}");
    let method = input.method.unwrap_or(Method::Get);
    let mut url = format!("{origin}/api{}", input.path);
    let token = &credential.token;
    let send = |url: String| async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            context.send(Outgoing {
                url,
                method,
                body: input.body.clone(),
                token: token.clone(),
            }),
        )
        .await
        .map_err(|_| ())?
    };
    let network = || {
        fail(
            None,
            "failed",
            "Could not reach GitCafe.".encode_utf16().collect(),
            None,
        )
    };
    let mut response = send(url.clone()).await.map_err(|_| network())?;
    for _ in 0..3 {
        let Some(next) = redirect(method, &origin, &url, &response) else {
            break;
        };
        url = next;
        response = send(url.clone()).await.map_err(|_| network())?;
    }
    if (200..300).contains(&response.status) {
        return Ok(response.text);
    }
    if response.status == 401 {
        context.invalidate(&host).await;
    }
    let reason = match response.status {
        401 => "unauthenticated",
        429 => "rate-limited",
        404 => "not-found",
        _ => "failed",
    };
    Err(fail(
        Some(response.status),
        reason,
        problem(&response.text).unwrap_or_else(|| {
            format!("GitCafe answered HTTP {}.", response.status)
                .encode_utf16()
                .collect()
        }),
        None,
    ))
}
