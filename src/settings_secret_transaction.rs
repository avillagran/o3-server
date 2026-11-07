//! Native private-settings planning, materialization, redaction and reverse rollback.
//! This leaf does not alter authentication keys or activate persistence consumers.
use crate::{
    server_secret_store::{SecretStore, StoreError},
    settings_secrets::{self, Secret, SecretValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
const REDACTED: &str = "••••••";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: String,
    pub secret_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_variable: Option<String>,
}
impl Change {
    fn new(secret: &Secret, value: Option<Vec<u8>>, operation: Option<&str>) -> Self {
        Self {
            kind: if value.is_some() { "write" } else { "remove" }.into(),
            secret_name: secret.secret_name.clone(),
            value,
            operation: operation.map(str::to_owned),
            provider_instance_id: secret.provider_instance_id.clone(),
            environment_variable: secret.environment_variable.clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionError {
    pub operation: String,
    pub provider_instance_id: Option<String>,
    pub environment_variable: Option<String>,
    pub cause: Box<StoreError>,
    pub rollback_errors: Vec<StoreError>,
}
impl TransactionError {
    fn new(
        operation: &str,
        id: Option<String>,
        environment: Option<String>,
        cause: StoreError,
    ) -> Self {
        Self {
            operation: operation.into(),
            provider_instance_id: id,
            environment_variable: environment,
            cause: Box::new(cause),
            rollback_errors: Vec::new(),
        }
    }
}
fn is_reference(value: &Value) -> bool {
    crate::secret_resolver::SecretReference::decode(value).is_some()
}
pub fn echo(current: &Value, next: &Value) -> Value {
    let mut references = HashMap::new();
    settings_secrets::map_values(current, |value, name| {
        if is_reference(&value) {
            references.insert(name, value.clone());
        }
        value
    });
    settings_secrets::map_values(next, |value, name| {
        if value == REDACTED {
            references.get(&name).cloned().unwrap_or(value)
        } else {
            value
        }
    })
}
pub fn redact(settings: &Value, references: bool) -> Value {
    let redacted = settings_secrets::map_secrets(settings, |secret| {
        if secret.sensitive {
            SecretValue {
                value: String::new(),
                redacted: secret.redacted || !secret.value.is_empty(),
            }
        } else {
            SecretValue {
                value: secret.value,
                redacted: false,
            }
        }
    });
    let mut result = if references {
        redacted
    } else {
        settings_secrets::map_values(&redacted, |value, _| {
            if is_reference(&value) {
                json!(REDACTED)
            } else {
                value
            }
        })
    };
    if let Some(providers) = result["secretProviders"].as_object_mut() {
        for provider in providers.values_mut() {
            if provider["kind"] != "one_password" {
                if let Some(fields) = provider.as_object_mut() {
                    fields.remove("config");
                }
            }
        }
    }
    result
}
pub fn plan(current: &Value, next: &Value) -> (Value, Vec<Change>) {
    let mut previous = Vec::<Secret>::new();
    settings_secrets::map_secrets(current, |secret| {
        if let Some(old) = previous
            .iter_mut()
            .find(|s| s.secret_name == secret.secret_name)
        {
            *old = secret.clone();
        } else {
            previous.push(secret.clone());
        }
        SecretValue {
            value: secret.value,
            redacted: secret.redacted,
        }
    });
    let mut changes = Vec::new();
    let mut retained = HashSet::new();
    let settings = settings_secrets::map_secrets(&echo(current, next), |secret| {
        if !secret.sensitive {
            changes.push(Change::new(&secret, None, Some("remove-secret")));
            return SecretValue {
                value: secret.value,
                redacted: false,
            };
        }
        let old = previous
            .iter()
            .find(|s| s.secret_name == secret.secret_name);
        let inline = old
            .filter(|s| s.sensitive && !s.redacted && !s.value.is_empty())
            .map(|s| s.value.clone());
        let value = if secret.redacted {
            inline
        } else {
            Some(secret.value.clone())
        };
        let Some(value) = value else {
            retained.insert(secret.secret_name);
            return SecretValue {
                value: String::new(),
                redacted: true,
            };
        };
        if value.is_empty() {
            changes.push(Change::new(&secret, None, Some("remove-secret")));
            return SecretValue {
                value,
                redacted: false,
            };
        }
        retained.insert(secret.secret_name.clone());
        changes.push(Change::new(&secret, Some(value.into_bytes()), None));
        SecretValue {
            value: String::new(),
            redacted: true,
        }
    });
    for secret in previous {
        if secret.sensitive
            && !retained.contains(&secret.secret_name)
            && !changes.iter().any(|c| c.secret_name == secret.secret_name)
        {
            changes.push(Change::new(&secret, None, Some("remove-stale-secret")));
        }
    }
    (settings, changes)
}
pub fn materialize(store: &impl SecretStore, settings: &Value) -> Result<Value, TransactionError> {
    let mut pending = Vec::new();
    settings_secrets::map_secrets(settings, |secret| {
        if secret.sensitive && secret.redacted {
            if let Some(index) = pending
                .iter()
                .position(|s: &Secret| s.secret_name == secret.secret_name)
            {
                pending[index] = secret.clone();
            } else {
                pending.push(secret.clone());
            }
        }
        SecretValue {
            value: secret.value,
            redacted: secret.redacted,
        }
    });
    let mut values = HashMap::new();
    for secret in pending {
        let bytes = store
            .get(&secret.secret_name)
            .map_err(|e| {
                TransactionError::new(
                    "read-secret",
                    secret.provider_instance_id,
                    secret.environment_variable,
                    e,
                )
            })?
            .unwrap_or_default();
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        values.insert(
            secret.secret_name,
            String::from_utf8_lossy(bytes).into_owned(),
        );
    }
    Ok(settings_secrets::map_secrets(settings, |secret| {
        SecretValue {
            value: if secret.sensitive && secret.redacted {
                values.get(&secret.secret_name).cloned().unwrap_or_default()
            } else {
                secret.value
            },
            redacted: secret.sensitive && secret.kind == "environment" && secret.redacted,
        }
    }))
}
#[derive(Clone)]
pub struct Applied {
    name: String,
    previous: Option<Vec<u8>>,
}
pub fn rollback(store: &impl SecretStore, writes: &[Applied]) -> Vec<StoreError> {
    writes
        .iter()
        .rev()
        .filter_map(|write| {
            match &write.previous {
                Some(value) => store.set(&write.name, value),
                None => store.remove(&write.name),
            }
            .err()
        })
        .collect()
}
pub fn apply(
    store: &impl SecretStore,
    changes: &[Change],
) -> Result<Vec<Applied>, TransactionError> {
    let mut applied = Vec::new();
    for change in changes {
        let result = (|| {
            let previous = store.get(&change.secret_name).map_err(|e| {
                TransactionError::new(
                    "read-secret",
                    change.provider_instance_id.clone(),
                    change.environment_variable.clone(),
                    e,
                )
            })?;
            applied.push(Applied {
                name: change.secret_name.clone(),
                previous,
            });
            let operation = if change.kind == "write" {
                "write-secret"
            } else {
                change.operation.as_deref().unwrap_or("remove-secret")
            };
            match &change.value {
                Some(value) => store.set(&change.secret_name, value),
                None => store.remove(&change.secret_name),
            }
            .map_err(|e| {
                TransactionError::new(
                    operation,
                    change.provider_instance_id.clone(),
                    change.environment_variable.clone(),
                    e,
                )
            })
        })();
        if let Err(mut error) = result {
            error.rollback_errors = rollback(store, &applied);
            return Err(error);
        }
    }
    Ok(applied)
}
