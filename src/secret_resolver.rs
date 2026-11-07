//! Native secret-provider resolution. Secrets remain in memory and never enter settings storage.
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::Semaphore};

type ReadFuture<'a> = Pin<Box<dyn Future<Output = Vec<Option<String>>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SecretReference {
    provider: String,
    reference: String,
}
impl SecretReference {
    /// Decode the native reference schema, retaining its distinction from a one-use secretRef.
    pub fn decode(value: &Value) -> Option<Self> {
        Self::from_fields(
            trim(value.get("provider")?.as_str()?),
            trim(value.get("reference")?.as_str()?),
        )
    }
    /// Read a schema-validated value without decoding it a second time.
    pub(crate) fn from_decoded(value: &Value) -> Option<Self> {
        Self::from_fields(
            value.get("provider")?.as_str()?,
            value.get("reference")?.as_str()?,
        )
    }
    fn from_fields(provider: &str, reference: &str) -> Option<Self> {
        if provider.is_empty()
            || provider.encode_utf16().count() > 64
            || !regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$")
                .unwrap()
                .is_match(provider)
            || trim(reference).is_empty()
            || reference.encode_utf16().count() > 1024
            || reference.contains(['\r', '\n'])
        {
            return None;
        }
        Some(Self {
            provider: provider.into(),
            reference: reference.into(),
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn to_value(&self) -> Value {
        json!({"provider":self.provider,"reference":self.reference})
    }
    fn key(&self) -> String {
        format!("{}\0{}", self.provider, self.reference)
    }
}

/// Drivers validate and decode their configuration before any result can enter the cache.
pub trait SecretDriver: Send + Sync {
    fn kind(&self) -> &str;
    fn decode_config(&self, value: &Value) -> Option<Value>;
    fn is_reference(&self, reference: &str) -> bool;
    fn read<'a>(&'a self, config: Value, references: Vec<String>) -> ReadFuture<'a>;
}
struct CachedSecret {
    config: String,
    value: Option<String>,
}
#[derive(Clone)]
struct Plan {
    driver: Arc<dyn SecretDriver>,
    config: Value,
    config_key: String,
}

pub(crate) struct ReadReceipt {
    pub value: Option<String>,
    pub identity: Option<String>,
}
fn identity(plan: &Plan) -> String {
    serde_json::to_string(&json!({"kind":plan.driver.kind(),"config":plan.config_key}))
        .expect("decoded secret provider identity")
}
pub struct SecretResolver {
    drivers: HashMap<String, Arc<dyn SecretDriver>>,
    cache: Mutex<HashMap<String, CachedSecret>>,
    semaphore: Semaphore,
}
impl SecretResolver {
    pub fn new(drivers: Vec<Arc<dyn SecretDriver>>) -> Self {
        Self {
            drivers: drivers
                .into_iter()
                .map(|d| (d.kind().to_owned(), d))
                .collect(),
            cache: Mutex::new(HashMap::new()),
            semaphore: Semaphore::new(1),
        }
    }
    pub(crate) fn validates_config(&self, kind: &str, value: &Value) -> Option<bool> {
        self.drivers
            .get(kind)
            .map(|driver| driver.decode_config(value).is_some())
    }
    pub fn system() -> Self {
        Self::new(vec![Arc::new(OnePassword::system())])
    }
    fn plan(&self, providers: &Value, reference: &SecretReference) -> Option<Plan> {
        let provider = providers.as_object()?.get(&reference.provider)?;
        let driver = self.drivers.get(provider.get("kind")?.as_str()?)?;
        if !driver.is_reference(&reference.reference) {
            return None;
        }
        let config = driver.decode_config(provider.get("config").unwrap_or(&Value::Null))?;
        let config_key = json_stringify(&config);
        Some(Plan {
            driver: driver.clone(),
            config,
            config_key,
        })
    }
    fn cached(&self, reference: &SecretReference, plan: &Plan) -> bool {
        self.cache
            .lock()
            .unwrap()
            .get(&reference.key())
            .is_some_and(|cached| cached.config == plan.config_key)
    }
    fn answer(
        &self,
        references: &[SecretReference],
        plans: &[Option<Plan>],
    ) -> Vec<Option<String>> {
        let cache = self.cache.lock().unwrap();
        references
            .iter()
            .zip(plans)
            .map(|(reference, plan)| {
                plan.as_ref().and_then(|plan| {
                    cache
                        .get(&reference.key())
                        .filter(|v| v.config == plan.config_key)
                        .and_then(|v| v.value.clone())
                })
            })
            .collect()
    }
    /// `providers` is one effective settings snapshot, obtained once before this read.
    pub async fn read(
        &self,
        providers: &Value,
        references: &[SecretReference],
    ) -> Vec<Option<String>> {
        let plans = references
            .iter()
            .map(|reference| self.plan(providers, reference))
            .collect::<Vec<_>>();
        self.read_planned(references, &plans).await
    }
    /// The identity comes from the exact decoded plan used for this lookup/cache answer.
    pub(crate) async fn read_with_identity(
        &self,
        providers: &Value,
        references: &[SecretReference],
    ) -> Vec<ReadReceipt> {
        let plans = references
            .iter()
            .map(|reference| self.plan(providers, reference))
            .collect::<Vec<_>>();
        self.read_planned(references, &plans)
            .await
            .into_iter()
            .zip(&plans)
            .map(|(value, plan)| ReadReceipt {
                value,
                identity: plan.as_ref().map(identity),
            })
            .collect()
    }
    pub(crate) fn identity(
        &self,
        providers: &Value,
        reference: &SecretReference,
    ) -> Option<String> {
        self.plan(providers, reference).as_ref().map(identity)
    }
    async fn read_planned(
        &self,
        references: &[SecretReference],
        plans: &[Option<Plan>],
    ) -> Vec<Option<String>> {
        if !references
            .iter()
            .zip(plans)
            .any(|(r, p)| p.as_ref().is_some_and(|p| !self.cached(r, p)))
        {
            return self.answer(references, plans);
        }
        let _permit = self
            .semaphore
            .acquire()
            .await
            .expect("resolver semaphore stays open");
        let mut wanted: IndexMap<String, (Plan, IndexMap<String, Vec<&SecretReference>>)> =
            IndexMap::new();
        for (reference, plan) in references.iter().zip(plans) {
            let Some(plan) = plan.as_ref().filter(|p| !self.cached(reference, p)) else {
                continue;
            };
            let group = wanted
                .entry(reference.provider.clone())
                .or_insert_with(|| (plan.clone(), IndexMap::new()));
            group
                .1
                .entry(reference.reference.clone())
                .or_default()
                .push(reference);
        }
        for (_, (plan, by_reference)) in wanted {
            let values = plan
                .driver
                .read(plan.config, by_reference.keys().cloned().collect())
                .await;
            let mut cache = self.cache.lock().unwrap();
            for (index, same) in by_reference.values().enumerate() {
                for reference in same {
                    cache.insert(
                        reference.key(),
                        CachedSecret {
                            config: plan.config_key.clone(),
                            value: values.get(index).cloned().flatten(),
                        },
                    );
                }
            }
        }
        self.answer(references, plans)
    }
    /// Serializes with pending unlocks; None invalidates every cached success and failure.
    pub async fn invalidate(&self, references: Option<&[SecretReference]>) {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .expect("resolver semaphore stays open");
        let mut cache = self.cache.lock().unwrap();
        if let Some(references) = references {
            for reference in references {
                cache.remove(&reference.key());
            }
        } else {
            cache.clear()
        }
    }
    pub fn label(providers: &Value, provider: &str) -> String {
        providers
            .as_object()
            .and_then(|v| v.get(provider))
            .and_then(|v| v.get("label"))
            .and_then(Value::as_str)
            .map(trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(provider)
            .to_owned()
    }
}

/// The native built-in driver invokes op with argv only, with no interactive daemon cache.
pub struct OnePassword {
    binary: Option<PathBuf>,
    timeout: Duration,
}
impl OnePassword {
    pub fn system() -> Self {
        Self {
            binary: None,
            timeout: Duration::from_secs(45),
        }
    }
    pub fn with_binary(binary: PathBuf) -> Self {
        Self {
            binary: Some(binary),
            timeout: Duration::from_secs(45),
        }
    }
    #[cfg(test)]
    fn with_timeout(binary: PathBuf, timeout: Duration) -> Self {
        Self {
            binary: Some(binary),
            timeout,
        }
    }
    fn valid_account(account: &str) -> bool {
        account.encode_utf16().count() <= 253
            && regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
                .unwrap()
                .is_match(account)
    }
    async fn command(&self, args: &[String]) -> Result<CommandResult, ()> {
        let mut command = tokio::process::Command::new(
            self.binary.as_deref().unwrap_or(std::path::Path::new("op")),
        );
        command
            .arg("--cache=false")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = command.output().await.map_err(|_| {
            eprintln!(
                "Could not run 1Password (stage={})",
                args.first().map_or("unknown", String::as_str)
            )
        })?;
        Ok(CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            code: output.status.code().unwrap_or(-1),
        })
    }
    async fn one(&self, account: &str, reference: &str) -> Option<String> {
        let result = self
            .command(&[
                "read".into(),
                format!("--account={account}"),
                "--no-newline".into(),
                reference.into(),
            ])
            .await
            .ok()?;
        if result.code != 0 {
            eprintln!(
                "Could not read secret from 1Password (stage=read, exitCode={})",
                result.code
            );
            return None;
        }
        non_empty(result.stdout)
    }
    async fn together(&self, account: &str, references: &[String]) -> Option<Vec<Option<String>>> {
        let separator = format!("__t3-secret-{}__", uuid().ok()?);
        let mut template = PrivateTemplate::create().ok()?;
        let contents = references
            .iter()
            .map(|r| format!("{{{{ {r} }}}}{separator}"))
            .collect::<String>();
        template.write(contents.as_bytes()).await.ok()?;
        let result = self
            .command(&[
                "inject".into(),
                format!("--account={account}"),
                "-i".into(),
                template.path.to_string_lossy().into_owned(),
            ])
            .await
            .ok()?;
        let mut values = result
            .stdout
            .split(&separator)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let trailing = values.pop()?;
        if result.code != 0 || values.len() != references.len() || !trim(&trailing).is_empty() {
            eprintln!("Could not batch-read secrets from 1Password (stage=inject, exitCode={}, references={})",result.code,references.len());
            return None;
        }
        Some(values.into_iter().map(non_empty).collect())
    }
    async fn read_values(&self, config: Value, references: Vec<String>) -> Vec<Option<String>> {
        let Some(config) = self.decode_config(&config) else {
            return references.iter().map(|_| None).collect();
        };
        let account = config["account"].as_str().unwrap();
        if references.len() > 1 {
            match tokio::time::timeout(self.timeout, self.together(account, &references)).await {
                Err(_) => {
                    eprintln!("1Password did not answer in time (stage=inject)");
                    return references.iter().map(|_| None).collect();
                }
                Ok(Some(values)) => return values,
                Ok(None) => {}
            }
        }
        let mut values = Vec::with_capacity(references.len());
        for reference in references {
            values.push(
                match tokio::time::timeout(self.timeout, self.one(account, &reference)).await {
                    Ok(value) => value,
                    Err(_) => {
                        eprintln!("1Password did not answer in time (stage=read)");
                        None
                    }
                },
            );
        }
        values
    }
}
impl SecretDriver for OnePassword {
    fn kind(&self) -> &str {
        "one_password"
    }
    fn decode_config(&self, value: &Value) -> Option<Value> {
        let account = value.get("account")?.as_str()?;
        Self::valid_account(account).then(|| json!({"account":account}))
    }
    fn is_reference(&self, reference: &str) -> bool {
        if reference.encode_utf16().count() > 1024 {
            return false;
        }
        let name = r"[A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?";
        regex::Regex::new(&format!(
            r"^op://{name}(?:/{name}){{2,3}}(?:\?[a-z-]+=[A-Za-z0-9_-]+)?$"
        ))
        .unwrap()
        .is_match(reference)
    }
    fn read<'a>(&'a self, config: Value, references: Vec<String>) -> ReadFuture<'a> {
        Box::pin(self.read_values(config, references))
    }
}
struct CommandResult {
    stdout: String,
    code: i32,
}
struct PrivateTemplate {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}
impl PrivateTemplate {
    fn create() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("t3code-op-inject-{}", uuid()?));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self {
            path,
            file: Some(tokio::fs::File::from_std(file)),
        })
    }
    async fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = self.file.take().unwrap();
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        Ok(())
    }
}
impl Drop for PrivateTemplate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
fn uuid() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let s = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    ))
}
fn non_empty(value: String) -> Option<String> {
    (!trim(&value).is_empty()).then_some(value)
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn json_stringify(value: &Value) -> String {
    match value {
        Value::Number(n) => ryu_js::Buffer::new().format(n.as_f64().unwrap()).to_owned(),
        Value::Array(v) => format!(
            "[{}]",
            v.iter().map(json_stringify).collect::<Vec<_>>().join(",")
        ),
        Value::Object(v) => {
            let mut entries: Vec<_> = v.iter().collect();
            entries.sort_by_key(|(k, _)| {
                k.parse::<u32>()
                    .ok()
                    .filter(|n| *n != u32::MAX && n.to_string() == **k)
                    .map_or((1, 0), |n| (0, n))
            });
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(k, v)| format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        json_stringify(v)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

#[cfg(test)]
#[path = "../tests/support/secret_resolver.rs"]
mod tests;
