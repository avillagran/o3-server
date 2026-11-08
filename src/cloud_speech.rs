//! Source-pinned cloud transcription. HTTP authority and private settings remain injected owners.
pub mod text;
use crate::{
    secret_resolver::{SecretReference, SecretResolver},
    speech_kernel::{self, Pcm},
    speech_request::TranscriptionOptions,
};
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
pub type Task<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Xai,
    OpenRouter,
}
impl Provider {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "openai" => Self::OpenAi,
            "xai" => Self::Xai,
            "openrouter" => Self::OpenRouter,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Xai => "xai",
            Self::OpenRouter => "openrouter",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    MissingKey,
    KeyChanged,
    UnreadableKey,
    RejectedKey,
    RateLimit,
    ModelUnavailable,
    Unavailable,
}
impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingKey => "missing-key",
            Self::KeyChanged => "key-changed",
            Self::UnreadableKey => "unreadable-key",
            Self::RejectedKey => "rejected-key",
            Self::RateLimit => "rate-limit",
            Self::ModelUnavailable => "model-unavailable",
            Self::Unavailable => "unavailable",
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub enum Error {
    Cloud { provider: Provider, reason: Reason },
    InvalidAudio { bytes: usize },
    Settings,
    Cancelled,
    CleanupUnknown,
}
impl Error {
    pub fn message(&self) -> String {
        let Self::Cloud { provider, reason } = self else {
            return match self {
                Self::InvalidAudio { .. } => "Invalid PCM audio.",
                Self::Settings => "Could not read private speech settings.",
                Self::Cancelled => "Speech operation cancelled.",
                _ => "Owned speech cleanup remains unknown.",
            }
            .into();
        };
        let name = match provider {
            Provider::OpenAi => "OpenAI",
            Provider::Xai => "xAI",
            Provider::OpenRouter => "OpenRouter",
        };
        match reason {
            Reason::MissingKey=>format!("Add your {name} API key in Settings → Voice."),
            Reason::KeyChanged=>format!("The {name} key changed before your recording was sent. Try again."),
            Reason::UnreadableKey=>format!("Couldn’t read the {name} API key. Unlock the vault on the selected environment or update the key in Settings → Voice. No audio was sent to {name}."),
            Reason::RejectedKey=>format!("{name} rejected this API key. Check its permissions and billing in Settings → Voice."),
            Reason::RateLimit=>format!("{name} reached a usage limit. Check the account's quota or try again later."),
            Reason::ModelUnavailable=>format!("The chosen model isn’t available on {name}. Choose another model in Settings → Voice."),
            Reason::Unavailable=>format!("{name} couldn’t transcribe this recording. Try again later."),
        }
    }
    pub fn encoded(&self) -> Value {
        match self {
            Self::Cloud { provider, reason } => {
                json!({"_tag":"CloudSpeechError","provider":provider.as_str(),"reason":reason.as_str()})
            }
            Self::InvalidAudio { bytes } => {
                json!({"_tag":"SpeechInvalidAudioError","byteLength":bytes,"message":"Invalid PCM audio."})
            }
            Self::Settings => json!({"_tag":"ServerSettingsError"}),
            Self::Cancelled => json!({"_tag":"Cancelled"}),
            Self::CleanupUnknown => json!({"_tag":"CleanupUnknown"}),
        }
    }
}
impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.encoded())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}
impl std::error::Error for Error {}
fn failed(provider: Provider, reason: Reason) -> Error {
    Error::Cloud { provider, reason }
}
pub struct Snapshot {
    pub credentials: Value,
    pub secret_providers: Value,
}
/// Every read is a fresh, coherent effective/private snapshot, not the public redacted DTO.
pub trait Settings: Send + Sync {
    fn check(&self) -> Result<(), Error>;
    fn read(&self) -> Task<'_, Result<Snapshot, Error>>;
    fn patch<'a>(&'a self, patch: Value) -> Task<'a, Result<(), Error>>;
}
/// No Debug implementation: only the opaque provider identity, never the key, is compared.
pub struct ResolvedSecret {
    pub value: String,
    pub identity: Option<String>,
}
pub trait Secrets: Send + Sync {
    fn identity(&self, _providers: &Value, _reference: &SecretReference) -> Option<String> {
        None
    }
    fn read<'a>(
        &'a self,
        providers: &'a Value,
        reference: &'a SecretReference,
    ) -> Task<'a, Option<ResolvedSecret>>;
}
pub struct Resolver {
    resolver: Arc<SecretResolver>,
    providers: Arc<dyn Fn() -> Value + Send + Sync>,
}
impl Resolver {
    pub fn new(
        resolver: Arc<SecretResolver>,
        providers: Arc<dyn Fn() -> Value + Send + Sync>,
    ) -> Self {
        Self {
            resolver,
            providers,
        }
    }
}
impl Secrets for Resolver {
    fn identity(&self, providers: &Value, reference: &SecretReference) -> Option<String> {
        self.resolver.identity(providers, reference)
    }
    fn read<'a>(
        &'a self,
        _providers: &'a Value,
        reference: &'a SecretReference,
    ) -> Task<'a, Option<ResolvedSecret>> {
        Box::pin(async move {
            let references = [reference.clone()];
            self.resolver.invalidate(Some(&references)).await;
            let providers = (self.providers)();
            self.resolver
                .read_with_identity(&providers, &references)
                .await
                .into_iter()
                .next()
                .and_then(|receipt| {
                    receipt.value.map(|value| ResolvedSecret {
                        value,
                        identity: receipt.identity,
                    })
                })
        })
    }
}
pub trait Text: Send + Sync {
    fn prepare(&self) -> Task<'_, Result<(), Error>>;
    fn cloud<'a>(
        &'a self,
        text: &'a str,
        options: &'a TranscriptionOptions,
        language: &'a str,
    ) -> Task<'a, Result<String, Error>>;
    fn shutdown(&self) -> Task<'_, Result<(), Error>>;
}
#[derive(Clone, Copy)]
pub enum Endpoint {
    Catalog,
    Transcribe(Provider),
}
impl Endpoint {
    pub fn url(self) -> &'static str {
        match self {
            Self::Catalog => {
                "https://openrouter.ai/api/v1/models?output_modalities=transcription&zdr=true"
            }
            Self::Transcribe(Provider::OpenAi) => "https://api.openai.com/v1/audio/transcriptions",
            Self::Transcribe(Provider::Xai) => "https://api.x.ai/v1/stt",
            Self::Transcribe(Provider::OpenRouter) => {
                "https://openrouter.ai/api/v1/audio/transcriptions"
            }
        }
    }
}
pub enum Part {
    Field {
        key: String,
        value: String,
    },
    File {
        key: &'static str,
        name: &'static str,
        mime: &'static str,
        bytes: Vec<u8>,
    },
}
pub enum Body {
    Empty,
    Multipart(Vec<Part>),
    Json(Value),
}
/// No Debug implementation: neither the bearer nor audio is a loggable descriptor.
pub struct Request {
    pub endpoint: Endpoint,
    pub bearer: Option<String>,
    pub body: Body,
}
pub struct Response {
    pub status: u16,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportError;
pub trait Transport: Send + Sync {
    fn execute(&self, request: Request) -> Task<'_, Result<Response, TransportError>>;
}
/// A pinned endpoint transport. There is no wire URL, redirect follow, or ambient auth.
pub struct Http {
    client: reqwest::Client,
    loopback: Option<std::net::SocketAddr>,
}
impl Http {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            loopback: None,
        })
    }
}
impl Http {
    /// Explicit application-owner injection for an owned private provider peer.
    /// Production startup always uses `new`; no settings, CLI, HTTP, or MCP URL decoder exists.
    pub fn loopback_peer(address: std::net::SocketAddr) -> Result<Self, TransportError> {
        if !address.ip().is_loopback() {
            return Err(TransportError);
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| TransportError)?,
            loopback: Some(address),
        })
    }
    fn url(&self, endpoint: Endpoint) -> String {
        match self.loopback {
            None => endpoint.url().into(),
            Some(address) => {
                let url = url::Url::parse(endpoint.url()).expect("compiled provider URL");
                format!(
                    "http://{address}{}{}",
                    url.path(),
                    url.query()
                        .map(|query| format!("?{query}"))
                        .unwrap_or_default()
                )
            }
        }
    }
}
impl Transport for Http {
    fn execute(&self, request: Request) -> Task<'_, Result<Response, TransportError>> {
        Box::pin(async move {
            async move {
        let mut builder=match request.endpoint {Endpoint::Catalog=>self.client.get(self.url(request.endpoint)),Endpoint::Transcribe(_)=>self.client.post(self.url(request.endpoint))};
        if let Some(key)=request.bearer {builder=builder.bearer_auth(key);}
        builder=match request.body {Body::Empty=>builder,Body::Json(value)=>builder.json(&value),Body::Multipart(parts)=>{
            let boundary=format!("o3-{}",crate::store::random_id().map_err(|_|())?);
            let mut chunks=Vec::new();
            for part in parts {
                match part {
                    Part::Field{key,value}=>{
                        if !key.bytes().all(|byte|byte.is_ascii_alphanumeric()||matches!(byte,b'['|b']'|b'_')){return Err(());}
                        let normalized=value.replace("\r\n","\n").replace('\r',"\n").replace('\n',"\r\n");
                        chunks.push(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{normalized}\r\n").into_bytes());
                    }
                    Part::File{key,name,mime,bytes}=>{
                        if key!="file"||name!="dictation.wav"||mime!="audio/wav"{return Err(());}
                        chunks.push(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes());
                        chunks.push(bytes);chunks.push(b"\r\n".to_vec());
                    }
                }
            }
            chunks.push(format!("--{boundary}--\r\n").into_bytes());
            let length=chunks.iter().try_fold(0usize,|length,chunk|length.checked_add(chunk.len())).ok_or(())?;
            builder.header(reqwest::header::CONTENT_TYPE,format!("multipart/form-data; boundary={boundary}")).header(reqwest::header::CONTENT_LENGTH,length).body(reqwest::Body::wrap(http_body_util::StreamBody::new(futures_util::stream::iter(chunks.into_iter().map(|chunk|Ok::<_,std::convert::Infallible>(hyper::body::Frame::data(hyper::body::Bytes::from(chunk))))))))
        }};
        let mut response=builder.send().await.map_err(|_|())?;let status=response.status().as_u16();
        // Error diagnostics are never read or returned. This is not an automatic retry path.
        if !(200..300).contains(&status){return Ok(Response{status,bytes:vec![]});}
        let mut bytes=Vec::new();
        while let Some(part)=response.chunk().await.map_err(|_|())? {if bytes.len().saturating_add(part.len())>8*1024*1024{return Err(());}bytes.extend_from_slice(&part);}
        Ok(Response{status,bytes})
        }.await.map_err(|()|TransportError)
        })
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}
struct Flight {
    result: watch::Receiver<Option<Result<Vec<Model>, Error>>>,
}
struct Catalog {
    expires: std::time::Instant,
    models: Option<Vec<Model>>,
    flight: Option<Arc<Flight>>,
}
struct Inner {
    http: Arc<dyn Transport>,
    secrets: Arc<dyn Secrets>,
    text: Arc<dyn Text>,
    catalog: Mutex<Catalog>,
    stop: watch::Sender<bool>,
    leases: watch::Sender<usize>,
    unknown: std::sync::atomic::AtomicBool,
}
#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}
impl Service {
    pub fn new(http: Arc<dyn Transport>, secrets: Arc<dyn Secrets>, text: Arc<dyn Text>) -> Self {
        let (stop, _) = watch::channel(false);
        let (leases, _) = watch::channel(0usize);
        Self {
            inner: Arc::new(Inner {
                http,
                secrets,
                text,
                catalog: Mutex::new(Catalog {
                    expires: std::time::Instant::now(),
                    models: None,
                    flight: None,
                }),
                stop,
                leases,
                unknown: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }
    fn enter(&self) -> Result<Lease, Error> {
        self.ensure_running()?;
        self.inner.leases.send_modify(|count| *count += 1);
        let lease = Lease(self.inner.leases.clone());
        self.ensure_running()?;
        Ok(lease)
    }
    fn ensure_running(&self) -> Result<(), Error> {
        if *self.inner.stop.borrow() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn bind(&self, settings: Arc<dyn Settings>) -> Call {
        Call {
            service: self.clone(),
            settings,
        }
    }
    pub fn begin_shutdown(&self) {
        self.inner.stop.send_replace(true);
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.begin_shutdown();
        let flight = self
            .inner
            .catalog
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .flight
            .clone();
        // Signal the text owner now, even when a different request has not retired.
        let text = self.inner.text.shutdown();
        let mut leases = self.inner.leases.subscribe();
        let cleanup = async {
            loop {
                let active = *leases.borrow_and_update();
                if active == 0 {
                    break;
                }
                if leases.changed().await.is_err() {
                    return Err(Error::CleanupUnknown);
                }
            }
            if let Some(flight) = flight {
                let mut done = flight.result.clone();
                loop {
                    if done.borrow_and_update().is_some() {
                        break;
                    }
                    if done.changed().await.is_err() {
                        return Err(Error::CleanupUnknown);
                    }
                }
            }
            text.await
        };
        let result = tokio::time::timeout(Duration::from_secs(5), cleanup)
            .await
            .unwrap_or(Err(Error::CleanupUnknown));
        if result.is_err() {
            self.inner
                .unknown
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if self
            .inner
            .unknown
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err(Error::CleanupUnknown)
        } else {
            Ok(())
        }
    }

    async fn catalog(&self) -> Result<Vec<Model>, Error> {
        self.ensure_running()?;
        let flight = {
            let mut state = self
                .inner
                .catalog
                .lock()
                .map_err(|_| Error::CleanupUnknown)?;
            if let Some(models) = state
                .models
                .as_ref()
                .filter(|_| std::time::Instant::now() < state.expires)
            {
                return Ok(models.clone());
            }
            if let Some(flight) = &state.flight {
                flight.clone()
            } else {
                let (done, result) = watch::channel(None);
                let flight = Arc::new(Flight { result });
                state.flight = Some(flight.clone());
                let inner = self.inner.clone();
                let mut stop = inner.stop.subscribe();
                tokio::spawn(async move {
                    let unavailable = || failed(Provider::OpenRouter, Reason::Unavailable);
                    let fetch = async {
                        let response = inner
                            .http
                            .execute(Request {
                                endpoint: Endpoint::Catalog,
                                bearer: None,
                                body: Body::Empty,
                            })
                            .await
                            .map_err(|_| unavailable())?;
                        if !(200..300).contains(&response.status) {
                            return Err(unavailable());
                        }
                        parse_catalog(&response.bytes).ok_or_else(unavailable)
                    };
                    let result = tokio::select! {biased;_=cancelled(&mut stop)=>Err(Error::Cancelled),result=tokio::time::timeout(Duration::from_secs(10),fetch)=>result.unwrap_or_else(|_|Err(unavailable()))};
                    if let Ok(mut state) = inner.catalog.lock() {
                        if let Ok(models) = &result {
                            state.models = Some(models.clone());
                            state.expires = std::time::Instant::now() + Duration::from_secs(600);
                        }
                        state.flight = None;
                    }
                    done.send_replace(Some(result));
                });
                flight
            }
        };
        let mut result = flight.result.clone();
        loop {
            if let Some(value) = result.borrow_and_update().clone() {
                return value;
            }
            if result.changed().await.is_err() {
                return Err(failed(Provider::OpenRouter, Reason::Unavailable));
            }
        }
    }
}
struct Lease(watch::Sender<usize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}
pub struct Call {
    service: Service,
    settings: Arc<dyn Settings>,
}
impl Call {
    fn check(&self) -> Result<(), Error> {
        self.service.ensure_running()?;
        self.settings.check()
    }
    pub async fn configured(&self) -> Result<Value, Error> {
        let _lease = self.service.enter()?;
        self.check()?;
        let snapshot = self.settings.read().await?;
        Ok(
            json!({"openai":present(&snapshot.credentials["openai"]),"xai":present(&snapshot.credentials["xai"]),"openrouter":present(&snapshot.credentials["openrouter"])}),
        )
    }
    pub async fn models(&self) -> Result<Vec<Model>, Error> {
        let _lease = self.service.enter()?;
        self.check()?;
        let result = self.service.catalog().await?;
        self.check()?;
        Ok(result)
    }
    /// Input was decoded by the actual HTTP/secret schema; null clears exactly one slot.
    pub async fn set_key(&self, provider: Provider, key: Option<Value>) -> Result<(), Error> {
        let _lease = self.service.enter()?;
        self.check()?;
        self.settings
            .patch(json!({"cloudSpeechCredentials":{provider.as_str():key.unwrap_or(json!(""))}}))
            .await
    }
    pub async fn transcribe(
        &self,
        provider: Provider,
        bytes: Vec<u8>,
        options: TranscriptionOptions,
        model: Option<String>,
    ) -> Result<String, Error> {
        let _lease = self.service.enter()?;
        let mut stop = self.service.inner.stop.subscribe();
        let operation = self.transcribe_inner(provider, bytes, options, model);
        tokio::select! {biased;_=cancelled(&mut stop)=>Err(Error::Cancelled),result=tokio::time::timeout(Duration::from_secs(120),operation)=>result.unwrap_or_else(|_|Err(failed(provider,Reason::Unavailable)))}
    }
    async fn transcribe_inner(
        &self,
        provider: Provider,
        bytes: Vec<u8>,
        options: TranscriptionOptions,
        model: Option<String>,
    ) -> Result<String, Error> {
        let snapshot = self.settings.read().await?;
        let source = snapshot.credentials[provider.as_str()].clone();
        if !present(&source) {
            return Err(failed(provider, Reason::MissingKey));
        }
        if provider == Provider::OpenRouter && !model.as_deref().is_some_and(valid_model_id) {
            return Err(failed(provider, Reason::ModelUnavailable));
        }
        let pcm = Pcm::decode(&bytes).map_err(|_| Error::InvalidAudio { bytes: bytes.len() })?;
        let language = effective_language(
            options.as_value()["speechLanguage"]
                .as_str()
                .unwrap_or("auto"),
        );
        if provider == Provider::OpenRouter
            && !self
                .service
                .catalog()
                .await?
                .iter()
                .any(|candidate| Some(&candidate.id) == model.as_ref())
        {
            return Err(failed(provider, Reason::ModelUnavailable));
        }
        self.check()?;
        let reference = crate::managed_settings_codec::is_secret_reference(&source)
            .then(|| SecretReference::from_decoded(&source))
            .flatten();
        let resolved = if let Some(reference) = &reference {
            self.service
                .inner
                .secrets
                .read(&snapshot.secret_providers, reference)
                .await
                .ok_or_else(|| failed(provider, Reason::UnreadableKey))?
        } else {
            ResolvedSecret {
                value: source.as_str().ok_or(Error::Settings)?.to_owned(),
                identity: None,
            }
        };
        if !valid_api_key(&resolved.value) {
            return Err(failed(provider, Reason::UnreadableKey));
        }
        let body = audio_body(
            provider,
            model.as_deref(),
            pcm.wave(),
            &language,
            options.as_value(),
        );
        self.check()?;
        self.service
            .inner
            .text
            .prepare()
            .await
            .map_err(|_| failed(provider, Reason::Unavailable))?;
        let current_snapshot = self.settings.read().await?;
        let current = current_snapshot.credentials[provider.as_str()].clone();
        if !present(&current) || !equivalent(&source, &current) {
            return Err(failed(
                provider,
                if present(&current) {
                    Reason::KeyChanged
                } else {
                    Reason::MissingKey
                },
            ));
        }
        // O3 intentionally strengthens native reference equivalence: a vault
        // account changing during the lookup cannot authorize an upload under its old key.
        if let Some(identity) = resolved.identity.as_ref() {
            if reference
                .as_ref()
                .and_then(|reference| {
                    self.service
                        .inner
                        .secrets
                        .identity(&current_snapshot.secret_providers, reference)
                })
                .as_ref()
                != Some(identity)
            {
                return Err(failed(provider, Reason::KeyChanged));
            }
        }
        self.check()?;
        let response = self
            .service
            .inner
            .http
            .execute(Request {
                endpoint: Endpoint::Transcribe(provider),
                bearer: Some(resolved.value),
                body,
            })
            .await
            .map_err(|_| failed(provider, Reason::Unavailable))?;
        if !(200..300).contains(&response.status) {
            return Err(failed(
                provider,
                match response.status {
                    429 => Reason::RateLimit,
                    401..=403 => Reason::RejectedKey,
                    404 if provider == Provider::OpenRouter => Reason::ModelUnavailable,
                    _ => Reason::Unavailable,
                },
            ));
        }
        let value =
            parse_json(&response.bytes).ok_or_else(|| failed(provider, Reason::Unavailable))?;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| failed(provider, Reason::Unavailable))?;
        self.check()?;
        let corrected = self
            .service
            .inner
            .text
            .cloud(text, &options, &language)
            .await
            .map_err(|_| failed(provider, Reason::Unavailable))?;
        self.check()?;
        Ok(corrected)
    }
}
async fn cancelled(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}
fn present(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.is_empty(),
        Value::Object(_) => true,
        _ => false,
    }
}
fn equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(a), Value::String(b)) => a == b,
        _ => SecretReference::from_decoded(left)
            .zip(SecretReference::from_decoded(right))
            .is_some_and(|(a, b)| a == b),
    }
}
fn parse_json(bytes: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
}
fn parse_catalog(bytes: &[u8]) -> Option<Vec<Model>> {
    let value = parse_json(bytes)?;
    let rows = value.get("data")?.as_array()?;
    let mut models = vec![];
    for row in rows {
        let id = row.get("id")?.as_str()?;
        let name = match row.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            _ => return None,
        };
        if valid_model_id(id) {
            let name = name
                .map(js_trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            models.push(Model {
                id: id.into(),
                name: name.into(),
            });
        }
    }
    Some(models)
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
pub fn valid_model_id(value: &str) -> bool {
    if value.len() < 3 || value.len() > 160 || !value.is_ascii() {
        return false;
    }
    let Some((author, model)) = value.split_once('/') else {
        return false;
    };
    if author.eq_ignore_ascii_case("openrouter") || model.contains('/') {
        return false;
    }
    let valid = |part: &str, model: bool| {
        let mut bytes = part.bytes();
        bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
            && bytes.all(|b| {
                b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || (model && b == b'_')
            })
    };
    valid(author, false) && valid(model, true)
}
pub fn valid_api_key(value: &str) -> bool {
    if value.is_empty() || value.len() > 4096 || !value.bytes().all(|b| matches!(b, b'!'..=b'~')) {
        return false;
    }
    if let Some((scheme, _)) = value.split_once("://") {
        let mut bytes = scheme.bytes();
        if bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
            && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
        {
            return false;
        }
    }
    true
}
fn effective_language(intent: &str) -> String {
    speech_kernel::effective_language(&speech_kernel::ModelLanguages{languages:"ar bg ca cs da de el en es fa fi fr hi hr hu id it ja ko mk ms nl pl pt ro ru sk sv th tr uk ur vi zh".split(' ').map(str::to_owned).collect(),supports_language_detection:true},intent)
}
fn audio_body(
    provider: Provider,
    model: Option<&str>,
    audio: Vec<u8>,
    language: &str,
    options: &Value,
) -> Body {
    if provider == Provider::OpenRouter {
        use base64::Engine;
        let mut body = json!({"model":model.unwrap_or(""),"input_audio":{"data":base64::engine::general_purpose::STANDARD.encode(audio),"format":"wav"}});
        if language != "auto" {
            body["language"] = json!(language);
        }
        body["provider"] = json!({"zdr":true});
        return Body::Json(body);
    }
    let mut parts = vec![];
    let mut field = |key: &str, value: &str| {
        parts.push(Part::Field {
            key: key.into(),
            value: value.into(),
        })
    };
    field(
        "model",
        if provider == Provider::OpenAi {
            "gpt-transcribe"
        } else {
            "grok-voice-transcribe-2.0"
        },
    );
    if language != "auto" {
        field(
            if provider == Provider::OpenAi {
                "languages[]"
            } else {
                "language"
            },
            language,
        );
    }
    for word in options["speechCustomWords"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let term = word["term"].as_str().unwrap_or("");
        if provider == Provider::OpenAi && term.contains(['<', '>', '\r', '\n']) {
            continue;
        }
        field(
            if provider == Provider::OpenAi {
                "keywords[]"
            } else {
                "keyterm"
            },
            term,
        );
    }
    if provider == Provider::Xai {
        if language != "auto" {
            field("format", "true");
        }
        field(
            "filler_words",
            if options["speechRemoveFillerWords"] != true
                || options["speechCorrectionWord"]
                    .as_str()
                    .is_some_and(|cue| !cue.is_empty())
            {
                "true"
            } else {
                "false"
            },
        );
        if options["speechSilenceFiltering"] == false {
            field("vad_threshold", "0");
        }
    }
    parts.push(Part::File {
        key: "file",
        name: "dictation.wav",
        mime: "audio/wav",
        bytes: audio,
    });
    Body::Multipart(parts)
}
