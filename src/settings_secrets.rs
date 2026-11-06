//! Native schema-declared settings secret traversal and reference resolution.
use crate::{
    managed_settings_codec as codec,
    secret_resolver::{SecretReference, SecretResolver},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;
const REDACTED: &str = "••••••";
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    secret_fields: Vec<String>,
    reference_fields: Vec<ReferenceField>,
}
#[derive(Clone, Deserialize)]
struct ReferenceField {
    field: String,
    keyed: bool,
}
static DESCRIPTORS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("settings-secret-fields-4288.json"))
        .expect("native secret descriptors")
});
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    pub secret_name: String,
    pub kind: &'static str,
    pub value: String,
    pub redacted: bool,
    pub sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_variable: Option<String>,
}
#[derive(Clone)]
pub struct SecretValue {
    pub value: String,
    pub redacted: bool,
}
#[derive(Clone)]
struct Group {
    section: &'static str,
    key: Key,
    nested: Option<&'static str>,
    descriptor: Descriptor,
    identity: Vec<String>,
}
#[derive(Clone)]
enum Key {
    Named(String),
    Index(usize),
}
fn encode(value: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.as_bytes())
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn reference(value: &Value) -> Option<SecretReference> {
    codec::is_secret_reference(value)
        .then(|| SecretReference::from_decoded(value))
        .flatten()
}
fn descriptor(value: &Value) -> Descriptor {
    serde_json::from_value(value.clone()).unwrap_or_else(|_| Descriptor {
        secret_fields: Vec::new(),
        reference_fields: Vec::new(),
    })
}
fn entries(value: &Value) -> Vec<(String, Value)> {
    let mut entries: Vec<_> = match value {
        Value::Object(v) => v.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Array(v) => v
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect(),
        _ => Vec::new(),
    };
    entries.sort_by_key(|(k, _)| codec::array_index(k).map_or((1, 0), |i| (0, i)));
    entries
}
impl Group {
    fn value<'a>(&self, settings: &'a Value) -> &'a Value {
        let entry = match &self.key {
            Key::Named(key) => &settings[self.section][key],
            Key::Index(index) => &settings[self.section][*index],
        };
        self.nested.map_or(entry, |key| &entry[key])
    }
    fn set(&self, settings: &mut Value, value: Value) {
        let entry = match &self.key {
            Key::Named(key) => &mut settings[self.section][key],
            Key::Index(index) => &mut settings[self.section][*index],
        };
        if let Some(key) = self.nested {
            entry[key] = value
        } else {
            *entry = value
        }
    }
    fn name(&self, field: &str, host: Option<&str>) -> String {
        let encoded = self.identity.iter().map(|v| encode(v)).collect::<Vec<_>>();
        let mut name = match self.section {
            "providerInstances" => format!(
                "provider-config-{}-{}-{}",
                encoded[0],
                encoded[1],
                encode(field)
            ),
            "sourceControlHosts" => format!("source-control-{}-{}", encoded[0], encode(field)),
            "usageLimitSources" => {
                if field == "managementKey" && host.is_none() {
                    format!("usage-limit-source-{}", encoded[0])
                } else {
                    format!("usage-limit-source-{}-{}", encoded[0], encode(field))
                }
            }
            "easDeviceHosts" => format!("device-host-{}-{}", encoded[0], encode(field)),
            "simlockDeviceHosts" => format!("simlock-device-host-{}-{}", encoded[0], encode(field)),
            _ => unreachable!(),
        };
        if let Some(host) = host {
            name.push('-');
            name.push_str(&encode(host));
        }
        name
    }
}
fn groups(settings: &Value) -> Vec<Group> {
    let mut groups = Vec::new();
    for (id, instance) in entries(&settings["providerInstances"]) {
        if instance["config"].is_object() {
            let driver = instance["driver"].as_str().unwrap_or("");
            groups.push(Group {
                section: "providerInstances",
                key: Key::Named(id.clone()),
                nested: Some("config"),
                descriptor: descriptor(&DESCRIPTORS["providers"][driver]),
                identity: vec![id, driver.into()],
            });
        }
    }
    for (kind, fields) in entries(&settings["sourceControlHosts"]) {
        let _ = fields;
        groups.push(Group {
            section: "sourceControlHosts",
            key: Key::Named(kind.clone()),
            nested: None,
            descriptor: descriptor(&DESCRIPTORS["hosts"][&kind]),
            identity: vec![kind],
        });
    }
    for (id, _) in entries(&settings["usageLimitSources"]) {
        groups.push(Group {
            section: "usageLimitSources",
            key: Key::Named(id.clone()),
            nested: None,
            descriptor: descriptor(&DESCRIPTORS["usage"]),
            identity: vec![id],
        });
    }
    for (section, key) in [("easDeviceHosts", "eas"), ("simlockDeviceHosts", "simlock")] {
        for (index, host) in settings[section]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            groups.push(Group {
                section,
                key: Key::Index(index),
                nested: None,
                descriptor: descriptor(&DESCRIPTORS[key]),
                identity: vec![host["id"].as_str().unwrap_or("").into()],
            });
        }
    }
    groups
}
fn read_values(value: &Value) -> Vec<(Option<String>, String)> {
    if let Some(value) = value.as_str() {
        return vec![(None, value.into())];
    }
    if reference(value).is_some() || !value.is_object() {
        return Vec::new();
    }
    entries(value)
        .into_iter()
        .filter_map(|(host, value)| {
            value
                .as_str()
                .map(|v| (Some(trim(&host).to_lowercase()), v.into()))
        })
        .collect()
}
fn write_values(original: &Value, values: Vec<(Option<String>, String)>) -> Value {
    if original.is_string() {
        return json!(values.first().map_or("", |(_, v)| v.as_str()));
    }
    if reference(original).is_some() {
        return original.clone();
    }
    let mut next = serde_json::Map::new();
    if original.is_object() {
        for (host, value) in entries(original) {
            if reference(&value).is_some() {
                next.insert(trim(&host).to_lowercase(), value);
            }
        }
    }
    for (host, value) in values {
        next.insert(host.unwrap_or_default(), json!(value));
    }
    Value::Object(next)
}
fn map_literal_group(
    group: &Group,
    settings: &Value,
    next: &mut Value,
    map: &mut impl FnMut(Secret) -> SecretValue,
) {
    let original = group.value(settings);
    let mut fields = original.as_object().cloned().unwrap_or_default();
    for field in &group.descriptor.secret_fields {
        let Some(value) = fields.get(field).cloned() else {
            continue;
        };
        if reference(&value).is_some() {
            continue;
        }
        let values = read_values(&value)
            .into_iter()
            .map(|(host, value)| {
                let result = map(Secret {
                    secret_name: group.name(field, host.as_deref()),
                    kind: "field",
                    redacted: value == REDACTED,
                    sensitive: true,
                    value,
                    provider_instance_id: None,
                    environment_variable: None,
                });
                (
                    host,
                    if result.redacted {
                        REDACTED.into()
                    } else {
                        result.value
                    },
                )
            })
            .collect();
        fields.insert(field.clone(), write_values(&value, values));
    }
    group.set(next, Value::Object(fields));
}
pub fn map_secrets(settings: &Value, mut map: impl FnMut(Secret) -> SecretValue) -> Value {
    let mut next = settings.clone();
    let mut field_groups = groups(settings);
    for (id, instance) in entries(&settings["providerInstances"]) {
        if let Some(index) = field_groups.iter().position(|g| {
            g.section == "providerInstances" && matches!(&g.key,Key::Named(key)if *key==id)
        }) {
            let group = field_groups.remove(index);
            map_literal_group(&group, settings, &mut next, &mut map);
        }
        if let Some(environment) = instance["environment"].as_array() {
            let mapped = environment
                .iter()
                .map(|variable| {
                    let mut rest = variable.as_object().cloned().unwrap_or_default();
                    rest.remove("valueRedacted");
                    let literal = variable["value"].as_str();
                    let result = map(Secret {
                        secret_name: format!(
                            "provider-env-{}-{}",
                            encode(&id),
                            encode(variable["name"].as_str().unwrap_or(""))
                        ),
                        kind: "environment",
                        value: literal.unwrap_or("").into(),
                        redacted: literal.is_some() && variable["valueRedacted"] == true,
                        sensitive: variable["sensitive"] == true,
                        provider_instance_id: Some(id.clone()),
                        environment_variable: Some(variable["name"].as_str().unwrap_or("").into()),
                    });
                    if literal.is_none() {
                        rest.insert("value".into(), variable["value"].clone());
                    } else {
                        rest.insert("value".into(), json!(result.value));
                        if result.redacted {
                            rest.insert("valueRedacted".into(), json!(true));
                        }
                    }
                    Value::Object(rest)
                })
                .collect::<Vec<_>>();
            next["providerInstances"][&id]["environment"] = json!(mapped);
        }
    }
    for group in field_groups {
        map_literal_group(&group, settings, &mut next, &mut map);
    }
    next
}
pub fn map_values(settings: &Value, mut map: impl FnMut(Value, String) -> Value) -> Value {
    let mut next = settings.clone();
    for group in groups(settings) {
        let mut fields = group
            .value(settings)
            .as_object()
            .cloned()
            .unwrap_or_default();
        for field in &group.descriptor.secret_fields {
            let Some(value) = fields.get(field).cloned() else {
                continue;
            };
            let mapped = if value.is_object() && reference(&value).is_none() {
                Value::Object(
                    entries(&value)
                        .into_iter()
                        .map(|(host, value)| {
                            let result = if value.is_string() || reference(&value).is_some() {
                                map(value, group.name(field, Some(&trim(&host).to_lowercase())))
                            } else {
                                value
                            };
                            (host, result)
                        })
                        .collect(),
                )
            } else if value.is_string() || reference(&value).is_some() {
                map(value, group.name(field, None))
            } else {
                value
            };
            fields.insert(field.clone(), mapped);
        }
        group.set(&mut next, Value::Object(fields));
    }
    next
}
pub fn without_removed_providers(current: &Value, next: &Value) -> Value {
    let removed = entries(&current["secretProviders"])
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| next["secretProviders"].get(id).is_none())
        .collect::<std::collections::HashSet<_>>();
    if removed.is_empty() {
        return next.clone();
    }
    let is_removed =
        |value: &Value| reference(value).is_some_and(|r| removed.contains(r.provider()));
    let mut fields = map_values(
        next,
        |value, _| if is_removed(&value) { json!("") } else { value },
    );
    for (id, instance) in entries(&fields["providerInstances"]) {
        if let Some(environment) = instance["environment"].as_array() {
            fields["providerInstances"][&id]["environment"] = Value::Array(
                environment
                    .iter()
                    .filter(|v| !is_removed(&v["value"]))
                    .cloned()
                    .collect(),
            );
        }
    }
    fields
}
fn references_in(
    fields: &Value,
    descriptor: &Descriptor,
) -> Vec<(String, Option<String>, SecretReference)> {
    let mut out = Vec::new();
    for field in &descriptor.reference_fields {
        let value = &fields[&field.field];
        if !field.keyed {
            if let Some(reference) = reference(value) {
                out.push((field.field.clone(), None, reference));
            }
        } else if value.is_object() || value.is_array() {
            for (key, value) in entries(value) {
                if let Some(reference) = reference(&value) {
                    out.push((field.field.clone(), Some(key), reference));
                }
            }
        }
    }
    out
}
pub fn collect_settings_references(settings: &Value) -> Vec<SecretReference> {
    let mut out = Vec::new();
    for (kind, metadata) in entries(&DESCRIPTORS["hosts"]) {
        out.extend(
            references_in(
                &settings["sourceControlHosts"][&kind],
                &descriptor(&metadata),
            )
            .into_iter()
            .map(|(_, _, r)| r),
        );
    }
    out.extend(
        entries(&settings["usageLimitSources"])
            .iter()
            .filter_map(|(_, v)| reference(&v["managementKey"])),
    );
    for (section, field) in [
        ("easDeviceHosts", "accessToken"),
        ("simlockDeviceHosts", "agentToken"),
    ] {
        out.extend(
            settings[section]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| reference(&v[field])),
        );
    }
    out
}
pub async fn invalidate(resolver: &SecretResolver, settings: Option<&Value>) {
    if let Some(settings) = settings {
        let references = collect_settings_references(settings);
        resolver.invalidate(Some(&references)).await
    } else {
        resolver.invalidate(None).await
    }
}
pub async fn resolve_fields(
    resolver: &SecretResolver,
    providers: &Value,
    category: &str,
    kind: Option<&str>,
    fields: &Value,
) -> Value {
    let metadata = kind.map_or(&DESCRIPTORS[category], |kind| &DESCRIPTORS[category][kind]);
    let slots = references_in(fields, &descriptor(metadata));
    if slots.is_empty() {
        return fields.clone();
    }
    let references = slots.iter().map(|(_, _, r)| r.clone()).collect::<Vec<_>>();
    let values = resolver.read(providers, &references).await;
    let mut next = fields.clone();
    for ((field, key, _), value) in slots.into_iter().zip(values) {
        if let Some(value) = value.filter(|v| !trim(v).is_empty()) {
            if let Some(key) = key {
                if !next[&field].is_object() {
                    next[&field] = Value::Object(entries(&next[&field]).into_iter().collect());
                }
                next[&field][key] = json!(value)
            } else {
                next[&field] = json!(value)
            }
        }
    }
    next
}
pub fn require_literal(value: &Value, providers: &Value) -> Result<String, String> {
    if let Some(reference) = reference(value) {
        return Err(format!("Could not read the credential from the secret provider \"{}\". Unlock or fix it in Settings, then try again.",SecretResolver::label(providers,reference.provider())));
    }
    Ok(value.as_str().unwrap_or("").into())
}

