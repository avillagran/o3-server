//! Pinned nightly managed policy schema and immutable machine policy intake.
//! Consumers must pass the enforcement gate before advertising an installed policy.
use indexmap::IndexMap;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    path::{Path, PathBuf},
};
#[path = "managed_providers.rs"]
mod provider_controls;

pub const EXIT_CODE: i32 = 77;
pub const MACOS_PROFILE: &str = "/Library/Managed Preferences/com.t3tools.t3code.plist";
pub const WINDOWS_POLICY_KEY: &str = "HKLM\\SOFTWARE\\Policies\\T3Tools\\T3Code";

pub fn machine_path(platform: &str) -> &'static str {
    match platform {
        "darwin" | "macos" => "/Library/Application Support/T3Code/policy.json",
        "win32" | "windows" => "C:\\Program Files\\T3Code\\policy.json",
        _ => "/etc/t3code/policy.json",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub path: PathBuf,
    pub reason: String,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "T3 Code cannot use the managed policy at {}: {}. It will not start until an administrator fixes this.",
            self.path.display(),
            self.reason
        )
    }
}
impl std::error::Error for Error {}
impl Error {
    fn new(path: &Path, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

fn present<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(de).map(Some)
}
fn js_space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct Text(String);
impl Text {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        let text = text.trim_matches(js_space);
        if text.is_empty() {
            return Err(D::Error::custom("expected a non-blank string"));
        }
        Ok(Self(text.into()))
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct Slug(Text);
impl Slug {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
impl<'de> Deserialize<'de> for Slug {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = Text::deserialize(de)?;
        let value = text.as_str();
        if value.len() > 64
            || !value.as_bytes()[0].is_ascii_alphabetic()
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err(D::Error::custom("expected a provider driver slug"));
        }
        Ok(Self(text))
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct EnvironmentName(Text);
impl EnvironmentName {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = Text::deserialize(de)?;
        let value = text.as_str();
        if value.len() > 128
            || !(value.as_bytes()[0].is_ascii_alphabetic() || value.starts_with('_'))
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(D::Error::custom("expected an environment variable name"));
        }
        Ok(Self(text))
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct HttpsUrl(Text);
impl<'de> Deserialize<'de> for HttpsUrl {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = Text::deserialize(de)?;
        if !text
            .as_str()
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
            || !text
                .as_str()
                .get(8..)
                .and_then(|v| v.chars().next())
                .is_some_and(|c| c != '/' && !js_space(c))
        {
            return Err(D::Error::custom("expected an https URL"));
        }
        Ok(Self(text))
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct NonEmpty<T>(pub Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for NonEmpty<T> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let values = Vec::deserialize(de)?;
        if values.is_empty() {
            return Err(D::Error::custom("expected a nonempty array"));
        }
        Ok(Self(values))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeMode {
    #[serde(rename = "approval-required")]
    Supervised,
    #[serde(rename = "auto-accept-edits")]
    Edit,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "full-access")]
    Full,
}
impl RuntimeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supervised => "Supervised",
            Self::Edit => "Auto-accept edits",
            Self::Auto => "Auto",
            Self::Full => "Full access",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Client {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "local-browser")]
    LocalBrowser,
    #[serde(rename = "remote")]
    Remote,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdatesMode {
    Auto,
    Off,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Updates {
    pub mode: UpdatesMode,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub feed_url: Option<HttpsUrl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub allow_nightly: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub name: EnvironmentName,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderInstance {
    pub locked: bool,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub environment: Option<Vec<Environment>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub binary_path: Option<Text>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub launch_args: Option<Text>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Providers {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub allowed_drivers: Option<NonEmpty<Slug>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub instances: Option<IndexMap<Slug, ProviderInstance>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_runtime_mode: Option<RuntimeMode>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub clients: Option<NonEmpty<Client>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub connect: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub webhook_tasks: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub telemetry: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub browser_cookie_import: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub providers: Option<Providers>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub updates: Option<Updates>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub value: Value,
    pub locked: bool,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(transparent)]
pub struct Version(u8);
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(de)?;
        if value.as_f64() != Some(1.0) {
            return Err(D::Error::custom("expected schemaVersion 1"));
        }
        Ok(Self(1))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub schema_version: Version,
    pub organization: Text,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub support_url: Option<Text>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub settings: Option<IndexMap<String, Setting>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub limits: Option<Limits>,
}

// JSON.parse replaces duplicate object keys before schema decoding. IndexMap
// retains first insertion order while replacing the previous value.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Json {
    Null(()),
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(IndexMap<String, Json>),
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub path: PathBuf,
    pub document: Option<Document>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Authentication,
    Webhooks,
    Settings,
    Providers,
    Runtime,
    Clients,
    Connect,
    Telemetry,
    BrowserCookies,
    Updates,
}
/// This list grows only with guarded consumers and their acceptance tests.
pub const ENFORCED_FAMILIES: &[Family] = &[Family::Authentication, Family::Webhooks];
impl Policy {
    pub fn load_server() -> Result<Self, Error> {
        let policy = Self::load_system()?;
        policy.require_enforced(ENFORCED_FAMILIES)?;
        Ok(policy)
    }
    pub fn unmanaged(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            document: None,
        }
    }
    pub fn parse(path: &Path, text: &str) -> Result<Self, Error> {
        let decode = || -> Result<Document, serde_json::Error> {
            let parsed: Json = serde_json::from_str(text)?;
            serde_json::from_str(&serde_json::to_string(&parsed)?)
        };
        let document = decode().map_err(|e| {
            Error::new(
                path,
                format!("it is not valid JSON or does not match the policy schema ({e})"),
            )
        })?;
        let policy = Self {
            path: path.into(),
            document: Some(document),
        };
        policy.validate_providers(std::env::consts::OS)?;
        Ok(policy)
    }
    /// Explicit paths are for draft checking and private tests. Production intake
    /// uses load_system and never a profile path or an environment override.
    pub fn load_file(path: &Path) -> Result<Self, Error> {
        let exists = path
            .try_exists()
            .map_err(|_| Error::new(path, "the file cannot be checked"))?;
        if !exists {
            return Ok(Self::unmanaged(path));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|_| Error::new(path, "the file cannot be read"))?;
        Self::parse(path, &text)
    }
    pub fn load_system() -> Result<Self, Error> {
        #[cfg(target_os = "windows")]
        return Err(Error::new(
            Path::new(WINDOWS_POLICY_KEY),
            "this build cannot load its registry reader; Windows managed policy support is not yet available",
        ));
        #[cfg(target_os = "macos")]
        {
            let path = Path::new(MACOS_PROFILE);
            let exists = path
                .try_exists()
                .map_err(|_| Error::new(path, "the profile cannot be checked"))?;
            let file = Self::load_file(Path::new(machine_path("darwin")))?;
            if !exists {
                return Ok(file);
            }
            let converted = std::process::Command::new("/usr/bin/plutil")
                .args(["-convert", "json", "-o", "-", MACOS_PROFILE])
                .output()
                .map_err(|_| Error::new(path, "the profile cannot be read"))?;
            if !converted.status.success() {
                return Err(Error::new(path, "the profile cannot be read"));
            }
            let converted = String::from_utf8(converted.stdout)
                .map_err(|_| Error::new(path, "the profile cannot be read"))?;
            let profile = Self::parse_macos_preferences(path, &converted)?;
            if file.document.is_some() {
                return Err(Error::new(
                    path,
                    format!(
                        "a policy file at {} also exists; deploy the policy one way",
                        file.path.display()
                    ),
                ));
            }
            return Ok(profile);
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Self::load_file(Path::new(machine_path(std::env::consts::OS)))
    }

    pub fn parse_macos_preferences(path: &Path, text: &str) -> Result<Self, Error> {
        fn clean(value: &mut Json) {
            if let Json::Object(values) = value {
                values.retain(|key, _| !key.starts_with("Payload") && !key.starts_with('_'));
            }
        }
        let mut value: Json = serde_json::from_str(text).map_err(|_| {
            Error::new(
                path,
                "it is not valid JSON or does not match the policy schema",
            )
        })?;
        clean(&mut value);
        let inner = match &value {
            Json::Object(values) if values.len() == 1 => match values.get("Forced") {
                Some(Json::Array(entries)) if entries.len() == 1 => match &entries[0] {
                    Json::Object(fields) if fields.len() == 1 => {
                        fields.get("mcx_preference_settings").cloned()
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        if let Some(inner) = inner {
            value = inner;
            clean(&mut value);
        }
        Self::parse(
            path,
            &serde_json::to_string(&value).map_err(|e| Error::new(path, e.to_string()))?,
        )
    }
    /// Do not start with a recognized restriction whose consumers are unfinished.
    pub fn require_enforced(&self, ready: &[Family]) -> Result<(), Error> {
        let Some(document) = &self.document else {
            return Ok(());
        };
        let mut needed = vec![Family::Authentication];
        if document.settings.as_ref().is_some_and(|v| !v.is_empty()) {
            needed.push(Family::Settings);
        }
        if let Some(limits) = &document.limits {
            if limits.webhook_tasks == Some(false) {
                needed.push(Family::Webhooks);
            }
            if limits.max_runtime_mode.is_some() {
                needed.extend([Family::Runtime, Family::Providers]);
            }
            if limits.clients.is_some() {
                needed.extend([Family::Clients, Family::Connect]);
            }
            if limits.connect == Some(false) {
                needed.push(Family::Connect);
            }
            if limits.telemetry == Some(false) {
                needed.push(Family::Telemetry);
            }
            if limits.browser_cookie_import == Some(false) {
                needed.push(Family::BrowserCookies);
            }
            if limits.providers.is_some() {
                needed.push(Family::Providers);
            }
            if limits.updates.is_some() {
                needed.push(Family::Updates);
            }
        }
        if let Some(family) = needed.into_iter().find(|family| !ready.contains(family)) {
            return Err(Error::new(
                &self.path,
                format!("this build cannot yet enforce the {family:?} managed policy controls"),
            ));
        }
        self.settings_controls()
            .map_err(|reason| Error::new(&self.path, reason))?;
        Ok(())
    }
    pub fn denial(&self, capability: &str) -> Option<String> {
        let document = self.document.as_ref()?;
        let limits = document.limits.as_ref()?;
        let denied = match capability {
            "webhookTasks" => limits.webhook_tasks == Some(false),
            "telemetry" => limits.telemetry == Some(false),
            "browserCookieImport" => limits.browser_cookie_import == Some(false),
            "remoteClients" => !self.allows_remote(),
            "connect" => !self.allows_connect(),
            _ => false,
        };
        let label = match capability {
            "webhookTasks" => "webhook tasks",
            "browserCookieImport" => "importing browser cookies",
            "remoteClients" => "connecting from other devices",
            "connect" => "T3 Connect",
            _ => capability,
        };
        denied.then(|| {
            format!(
                "{}'s policy does not allow {label}.",
                document.organization.as_str()
            )
        })
    }
    pub fn task_view(&self, mut task: Value) -> Value {
        if let Some(fields) = task.as_object_mut() {
            fields.remove("blockedByPolicy");
        }
        if task["schedule"]["type"] == "webhook" {
            if let Some(reason) = self.denial("webhookTasks") {
                task["blockedByPolicy"] = json!(reason);
            }
        }
        task
    }
    pub fn allows_auth(&self, policy: &str) -> bool {
        let Some(document) = &self.document else {
            return true;
        };
        if policy == "unsafe-no-auth" {
            return false;
        }
        let clients = document.limits.as_ref().and_then(|v| v.clients.as_ref());
        clients.is_none_or(|clients| {
            clients.0.iter().any(|client| {
                matches!(
                    (client, policy),
                    (Client::Desktop, "desktop-managed-local")
                        | (Client::LocalBrowser, "loopback-browser")
                        | (Client::Remote, "remote-reachable")
                )
            })
        })
    }
    pub fn allows_remote(&self) -> bool {
        self.allows_auth("remote-reachable")
    }
    pub fn allows_connect(&self) -> bool {
        self.document
            .as_ref()
            .and_then(|v| v.limits.as_ref())
            .is_none_or(|v| v.connect != Some(false))
            && self.allows_remote()
    }
    pub fn runtime_denial(&self, mode: RuntimeMode) -> Option<String> {
        let document = self.document.as_ref()?;
        let ceiling = document.limits.as_ref()?.max_runtime_mode?;
        (mode > ceiling).then(|| {
            format!(
                "{} limits threads to {}.",
                document.organization.as_str(),
                ceiling.label()
            )
        })
    }
    pub fn driver_denial(&self, driver: &str) -> Option<String> {
        let document = self.document.as_ref()?;
        let allowed = document
            .limits
            .as_ref()?
            .providers
            .as_ref()?
            .allowed_drivers
            .as_ref()?;
        (!allowed.0.iter().any(|value| value.as_str() == driver)).then(|| {
            format!(
                "{}'s policy does not allow the {} provider.",
                document.organization.as_str(),
                driver_label(driver)
            )
        })
    }
    pub(crate) fn settings_controls(
        &self,
    ) -> Result<Option<crate::managed_settings::ManagedSettings>, String> {
        let Some(document) = &self.document else {
            return Ok(None);
        };
        let ceiling = document
            .limits
            .as_ref()
            .and_then(|limits| limits.max_runtime_mode);
        if document
            .settings
            .as_ref()
            .is_none_or(|settings| settings.is_empty())
            && ceiling.is_none()
        {
            return Ok(None);
        }
        crate::managed_settings::ManagedSettings::make(
            document.settings.as_ref().unwrap_or(&IndexMap::new()),
            ceiling,
        )
        .map(Some)
    }
    pub(crate) fn effective_settings(&self, settings: &Value) -> Result<Value, String> {
        let mut effective = settings.clone();
        if let Some(controls) = self.settings_controls()? {
            let decoded = crate::managed_settings_codec::decode(controls.with_defaults(settings))
                .map_err(|_| "Invalid managed settings input")?;
            let locked = controls
                .apply_locks(&decoded)
                .map_err(|_| "Invalid managed settings overlay")?;
            let encoded = crate::managed_settings_codec::encode(locked)
                .map_err(|_| "Invalid managed settings encoding")?;
            effective = crate::settings::with_native_fields(settings, encoded);
        }
        let effective = self.provider_overlay(&effective);
        Ok(if self.document.is_some() {
            provider_controls::resolve_text_generation(self, &effective)
        } else {
            effective
        })
    }
    pub fn provider_overlay(&self, settings: &Value) -> Value {
        provider_controls::apply(self, settings, cfg!(target_os = "windows"))
    }
    pub fn vet_provider_write(&self, current: &Value, next: &Value) -> Result<Value, String> {
        self.vet_provider_write_rpc(current, next)
            .map_err(|error| error["message"].as_str().unwrap().to_owned())
    }
    pub(crate) fn vet_provider_write_rpc(
        &self,
        current: &Value,
        next: &Value,
    ) -> Result<Value, Value> {
        provider_controls::vet(self, current, next, cfg!(target_os = "windows")).map_err(|error| {
            let organization = self.document.as_ref().map(|v| v.organization.as_str()).unwrap_or("your organization");
            match error {
                provider_controls::WriteError::Locked(path) => json!({"operation":"write-managed-setting","managedSetting":path,"managedBy":organization,"message":format!("{path} is managed by {organization} and cannot be changed.")}),
                provider_controls::WriteError::Disallowed { instance_id, driver } => json!({"operation":"use-disallowed-provider","providerInstanceId":instance_id,"providerLabel":driver_label(&driver),"managedBy":organization,"message":self.driver_denial(&driver).unwrap()}),
            }
        })
    }
    pub fn provider_locks(&self) -> Value {
        provider_controls::locked_fields(self, cfg!(target_os = "windows"))
    }
    pub fn validate_providers(&self, platform: &str) -> Result<(), Error> {
        let Some(providers) = self
            .document
            .as_ref()
            .and_then(|v| v.limits.as_ref())
            .and_then(|v| v.providers.as_ref())
        else {
            return Ok(());
        };
        let known = [
            "codex",
            "claudeAgent",
            "antigravity",
            "cursor",
            "grok",
            "fx",
            "kiro",
            "muse",
            "opencode",
            "pi",
            "acpRegistry",
        ];
        for driver in providers.allowed_drivers.iter().flat_map(|v| &v.0) {
            if !known.contains(&driver.as_str()) {
                return Err(Error::new(
                    &self.path,
                    format!(
                        "\"{}\" in allowedDrivers is not a known provider",
                        driver.as_str()
                    ),
                ));
            }
        }
        for (driver, pin) in providers.instances.iter().flat_map(|v| v.iter()) {
            let driver = driver.as_str();
            if !known.contains(&driver) {
                return Err(Error::new(
                    &self.path,
                    format!("\"{driver}\" in instances is not a known provider"),
                ));
            }
            if pin.binary_path.is_some() && matches!(driver, "antigravity" | "acpRegistry") {
                return Err(Error::new(
                    &self.path,
                    format!(
                        "the {} provider has no binaryPath setting",
                        driver_label(driver)
                    ),
                ));
            }
            if pin.launch_args.is_some() && !matches!(driver, "codex" | "claudeAgent" | "pi") {
                return Err(Error::new(
                    &self.path,
                    format!(
                        "the {} provider has no launchArgs setting",
                        driver_label(driver)
                    ),
                ));
            }
            let mut names = std::collections::HashSet::new();
            for variable in pin.environment.iter().flatten() {
                let name = if matches!(platform, "windows" | "win32") {
                    variable.name.as_str().to_ascii_uppercase()
                } else {
                    variable.name.as_str().into()
                };
                if !names.insert(name) {
                    return Err(Error::new(
                        &self.path,
                        format!("instances.{driver}.environment repeats a variable"),
                    ));
                }
            }
        }
        Ok(())
    }
    /// Policy launch values and updater feed URLs are never included in summaries.
    /// Provider lock summaries are added only when provider enforcement is ready.
    pub fn basic_summary(&self) -> Option<Value> {
        let document = self.document.as_ref()?;
        let locked_settings = self
            .settings_controls()
            .ok()
            .flatten()
            .map(|settings| settings.locked_paths())
            .unwrap_or_default();
        let mut summary =
            json!({"organization":document.organization.as_str(),"lockedSettings":locked_settings});
        if let Some(url) = &document.support_url {
            summary["supportUrl"] = json!(url.as_str());
        }
        if let Some(limits) = &document.limits {
            let mut value = serde_json::to_value(limits).unwrap();
            value.as_object_mut().unwrap().remove("providers");
            let locks = self.provider_locks();
            let allowed = limits
                .providers
                .as_ref()
                .and_then(|p| p.allowed_drivers.as_ref());
            if allowed.is_some() || locks.as_object().is_some_and(|m| !m.is_empty()) {
                let mut providers = json!({});
                if let Some(allowed) = allowed {
                    providers["allowedDrivers"] = serde_json::to_value(allowed).unwrap();
                }
                if locks.as_object().is_some_and(|m| !m.is_empty()) {
                    providers["lockedInstanceFields"] = locks;
                }
                value["providers"] = providers;
            }
            if !self.allows_connect() {
                value["connect"] = json!(false);
            }
            if let Some(updates) = value.get_mut("updates") {
                updates.as_object_mut().unwrap().remove("feedUrl");
            }
            summary["limits"] = value;
        }
        Some(summary)
    }
}

/// The caller supplies its transaction so policy checks cannot re-enter the
/// profile's database mutex while admission is in progress.
pub fn check_command(
    state: &crate::store::State,
    db: &rusqlite::Connection,
    command: &Value,
) -> Result<(), String> {
    if state.managed_policy.document.is_none() {
        return Ok(());
    }
    let kind = command["type"].as_str().unwrap_or("");
    let thread = command["threadId"]
        .as_str()
        .and_then(|id| crate::events::get(db, "thread", id).ok());
    let mode = command
        .get("runtimeMode")
        .or_else(|| thread.as_ref().and_then(|v| v.get("runtimeMode")));
    if command.get("runtimeMode").is_some()
        || matches!(
            kind,
            "message.dispatch" | "prepared-run.retry" | "run.retry"
        )
    {
        if let Some(mode) = mode.and_then(|v| serde_json::from_value::<RuntimeMode>(v.clone()).ok())
        {
            if let Some(reason) = state.managed_policy.runtime_denial(mode) {
                return Err(reason);
            }
        }
    }
    if matches!(
        kind,
        "thread.create"
            | "message.dispatch"
            | "thread.model-selection.set"
            | "provider.switch"
            | "prepared-run.retry"
            | "run.retry"
    ) {
        if let Some(instance) = command["modelSelection"]["instanceId"]
            .as_str()
            .or_else(|| {
                thread
                    .as_ref()
                    .and_then(|v| v["modelSelection"]["instanceId"].as_str())
            })
        {
            let settings = crate::settings::read(state)?;
            let id = crate::provider_policy::configured_id(&settings, instance);
            let driver = settings["providerInstances"][id]["driver"]
                .as_str()
                .unwrap_or(if id == "codex" {
                    "codex"
                } else if matches!(id, "claudeAgent" | "claude" | "claude-code") {
                    "claudeAgent"
                } else {
                    id
                });
            if let Some(reason) = state.managed_policy.driver_denial(driver) {
                return Err(reason);
            }
        }
    }
    Ok(())
}
fn driver_label(driver: &str) -> &str {
    match driver {
        "codex" => "Codex",
        "claudeAgent" => "Claude",
        "antigravity" => "Antigravity",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "muse" => "Muse Code",
        "opencode" => "OpenCode",
        "pi" => "Pi",
        "kiro" => "Kiro",
        "acpRegistry" => "ACP Registry",
        other => other,
    }
}

/// Inherited launch overrides must not outrank any administrator policy. Explicit
/// instance variables are applied afterwards and are separately pinned by provider rules.
pub(crate) fn provider_host_environment(command: &mut tokio::process::Command, managed: bool) {
    if managed {
        let names = std::env::vars_os()
            .map(|(name, _)| name)
            .chain(command.as_std().get_envs().map(|(name, _)| name.to_owned()))
            .filter(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("T3CODE_CODEX_LAUNCH_ARGS")
            })
            .collect::<Vec<_>>();
        command.env_remove("T3CODE_CODEX_LAUNCH_ARGS");
        for name in names {
            command.env_remove(name);
        }
    }
}

#[cfg(test)]
mod host_environment_tests {
    use super::*;

    #[tokio::test]
    async fn managed_host_launch_overrides_are_removed_before_explicit_instance_environment() {
        for managed in [false, true] {
            let mut command = tokio::process::Command::new("/usr/bin/env");
            command.env_clear();
            command.env("T3CODE_CODEX_LAUNCH_ARGS", "host");
            command.env("t3code_codex_launch_args", "lower");
            command.env("OTHER", "retained");
            provider_host_environment(&mut command, managed);
            let output = command.output().await.unwrap();
            assert!(output.status.success());
            let environment = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                environment.contains("T3CODE_CODEX_LAUNCH_ARGS=host"),
                !managed
            );
            assert_eq!(
                environment.contains("t3code_codex_launch_args=lower"),
                !managed
            );
            assert!(environment.contains("OTHER=retained"));
            command.env("T3CODE_CODEX_LAUNCH_ARGS", "explicit-instance");
            let output = command.output().await.unwrap();
            assert!(String::from_utf8(output.stdout)
                .unwrap()
                .contains("T3CODE_CODEX_LAUNCH_ARGS=explicit-instance"));
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn managed_settings_read_write_preserves_hidden_values_and_sparse_defaults() {
        crate::test_sandbox::assert_isolated();
        let directory = std::env::temp_dir().join(format!(
            "managed-settings-physical-{}",
            crate::store::random_id().unwrap()
        ));
        let mut state = crate::store::State::open(&directory).unwrap();
        state.managed_policy = Policy::parse(Path::new("/fixture/policy.json"), r#"{"schemaVersion":1,"organization":"Fixture Corp","settings":{"enableAgentDeviceAccess":{"locked":true,"value":false},"worktreesDirectory":{"locked":false,"value":"/managed/default-worktrees"}},"limits":{"maxRuntimeMode":"approval-required"}}"#).unwrap();
        let path = directory.join("settings.json");
        std::fs::write(&path, json!({"enableAgentDeviceAccess":true,"responseStreamingMode":"token","defaultRuntimeMode":"full-access","o3Extension":{"retain":true},"projectSettingsOverrides":{"p":{"defaultRuntimeMode":"auto"}}}).to_string()).unwrap();
        let effective = crate::settings::read(&state).unwrap();
        assert_eq!(effective["enableAgentDeviceAccess"], false);
        assert_eq!(
            effective["worktreesDirectory"],
            "/managed/default-worktrees"
        );
        assert_eq!(effective["responseStreamingMode"], "paragraph");
        assert_eq!(effective["defaultRuntimeMode"], "approval-required");
        assert_eq!(
            effective["projectSettingsOverrides"]["p"]["defaultRuntimeMode"],
            "approval-required"
        );
        assert_eq!(effective["o3Extension"]["retain"], true);
        let updated = crate::settings::update_rpc(
            &state,
            &json!({"patch":{"defaultAutoPull":true,"enableAgentDeviceAccess":false}}),
        )
        .unwrap();
        assert_eq!(updated["enableAgentDeviceAccess"], false);
        let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["enableAgentDeviceAccess"], true);
        assert_eq!(stored["defaultAutoPull"], true);
        assert!(stored.get("worktreesDirectory").is_none());
        assert_eq!(stored["o3Extension"]["retain"], true);
        let before = std::fs::read(&path).unwrap();
        let error =
            crate::settings::update_rpc(&state, &json!({"patch":{"enableAgentDeviceAccess":true}}))
                .unwrap_err();
        assert_eq!(error["managedSetting"], "enableAgentDeviceAccess");
        assert_eq!(error["operation"], "write-managed-setting");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn recovered_runs_are_refused_before_provider_fence_when_frozen_policy_exceeds_limits() {
        crate::test_sandbox::assert_isolated();
        for (driver, mode) in [
            ("codex", "full-access"),
            ("claudeAgent", "approval-required"),
        ] {
            let directory = std::env::temp_dir().join(format!(
                "managed-recovered-{}",
                crate::store::random_id().unwrap()
            ));
            let mut state = crate::store::State::open(&directory).unwrap();
            state.managed_policy = Policy::parse(Path::new("/fixture/policy.json"), r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"maxRuntimeMode":"approval-required","providers":{"allowedDrivers":["codex"]}}}"#).unwrap();
            let state = Arc::new(state);
            *state.provider.lock().unwrap() = Some(Arc::new(crate::provider::ScriptedProvider));
            {
                let db = state.db.lock().unwrap();
                crate::events::put(&db, "thread", "t", "t", &json!({"id":"t","runtimeMode":"approval-required","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"}})).unwrap();
                crate::events::put(&db, "runs", "r", "t", &json!({"id":"r","status":"preparing","providerInstanceId":driver,"executionPolicy":{"runtimeMode":mode},"ordinal":1})).unwrap();
            }
            crate::provider::start(
                state.clone(),
                "t".into(),
                "r".into(),
                "Synthetic recovered work".into(),
            );
            let run = crate::events::get(&state.db.lock().unwrap(), "runs", "r").unwrap();
            assert_eq!(run["status"], "failed");
            assert!(run["error"].as_str().unwrap().contains("Fixture Corp"));
            assert!(state.active.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn provider_write_errors_preserve_native_rpc_metadata_and_do_not_persist() {
        crate::test_sandbox::assert_isolated();
        let directory = std::env::temp_dir().join(format!(
            "managed-write-{}",
            crate::store::random_id().unwrap()
        ));
        let mut state = crate::store::State::open(&directory).unwrap();
        state.managed_policy = Policy::parse(Path::new("/fixture/policy.json"), r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"providers":{"allowedDrivers":["codex"],"instances":{"codex":{"locked":true,"binaryPath":"/managed/codex"}}}}}"#).unwrap();
        let stored = json!({"providerInstances":{"codex":{"driver":"codex","config":{"binaryPath":"/stored/codex"}}}});
        let path = directory.join("settings.json");
        std::fs::write(&path, stored.to_string()).unwrap();
        let error = crate::settings::update_rpc(&state, &json!({"patch":{"providerInstances":{"codex":{"driver":"codex","config":{"binaryPath":"/changed/codex"}}}}})).unwrap_err();
        assert_eq!(error["operation"], "write-managed-setting");
        assert_eq!(
            error["managedSetting"],
            "providerInstances.codex.config.binaryPath"
        );
        assert_eq!(error["managedBy"], "Fixture Corp");
        assert_eq!(error["_tag"], "ServerSettingsError");
        let error = crate::settings::update_rpc(&state, &json!({"patch":{"providerInstances":{"blocked":{"driver":"claudeAgent","enabled":true}}}})).unwrap_err();
        assert_eq!(error["operation"], "use-disallowed-provider");
        assert_eq!(error["providerInstanceId"], "blocked");
        assert_eq!(error["providerLabel"], "Claude");
        assert_eq!(error["managedBy"], "Fixture Corp");
        assert_eq!(std::fs::read_to_string(path).unwrap(), stored.to_string());
    }

    #[test]
    fn policy_rejects_transactional_runtime_and_driver_changes_without_receipts_or_events() {
        crate::test_sandbox::assert_isolated();
        let directory = std::env::temp_dir().join(format!(
            "managed-admission-{}",
            crate::store::random_id().unwrap()
        ));
        let mut state = crate::store::State::open(&directory).unwrap();
        // Keep the production readiness gate closed while testing unfinished consumers.
        state.managed_policy = Policy::parse(Path::new("/fixture/policy.json"), r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"maxRuntimeMode":"approval-required","providers":{"allowedDrivers":["codex"]}}}"#).unwrap();
        let state = Arc::new(state);
        {
            let db = state.db.lock().unwrap();
            crate::events::put(&db, "thread", "t", "t", &json!({"id":"t","runtimeMode":"approval-required","modelSelection":{"instanceId":"codex","model":"test"},"providerInstanceId":"codex"})).unwrap();
        }
        let commands = [
            json!({"type":"thread.runtime-mode.set","runtimeMode":"full-access"}),
            json!({"type":"thread.model-selection.set","modelSelection":{"instanceId":"claudeAgent","model":"test"}}),
            json!({"type":"provider.switch","modelSelection":{"instanceId":"claudeAgent","model":"test"}}),
            json!({"type":"message.dispatch","runtimeMode":"full-access","modelSelection":{"instanceId":"codex","model":"test"}}),
            json!({"type":"message.dispatch","modelSelection":{"instanceId":"claudeAgent","model":"test"}}),
        ];
        for (index, mut command) in commands.into_iter().enumerate() {
            command["threadId"] = json!("t");
            command["commandId"] = json!(format!("denied-{index}"));
            let error = crate::events::dispatch(&state, &command).unwrap_err();
            assert!(error.contains("Fixture Corp"), "{command}: {error}");
        }
        let db = state.db.lock().unwrap();
        for table in ["events", "receipts"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert_eq!(
            crate::events::get(&db, "thread", "t").unwrap()["runtimeMode"],
            "approval-required"
        );
    }
}
