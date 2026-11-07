//! Native GitCafe/Bitbucket credential policy, independent of process and HTTP transports.
use crate::settings_secrets;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
};
type SettingsFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, ()>> + Send + 'a>>;
type HttpFuture<'a> = Pin<Box<dyn Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>>;
type ProcessFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ProcessFailure>> + Send + 'a>>;
#[derive(Clone, Copy, Debug)]
pub enum ProcessFailure {
    Missing,
    Exit,
    Other,
}
#[derive(Clone)]
pub struct Request {
    pub operation: &'static str,
    pub command: &'static str,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
    pub env: BTreeMap<String, String>,
    pub timeout_ms: u64,
}
pub trait Context: Sync {
    fn now_ms(&self) -> u64;
    fn environment(&self) -> Value;
    fn cwd(&self) -> PathBuf;
    fn settings<'a>(&'a self, kind: &'static str) -> SettingsFuture<'a>;
    fn process<'a>(&'a self, request: Request) -> ProcessFuture<'a>;
    fn bitbucket_url(&self) -> String;
    fn http<'a>(&'a self, url: String, credential: Option<BitbucketCredential>) -> HttpFuture<'a>;
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn normalized(host: &str) -> String {
    trim(host).to_lowercase()
}
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialError {
    Missing(String),
    SignedOut(String),
    Failed(String),
    Unresolved(String),
}
impl CredentialError {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Missing(_) => "GitCafeCliMissingError",
            Self::SignedOut(_) => "GitCafeNotSignedInError",
            Self::Failed(_) => "GitCafeCliFailedError",
            Self::Unresolved(_) => "SettingsSecretUnresolvedError",
        }
    }
    pub fn message(&self) -> String {
        match self{
        Self::Missing(host)=>format!("No GitCafe credential for {host}: set CAFE_TOKEN, or install the GitCafe CLI and run `cafe auth login`."),
        Self::SignedOut(host)=>format!("No GitCafe credential for {host}: run `cafe auth login --host https://{host}/api`."),
        Self::Failed(host)=>format!("The GitCafe CLI could not hand over a credential for {host}. Check `cafe auth status` on the server."),
        Self::Unresolved(message)=>message.clone(),
    }
    }
    fn ttl(&self) -> u64 {
        if matches!(self, Self::Failed(_)) {
            0
        } else {
            10_000
        }
    }
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Missing(v) | Self::SignedOut(v) | Self::Failed(v) => Some(v),
            Self::Unresolved(_) => None,
        }
    }
}
impl std::fmt::Debug for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(self.tag())
            .field("message", &self.message())
            .finish()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct CafeCredential {
    pub host: String,
    pub token: String,
    pub source: &'static str,
}
impl std::fmt::Debug for CafeCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CafeCredential")
            .field("host", &self.host)
            .field("source", &self.source)
            .field("token", &"<redacted>")
            .finish()
    }
}
struct Cached {
    expires: u64,
    value: Result<CafeCredential, CredentialError>,
}
#[derive(Default)]
pub struct CafeCredentials {
    entries: Mutex<VecDeque<(String, Cached)>>,
    gates: Mutex<BTreeMap<String, Weak<tokio::sync::Mutex<()>>>>,
}
impl CafeCredentials {
    async fn saved(
        &self,
        context: &impl Context,
        host: &str,
    ) -> Result<Option<String>, CredentialError> {
        if normalized(host) != "git.cafe" {
            return Ok(None);
        }
        let settings = context
            .settings("gitcafe")
            .await
            .unwrap_or_else(|_| json!({"sourceControlHosts":{},"secretProviders":{}}));
        let config = settings_secrets::read_host_config(
            "gitcafe",
            settings["sourceControlHosts"]["gitcafe"].clone(),
        )
        .expect("pinned GitCafe schema");
        let token =
            settings_secrets::require_literal(&config["token"], &settings["secretProviders"])
                .map_err(CredentialError::Unresolved)?;
        Ok((!token.is_empty()).then_some(token))
    }
    pub async fn cli_env(
        &self,
        context: &impl Context,
        host: &str,
    ) -> Result<Value, CredentialError> {
        Ok(match self.saved(context, host).await? {
            Some(token) => json!({"CAFE_TOKEN":token}),
            None => json!({}),
        })
    }
    pub fn invalidate(&self, host: &str) {
        let key = normalized(host);
        self.entries
            .lock()
            .unwrap()
            .retain(|(host, _)| host != &key);
    }
    pub async fn get(
        &self,
        context: &impl Context,
        host: &str,
    ) -> Result<CafeCredential, CredentialError> {
        if let Some(token) = self.saved(context, host).await? {
            return Ok(CafeCredential {
                host: normalized(host),
                token,
                source: "settings",
            });
        }
        let host = normalized(host);
        let gate = {
            let mut gates = self.gates.lock().unwrap();
            gates.retain(|_, v| v.strong_count() > 0);
            if let Some(gate) = gates.get(&host).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(host.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let _guard = gate.lock().await;
        let now = context.now_ms();
        {
            let mut entries = self.entries.lock().unwrap();
            if let Some(at) = entries.iter().position(|(name, _)| name == &host) {
                let (name, entry) = entries.remove(at).unwrap();
                if entry.expires > now {
                    let value = entry.value.clone();
                    entries.push_back((name, entry));
                    return value;
                }
            }
        }
        let result = if let Some(token) = environment_token(&host, &context.environment()) {
            Ok(CafeCredential {
                host: host.clone(),
                token,
                source: "env",
            })
        } else {
            let request = Request {
                operation: "GitCafeCredentials.get",
                command: "cafe",
                args: vec![
                    "auth".into(),
                    "http".into(),
                    "credential".into(),
                    "--host".into(),
                    format!("https://{host}/api"),
                    "get".into(),
                ],
                cwd: context.cwd(),
                stdin: format!("protocol=https\nhost={host}\n\n"),
                env: BTreeMap::from([("CAFE_NO_UPDATE_CHECK".into(), "1".into())]),
                timeout_ms: 10_000,
            };
            match context.process(request).await {
                Ok(output) => credential_password(&output)
                    .map(|token| CafeCredential {
                        host: host.clone(),
                        token,
                        source: "cafe",
                    })
                    .ok_or_else(|| CredentialError::SignedOut(host.clone())),
                Err(failure) => Err(match failure {
                    ProcessFailure::Missing => CredentialError::Missing(host.clone()),
                    ProcessFailure::Exit => CredentialError::SignedOut(host.clone()),
                    ProcessFailure::Other => CredentialError::Failed(host.clone()),
                }),
            }
        };
        let ttl = result
            .as_ref()
            .map(|_| 300_000)
            .unwrap_or_else(CredentialError::ttl);
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= 4 {
            entries.pop_front();
        }
        entries.push_back((
            host,
            Cached {
                expires: context.now_ms().saturating_add(ttl),
                value: result.clone(),
            },
        ));
        result
    }
}
pub fn environment_token(host: &str, env: &Value) -> Option<String> {
    let configured = env["CAFE_HOST"].as_str().map(trim).unwrap_or("");
    let configured = if configured.is_empty() {
        "git.cafe".to_owned()
    } else {
        let value = if configured.contains("://") {
            configured.to_owned()
        } else {
            format!("https://{configured}")
        };
        let url = url::Url::parse(&value).ok()?;
        let host = url.host_str()?;
        match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        }
    };
    if configured.to_lowercase() != normalized(host) {
        return None;
    }
    env["CAFE_TOKEN"]
        .as_str()
        .map(trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
pub fn credential_password(output: &str) -> Option<String> {
    for line in output.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("password=") {
            return (!trim(value).is_empty()).then(|| trim(value).to_owned());
        }
    }
    None
}
#[derive(Clone, PartialEq, Eq)]
pub enum BitbucketCredential {
    Access(String),
    Api { email: String, token: String },
}
impl std::fmt::Debug for BitbucketCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BitbucketCredential(<redacted>)")
    }
}
fn header_safe(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|v| (0x21..=0x7e).contains(&v))
}
fn from_fields(access: &str, email: &str, api: &str) -> Option<BitbucketCredential> {
    if header_safe(access) {
        Some(BitbucketCredential::Access(access.into()))
    } else if header_safe(email) && header_safe(api) {
        Some(BitbucketCredential::Api {
            email: email.into(),
            token: api.into(),
        })
    } else {
        None
    }
}
pub async fn bitbucket_credential(
    context: &impl Context,
) -> Result<Option<BitbucketCredential>, CredentialError> {
    let settings = context
        .settings("bitbucket")
        .await
        .unwrap_or_else(|_| json!({"sourceControlHosts":{},"secretProviders":{}}));
    let fields = settings_secrets::read_host_config(
        "bitbucket",
        settings["sourceControlHosts"]["bitbucket"].clone(),
    )
    .expect("pinned Bitbucket schema");
    let access =
        settings_secrets::require_literal(&fields["accessToken"], &settings["secretProviders"])
            .map_err(CredentialError::Unresolved)?;
    let api = if header_safe(&access) {
        String::new()
    } else {
        settings_secrets::require_literal(&fields["apiToken"], &settings["secretProviders"])
            .map_err(CredentialError::Unresolved)?
    };
    let env = context.environment();
    Ok(
        from_fields(&access, fields["email"].as_str().unwrap_or(""), &api).or_else(|| {
            from_fields(
                env["T3CODE_BITBUCKET_ACCESS_TOKEN"].as_str().unwrap_or(""),
                env["T3CODE_BITBUCKET_EMAIL"].as_str().unwrap_or(""),
                env["T3CODE_BITBUCKET_API_TOKEN"].as_str().unwrap_or(""),
            )
        }),
    )
}
fn option(value: Option<&str>) -> Value {
    value
        .map(|v| json!({"_tag":"Some","value":v}))
        .unwrap_or_else(|| json!({"_tag":"None"}))
}
pub fn bitbucket_auth(credential: Option<&BitbucketCredential>) -> Value {
    match credential {
        Some(BitbucketCredential::Access(_)) => {
            json!({"status":"unknown","account":option(None),"host":option(Some("bitbucket.org")),"detail":option(Some("An access token is configured."))})
        }
        Some(BitbucketCredential::Api { email, .. }) => {
            json!({"status":"unknown","account":option(Some(email)),"host":option(Some("bitbucket.org")),"detail":option(Some("An API token is configured."))})
        }
        None => {
            json!({"status":"unauthenticated","account":option(None),"host":option(Some("bitbucket.org")),"detail":option(Some("Add a Bitbucket token in Settings → Source Control, or set the T3CODE_BITBUCKET_* environment variables on the server."))})
        }
    }
}