#[cfg(test)]
#[path = "../tests/support/settings_secrets.rs"]
mod tests;

/// Read native private slots into a consumer snapshot; the persisted settings are never changed.
pub fn materialize_profile(profile: &std::path::Path, settings: &Value) -> Result<Value, String> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let mut decoded = settings.clone();
    for (id, entry) in entries(&settings["providerInstances"]) {
        if let Ok(entry) = codec::decode_provider_instance(entry) {
            decoded["providerInstances"][id] = entry;
        }
    }
    let mut pending = indexmap::IndexMap::new();
    map_secrets(&decoded, |secret| {
        if secret.sensitive && secret.redacted {
            pending.insert(secret.secret_name.clone(), secret.clone());
        }
        SecretValue {
            value: secret.value,
            redacted: secret.redacted,
        }
    });
    let mut values = std::collections::HashMap::new();
    for secret in pending.values() {
        let path = profile
            .join("secrets")
            .join(format!("{}.bin", secret.secret_name));
        let value = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(mut file) => {
                if !file.metadata().is_ok_and(|m| m.is_file()) {
                    return Err(format!("Failed to read secret {}.", secret.secret_name));
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|_| format!("Failed to read secret {}.", secret.secret_name))?;
                String::from_utf8_lossy(&bytes).into_owned()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return Err(format!("Failed to read secret {}.", secret.secret_name)),
        };
        values.insert(secret.secret_name.clone(), value);
    }
    let mut materialized = map_secrets(&decoded, |secret| SecretValue {
        value: if secret.sensitive && secret.redacted {
            values.get(&secret.secret_name).cloned().unwrap_or_default()
        } else {
            secret.value
        },
        redacted: secret.sensitive && secret.kind == "environment" && secret.redacted,
    });
    for (id, entry) in entries(&materialized["providerInstances"]) {
        if let Ok(entry) = codec::encode_provider_instance(entry) {
            materialized["providerInstances"][id] = entry;
        }
    }
    Ok(materialized)
}
