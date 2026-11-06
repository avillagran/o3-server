//! Schema-declared provider secret mapping, separate from stored provider identities.
use crate::{
    managed_settings_codec as codec,
    secret_resolver::{SecretReference, SecretResolver},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Field {
    field: String,
    keyed: bool,
}
static FIELDS: LazyLock<std::collections::BTreeMap<String, Vec<Field>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("provider-secret-fields-4275.json"))
        .expect("native reference field metadata")
});
#[derive(Clone, Debug)]
struct ConfigReference {
    field: String,
    key: Option<String>,
    reference: SecretReference,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct ConfigValue {
    field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Secrets {
    pub environment: Vec<Value>,
    pub config: Vec<ConfigValue>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolution {
    pub secrets: Secrets,
    pub unreadable: Vec<String>,
}
impl Resolution {
    pub(crate) fn reason(&self) -> Option<String> {
        (!self.unreadable.is_empty()).then(|| {
            format!(
                "Could not read {} from its secret provider. Unlock it, then refresh.",
                self.unreadable.join(", ")
            )
        })
    }
    fn config(&self, typed: &Value) -> Value {
        let mut next = typed.clone();
        for slot in &self.secrets.config {
            if let Some(key) = &slot.key {
                if !next[&slot.field].is_object() {
                    next[&slot.field] = json!({})
                }
                next[&slot.field][key] = json!(slot.value);
            } else {
                next[&slot.field] = json!(slot.value)
            }
        }
        next
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub raw: Value,
    pub decoded: Value,
    pub typed_config: Value,
    pub enabled: bool,
    fields: Vec<Field>,
}
#[derive(Debug)]
pub(crate) enum PreparationError {
    InvalidEntry,
    Unsupported(String),
    InvalidConfig(String),
    Blocked(String),
}
impl PreparationError {
    fn message(self, id: &str) -> String {
        match self {
            Self::InvalidEntry => format!("Invalid provider instance '{id}'"),
            Self::Unsupported(driver) => {
                format!("Driver '{driver}' is not registered in this build.")
            }
            Self::InvalidConfig(detail) => format!("Invalid config for instance '{id}': {detail}"),
            Self::Blocked(reason) => reason,
        }
    }
}
impl Prepared {
    pub(crate) fn new(raw: &Value) -> Result<Self, PreparationError> {
        let decoded = codec::decode_provider_instance(raw.clone())
            .map_err(|_| PreparationError::InvalidEntry)?;
        let driver = decoded["driver"].as_str().unwrap();
        if !matches!(driver, "codex" | "claudeAgent") {
            return Err(PreparationError::Unsupported(driver.to_owned()));
        }
        let config = decoded
            .get("config")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let typed_config = codec::decode_provider_config(driver, config)
            .map_err(PreparationError::InvalidConfig)?;
        let enabled = resolve_enabled(&decoded, &typed_config);
        let fields = FIELDS.get(driver).cloned().unwrap_or_default();
        Ok(Self {
            raw: raw.clone(),
            decoded,
            typed_config,
            enabled,
            fields,
        })
    }
    fn config_references(&self) -> Vec<ConfigReference> {
        config_references(&self.decoded["config"], &self.fields)
    }
    pub(crate) fn references(&self) -> Vec<SecretReference> {
        self.decoded["environment"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                (!v["value"].is_string())
                    .then(|| SecretReference::from_decoded(&v["value"]))
                    .flatten()
            })
            .chain(
                self.config_references()
                    .into_iter()
                    .map(|slot| slot.reference),
            )
            .collect()
    }
    pub(crate) async fn resolve(&self, resolver: &SecretResolver, providers: &Value) -> Resolution {
        let references = self.references();
        let values = if !self.enabled || references.is_empty() {
            Vec::new()
        } else {
            resolver.read(providers, &references).await
        };
        self.with_values(&values)
    }
    fn with_values(&self, values: &[Option<String>]) -> Resolution {
        let variables = self.decoded["environment"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !self.enabled {
            return Resolution {
                secrets: Secrets {
                    environment: variables
                        .into_iter()
                        .filter(|v| v["value"].is_string())
                        .collect(),
                    config: Vec::new(),
                },
                unreadable: Vec::new(),
            };
        }
        let mut index = 0;
        let mut unreadable = Vec::new();
        let mut next = || {
            let v = values
                .get(index)
                .cloned()
                .flatten()
                .filter(|v| !js_trim(v).is_empty());
            index += 1;
            v
        };
        let environment = variables
            .into_iter()
            .filter_map(|mut v| {
                if v["value"].is_string() {
                    return Some(v);
                }
                if let Some(secret) = next() {
                    v["value"] = json!(secret);
                    v["sensitive"] = json!(true);
                    Some(v)
                } else {
                    unreadable.push(v["name"].as_str().unwrap_or("").to_owned());
                    None
                }
            })
            .collect();
        let config = self
            .config_references()
            .into_iter()
            .filter_map(|slot| {
                if let Some(secret) = next() {
                    Some(ConfigValue {
                        field: slot.field,
                        key: slot.key,
                        value: secret,
                    })
                } else {
                    unreadable.push(slot.field);
                    None
                }
            })
            .collect();
        Resolution {
            secrets: Secrets {
                environment,
                config,
            },
            unreadable,
        }
    }
    pub(crate) fn resolved_entry(&self, resolution: &Resolution) -> Value {
        let mut entry = self.decoded.clone();
        entry["config"] = resolution.config(&self.typed_config);
        entry["environment"] = json!(resolution.secrets.environment);
        entry
    }
}
fn resolve_enabled(entry: &Value, typed: &Value) -> bool {
    if entry["enabled"] == false || entry["config"]["enabled"] == false {
        return false;
    }
    entry["enabled"]
        .as_bool()
        .or_else(|| typed["enabled"].as_bool())
        .unwrap_or(true)
}
fn config_references(config: &Value, fields: &[Field]) -> Vec<ConfigReference> {
    let mut out = Vec::new();
    for field in fields {
        let value = &config[&field.field];
        if !field.keyed {
            if codec::is_secret_reference(value) {
                if let Some(reference) = SecretReference::from_decoded(value) {
                    out.push(ConfigReference {
                        field: field.field.clone(),
                        key: None,
                        reference,
                    })
                }
            }
        } else if let Some(values) = value.as_object() {
            let mut values: Vec<_> = values.iter().collect();
            values.sort_by_key(|(k, _)| codec::array_index(k).map_or((1, 0), |v| (0, v)));
            for (key, value) in values {
                if codec::is_secret_reference(value) {
                    if let Some(reference) = SecretReference::from_decoded(value) {
                        out.push(ConfigReference {
                            field: field.field.clone(),
                            key: Some(key.clone()),
                            reference,
                        })
                    }
                }
            }
        }
    }
    out
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    Ready { entry: Value, secrets: Secrets },
    Reading,
    Unavailable(String),
}
struct Slot {
    raw: Value,
    prepared: Option<Prepared>,
    status: Status,
    ticket: u64,
    started: bool,
}
/// A registry's raw identities and resolved values have separate lifetimes.
pub(crate) struct Registry {
    resolver: std::sync::Arc<SecretResolver>,
    slots: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Slot>>>,
    changed: tokio::sync::watch::Sender<u64>,
    notify: Option<tokio::sync::watch::Sender<u64>>,
    ticket: std::sync::atomic::AtomicU64,
    jobs: std::sync::Mutex<Vec<tokio::task::AbortHandle>>,
}
impl Registry {
    pub(crate) fn new(resolver: std::sync::Arc<SecretResolver>) -> Self {
        Self {
            resolver,
            slots: Default::default(),
            changed: tokio::sync::watch::channel(0).0,
            notify: None,
            ticket: std::sync::atomic::AtomicU64::new(0),
            jobs: Default::default(),
        }
    }
    pub(crate) fn with_resolver(mut self, resolver: std::sync::Arc<SecretResolver>) -> Self {
        self.resolver = resolver;
        self
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.slots
            .lock()
            .unwrap()
            .values()
            .any(|s| s.status == Status::Reading)
    }
    pub(crate) fn with_notifications(mut self, notify: tokio::sync::watch::Sender<u64>) -> Self {
        self.notify = Some(notify);
        self
    }
    pub(crate) fn reconcile(
        &self,
        entries: &Value,
        settings: &Value,
        policy: &crate::managed_policy::Policy,
    ) -> std::collections::BTreeMap<String, Status> {
        use std::sync::atomic::Ordering;
        let runtime = tokio::runtime::Handle::try_current().ok();
        let mut jobs = Vec::new();
        let mut slots = self.slots.lock().unwrap();
        slots.retain(|id, slot| entries.get(id).is_some_and(|entry| *entry == slot.raw));
        for (id, raw) in entries.as_object().into_iter().flatten() {
            if !slots.contains_key(id) {
                let ticket = self.ticket.fetch_add(1, Ordering::Relaxed);
                let prepared = policy
                    .driver_denial(raw["driver"].as_str().unwrap_or(""))
                    .map_or_else(
                        || Prepared::new(raw),
                        |reason| Err(PreparationError::Blocked(reason)),
                    );
                let (prepared, status) = match prepared {
                    Err(error) => (None, Status::Unavailable(error.message(id))),
                    Ok(prepared) => {
                        let status = if prepared.enabled && !prepared.references().is_empty() {
                            Status::Reading
                        } else {
                            let resolution = prepared.with_values(&[]);
                            Status::Ready {
                                entry: prepared.resolved_entry(&resolution),
                                secrets: resolution.secrets,
                            }
                        };
                        (Some(prepared), status)
                    }
                };
                slots.insert(
                    id.clone(),
                    Slot {
                        raw: raw.clone(),
                        prepared,
                        status,
                        ticket,
                        started: false,
                    },
                );
            }
            let slot = slots.get_mut(id).unwrap();
            if slot.status == Status::Reading && !slot.started && runtime.is_some() {
                slot.started = true;
                jobs.push((id.clone(), slot.ticket, slot.prepared.clone().unwrap()));
            }
        }
        let statuses = slots
            .iter()
            .map(|(id, slot)| (id.clone(), slot.status.clone()))
            .collect();
        drop(slots);
        if let Some(runtime) = runtime.filter(|_| !jobs.is_empty()) {
            let resolver = self.resolver.clone();
            let providers = settings["secretProviders"].clone();
            let slots = self.slots.clone();
            let changed = self.changed.clone();
            let notify = self.notify.clone();
            let task = runtime.spawn(async move {
                // Prefetch across instances, so one provider unlock serves the whole rebuild.
                let references = jobs
                    .iter()
                    .flat_map(|(_, _, p)| p.references())
                    .collect::<Vec<_>>();
                resolver.read(&providers, &references).await;
                for (id, ticket, prepared) in jobs {
                    let resolution = prepared.resolve(&resolver, &providers).await;
                    let status = match resolution.reason() {
                        Some(reason) => Status::Unavailable(reason),
                        None => Status::Ready {
                            entry: prepared.resolved_entry(&resolution),
                            secrets: resolution.secrets,
                        },
                    };
                    let mut slots = slots.lock().unwrap();
                    if let Some(slot) = slots
                        .get_mut(&id)
                        .filter(|s| s.ticket == ticket && s.raw == prepared.raw)
                    {
                        slot.status = status;
                    }
                }
                changed.send_modify(|v| *v = v.wrapping_add(1));
                if let Some(notify) = notify {
                    notify.send_modify(|v| *v = v.wrapping_add(1));
                }
            });
            let mut jobs = self.jobs.lock().unwrap();
            jobs.retain(|job| !job.is_finished());
            jobs.push(task.abort_handle());
        }
        statuses
    }
    pub(crate) async fn wait(&self) {
        let mut changes = self.changed.subscribe();
        loop {
            if !self
                .slots
                .lock()
                .unwrap()
                .values()
                .any(|s| s.status == Status::Reading)
            {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
    pub(crate) async fn reload(
        &self,
        entries: &Value,
        settings: &Value,
        policy: &crate::managed_policy::Policy,
        instance: Option<&str>,
    ) {
        self.reconcile(entries, settings, policy);
        self.wait().await;
        let targets = self
            .slots
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| instance.is_none_or(|wanted| wanted == id.as_str()))
            .filter_map(|(id, slot)| {
                slot.prepared
                    .as_ref()
                    .filter(|p| p.enabled && !p.references().is_empty())
                    .map(|p| (id.clone(), slot.ticket, p.clone()))
            })
            .collect::<Vec<_>>();
        let references = targets
            .iter()
            .flat_map(|(_, _, p)| p.references())
            .collect::<Vec<_>>();
        if references.is_empty() {
            return;
        }
        self.resolver.invalidate(Some(&references)).await;
        let providers = &settings["secretProviders"];
        self.resolver.read(providers, &references).await;
        let mut any = false;
        for (id, ticket, prepared) in targets {
            let resolution = prepared.resolve(&self.resolver, providers).await;
            let mut slots = self.slots.lock().unwrap();
            let Some(slot) = slots
                .get_mut(&id)
                .filter(|s| s.ticket == ticket && s.raw == prepared.raw)
            else {
                continue;
            };
            let status = if let Some(reason) = resolution.reason() {
                if matches!(slot.status, Status::Ready { .. }) {
                    continue;
                }
                Status::Unavailable(reason)
            } else {
                Status::Ready {
                    entry: prepared.resolved_entry(&resolution),
                    secrets: resolution.secrets,
                }
            };
            if slot.status != status {
                slot.status = status;
                any = true
            }
        }
        if any {
            self.changed.send_modify(|v| *v = v.wrapping_add(1));
            if let Some(notify) = &self.notify {
                notify.send_modify(|v| *v = v.wrapping_add(1));
            }
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        for job in self.jobs.lock().unwrap().iter() {
            job.abort();
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/provider_secrets.rs"]
mod tests;