impl BitbucketCredential {
    pub fn authorization(&self) -> String {
        use base64::Engine;
        match self {
            Self::Access(token) => format!("Bearer {token}"),
            Self::Api { email, token } => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"))
            ),
        }
    }
}
fn user_auth(bytes: &[u8]) -> Option<Value> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let value: Value = serde_json::from_str(&String::from_utf8_lossy(bytes)).ok()?;
    let object = value.as_object()?;
    let mut account = None;
    for key in ["username", "display_name", "account_id"] {
        if let Some(value) = object.get(key).filter(|value| !value.is_null()) {
            let text = trim(value.as_str()?);
            if text.is_empty() {
                return None;
            }
            if account.is_none() {
                account = Some(text);
            }
        }
    }
    Some(
        json!({"status":"authenticated","account":option(account),"host":option(Some("bitbucket.org")),"detail":option(None)}),
    )
}
fn unresolved_auth(error: CredentialError) -> Value {
    json!({"status":"unauthenticated","account":option(None),"host":option(Some("bitbucket.org")),"detail":option(Some(&error.message()))})
}
/// Probe follows the native fresh-read boundaries: admission, request header,
/// and (on failure) credential-only status projection all read independently.
pub async fn bitbucket_probe(context: &impl Context) -> Value {
    match bitbucket_credential(context).await {
        Err(error) => unresolved_auth(error),
        Ok(None) => bitbucket_auth(None),
        Ok(Some(_)) => {
            let request = match bitbucket_credential(context).await {
                Ok(credential) => context.http(context.bitbucket_url(), credential).await,
                Err(_) => Err(()),
            };
            if let Ok((status, bytes)) = request {
                if (200..300).contains(&status) {
                    if let Some(auth) = user_auth(&bytes) {
                        return auth;
                    }
                }
            }
            match bitbucket_credential(context).await {
                Ok(credential) => bitbucket_auth(credential.as_ref()),
                Err(error) => unresolved_auth(error),
            }
        }
    }
}
