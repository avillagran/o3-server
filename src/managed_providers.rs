//! Provider launch overlays and write preservation from nightly 4262 managedProviders.
use super::{Policy, ProviderInstance};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::HashSet;

const DRIVERS: &[&str] = &[
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Environment {
    All,
    Locked,
    Fill,
}
struct Rule<'a> {
    pin: Option<&'a ProviderInstance>,
    config: Vec<&'static str>,
    fill: Vec<&'static str>,
    environment: Environment,
}
fn key(name: &str, windows: bool) -> String {
    if windows {
        name.to_ascii_uppercase()
    } else {
        name.into()
    }
}
fn launch_fields(driver: &str) -> Vec<&'static str> {
    match driver {
        "codex" => vec!["binaryPath", "launchArgs", "homePath", "shadowHomePath"],
        "claudeAgent" => vec!["binaryPath", "launchArgs", "homePath"],
        "pi" => vec!["binaryPath", "launchArgs"],
        "opencode" => vec!["binaryPath", "serverUrl"],
        "acpRegistry" => vec!["commandPath", "commandArgs"],
        _ => vec!["binaryPath"],
    }
}
fn pinned_value(pin: Option<&ProviderInstance>, field: &str) -> Option<Value> {
    match field {
        "binaryPath" => pin?.binary_path.as_ref().map(|v| json!(v.as_str())),
        "launchArgs" => pin?.launch_args.as_ref().map(|v| json!(v.as_str())),
        _ => None,
    }
}
fn rules(policy: &Policy, windows: bool) -> IndexMap<&str, Rule<'_>> {
    let Some(document) = &policy.document else {
        return IndexMap::new();
    };
    let Some(limits) = &document.limits else {
        return IndexMap::new();
    };
    let pins = limits.providers.as_ref().and_then(|v| v.instances.as_ref());
    let mut rules = IndexMap::new();
    for driver in DRIVERS {
        let pin = pins.and_then(|v| {
            v.iter()
                .find(|(k, _)| k.as_str() == *driver)
                .map(|(_, p)| p)
        });
        let bypass = if pin.is_some() {
            match *driver {
                "codex" => vec!["setupMode"],
                "opencode" => vec!["serverUrl"],
                _ => vec![],
            }
        } else {
            vec![]
        };
        if limits.max_runtime_mode.is_some() {
            let mut config = launch_fields(driver);
            config.extend(bypass);
            rules.insert(
                *driver,
                Rule {
                    pin,
                    config,
                    fill: vec![],
                    environment: Environment::All,
                },
            );
        } else if let Some(pin) = pin {
            let fields = ["binaryPath", "launchArgs"]
                .into_iter()
                .filter(|f| pinned_value(Some(pin), f).is_some())
                .collect::<Vec<_>>();
            let home = match *driver {
                "codex" => Some(("CODEX_HOME", vec!["homePath", "shadowHomePath"])),
                "claudeAgent" => Some(("CLAUDE_CONFIG_DIR", vec!["homePath"])),
                _ => None,
            };
            let home_fields = home
                .filter(|(name, _)| {
                    pin.environment
                        .iter()
                        .flatten()
                        .any(|v| key(v.name.as_str(), windows) == key(name, windows))
                })
                .map(|(_, fields)| fields)
                .unwrap_or_default();
            let (config, fill, environment) = if pin.locked {
                let mut config = fields;
                config.extend(bypass);
                config.extend(home_fields);
                (config, vec![], Environment::Locked)
            } else {
                (vec![], fields, Environment::Fill)
            };
            rules.insert(
                *driver,
                Rule {
                    pin: Some(pin),
                    config,
                    fill,
                    environment,
                },
            );
        }
    }
    rules
}
fn pinned_environment(rule: &Rule<'_>) -> Vec<Value> {
    rule.pin
        .iter()
        .flat_map(|p| p.environment.iter().flatten())
        .map(|v| json!({"name":v.name.as_str(),"value":v.value,"sensitive":false}))
        .collect()
}
fn variables(instance: &Value) -> Vec<Value> {
    instance["environment"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
fn config_value<'a>(instance: &'a Value, field: &str) -> Option<&'a Value> {
    instance["config"]
        .get(field)
        .filter(|v| *v != "" && !v.as_array().is_some_and(Vec::is_empty))
}
fn overlay(instance: &Value, rule: &Rule<'_>, windows: bool) -> Value {
    let mut result = instance.clone();
    let mut config = instance["config"].as_object().cloned().unwrap_or_default();
    for field in &rule.config {
        if let Some(value) = pinned_value(rule.pin, field) {
            config.insert((*field).into(), value);
        } else {
            config.remove(*field);
        }
    }
    for field in &rule.fill {
        if config_value(instance, field).is_none() {
            if let Some(value) = pinned_value(rule.pin, field) {
                config.insert((*field).into(), value);
            }
        }
    }
    let pinned = pinned_environment(rule);
    let mut environment = if rule.environment == Environment::All {
        pinned.clone()
    } else {
        vec![]
    };
    if rule.environment != Environment::All {
        let mut seen = HashSet::new();
        for variable in variables(instance) {
            let name = variable["name"].as_str().unwrap_or("");
            let name_key = key(name, windows);
            if rule.config.contains(&"launchArgs")
                && name_key == key("T3CODE_CODEX_LAUNCH_ARGS", windows)
            {
                continue;
            }
            let pin = (rule.environment == Environment::Locked)
                .then(|| {
                    pinned
                        .iter()
                        .find(|p| key(p["name"].as_str().unwrap_or(""), windows) == name_key)
                })
                .flatten();
            if let Some(pin) = pin {
                if !seen.contains(&name_key) {
                    environment.push(pin.clone());
                }
            } else {
                environment.push(variable);
            }
            seen.insert(name_key);
        }
        for pin in pinned {
            if !seen.contains(&key(pin["name"].as_str().unwrap_or(""), windows)) {
                environment.push(pin);
            }
        }
    }
    result.as_object_mut().unwrap().remove("config");
    result.as_object_mut().unwrap().remove("environment");
    if instance.get("config").is_some() || !config.is_empty() {
        result["config"] = Value::Object(config);
    }
    if !environment.is_empty() {
        result["environment"] = json!(environment);
    }
    result
}

pub(super) fn apply(policy: &Policy, settings: &Value, windows: bool) -> Value {
    let rules = rules(policy, windows);
    if rules.is_empty()
        && policy
            .document
            .as_ref()
            .and_then(|v| v.limits.as_ref())
            .and_then(|v| v.providers.as_ref())
            .is_none()
    {
        return settings.clone();
    }
    let mut next = settings.clone();
    let mut instances = settings["providerInstances"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (driver, rule) in &rules {
        if rule.pin.is_some() && *driver != "acpRegistry" && !instances.contains_key(*driver) {
            instances.insert((*driver).into(), json!({"driver":driver}));
        }
    }
    for instance in instances.values_mut() {
        if let Some(rule) = instance["driver"]
            .as_str()
            .and_then(|driver| rules.get(driver))
        {
            *instance = overlay(instance, rule, windows);
        }
    }
    next["providerInstances"] = Value::Object(instances);
    for field in [
        "sourceControlWriterModelSelection",
        "reviewGuideModelSelection",
    ] {
        if let Some(id) = settings[field]["instanceId"].as_str() {
            let driver = settings["providerInstances"][id]["driver"]
                .as_str()
                .unwrap_or(id);
            if policy.driver_denial(driver).is_some() {
                next[field] = Value::Null;
            }
        }
    }
    next
}

pub(super) fn locked_fields(policy: &Policy, windows: bool) -> Value {
    let mut fields = serde_json::Map::new();
    for (driver, rule) in rules(policy, windows) {
        let mut paths = rule
            .config
            .iter()
            .map(|v| format!("config.{v}"))
            .collect::<Vec<_>>();
        if rule.environment == Environment::All {
            paths.push("environment".into());
        } else if rule.environment == Environment::Locked {
            paths.extend(
                rule.pin
                    .iter()
                    .flat_map(|p| p.environment.iter().flatten())
                    .map(|v| format!("environment.{}", v.name.as_str())),
            );
        }
        if !paths.is_empty() {
            fields.insert(driver.into(), json!(paths));
        }
    }
    Value::Object(fields)
}
fn same_variable(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.get("value") == b.get("value") && a.get("sensitive") == b.get("sensitive")
        }
        _ => false,
    }
}
fn echoed(variable: &Value, pinned: &[Value]) -> bool {
    variable["sensitive"] != true
        && pinned
            .iter()
            .any(|p| p["name"] == variable["name"] && p["value"] == variable["value"])
}
#[derive(Debug, PartialEq)]
pub(super) enum WriteError {
    Locked(String),
    Disallowed { instance_id: String, driver: String },
}
impl std::fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked(path) => write!(formatter, "Managed provider field is locked: {path}"),
            Self::Disallowed { driver, .. } => {
                write!(formatter, "Provider is disallowed: {driver}")
            }
        }
    }
}
fn locked(path: &str) -> WriteError {
    WriteError::Locked(path.into())
}
fn vet_environment(
    id: &str,
    stored: Option<&Value>,
    next: &Value,
    rule: &Rule<'_>,
    windows: bool,
) -> Result<Vec<Value>, WriteError> {
    let pinned = pinned_environment(rule);
    let stored_values = stored.map(variables).unwrap_or_default();
    let mut environment = variables(next);
    if rule.environment == Environment::All {
        for variable in &environment {
            if echoed(variable, &pinned)
                || stored_values
                    .iter()
                    .any(|v| same_variable(Some(v), Some(variable)))
            {
                continue;
            }
            return Err(locked(&format!("providerInstances.{id}.environment")));
        }
        return Ok(stored_values);
    }
    for pin in &pinned {
        let name = pin["name"].as_str().unwrap_or("");
        let normalized = key(name, windows);
        let sent = environment
            .iter()
            .find(|v| key(v["name"].as_str().unwrap_or(""), windows) == normalized);
        let kept = stored_values
            .iter()
            .find(|v| key(v["name"].as_str().unwrap_or(""), windows) == normalized);
        if sent.is_some_and(|v| echoed(v, &pinned)) || same_variable(sent, kept) {
            let entries = stored_values
                .iter()
                .filter(|v| key(v["name"].as_str().unwrap_or(""), windows) == normalized)
                .cloned()
                .collect::<Vec<_>>();
            let mut restored = false;
            environment = environment
                .into_iter()
                .flat_map(|v| {
                    if key(v["name"].as_str().unwrap_or(""), windows) != normalized {
                        vec![v]
                    } else if restored {
                        vec![]
                    } else {
                        restored = true;
                        entries.clone()
                    }
                })
                .collect();
        } else if rule.environment == Environment::Locked {
            return Err(locked(&format!(
                "providerInstances.{id}.environment.{name}"
            )));
        }
    }
    if rule.config.contains(&"launchArgs") {
        environment.retain(|v| {
            key(v["name"].as_str().unwrap_or(""), windows)
                != key("T3CODE_CODEX_LAUNCH_ARGS", windows)
        });
        environment.extend(stored_values.into_iter().filter(|v| {
            key(v["name"].as_str().unwrap_or(""), windows)
                == key("T3CODE_CODEX_LAUNCH_ARGS", windows)
        }));
    }
    Ok(environment)
}
fn vet_instance(
    id: &str,
    stored: Option<&Value>,
    next: &Value,
    rule: &Rule<'_>,
    windows: bool,
) -> Result<Option<Value>, WriteError> {
    let mut config = next["config"].as_object().cloned().unwrap_or_default();
    for field in rule.config.iter().chain(rule.fill.iter()) {
        let value = config_value(next, field);
        let previous = stored.and_then(|v| config_value(v, field));
        let pin = pinned_value(rule.pin, field);
        let harmless = match (next["driver"].as_str(), *field) {
            (Some("codex"), "setupMode") => Some(json!("existing")),
            (Some("opencode"), "serverUrl") => Some(json!("")),
            _ => None,
        };
        if value == pin.as_ref()
            || value == previous
            || harmless.as_ref().is_some_and(|v| Some(v) == value)
        {
            if let Some(previous) = previous {
                config.insert((*field).into(), previous.clone());
            } else {
                config.remove(*field);
            }
        } else if rule.config.contains(field) {
            return Err(locked(&format!("providerInstances.{id}.config.{field}")));
        }
    }
    let environment = vet_environment(id, stored, next, rule, windows)?;
    let mut kept = next.clone();
    kept.as_object_mut().unwrap().remove("config");
    kept.as_object_mut().unwrap().remove("environment");
    if !config.is_empty() || stored.is_some_and(|v| v.get("config").is_some()) {
        kept["config"] = Value::Object(config);
    }
    if !environment.is_empty() {
        kept["environment"] = json!(environment);
    }
    if stored.is_none()
        && id == next["driver"].as_str().unwrap_or("")
        && kept.as_object().unwrap().len() == 1
    {
        Ok(None)
    } else {
        Ok(Some(kept))
    }
}
pub(super) fn vet(
    policy: &Policy,
    current: &Value,
    next: &Value,
    windows: bool,
) -> Result<Value, WriteError> {
    let rules = rules(policy, windows);
    let mut instances = serde_json::Map::new();
    for (id, instance) in next["providerInstances"].as_object().into_iter().flatten() {
        let driver = instance["driver"].as_str().unwrap_or("");
        let stored = current["providerInstances"].get(id);
        let was_enabled = stored.map_or(id == driver, |v| {
            v["driver"] == driver && crate::provider_policy::instance_enabled(v)
        });
        if !was_enabled
            && crate::provider_policy::instance_enabled(instance)
            && policy.driver_denial(driver).is_some()
        {
            return Err(WriteError::Disallowed {
                instance_id: id.clone(),
                driver: driver.into(),
            });
        }
        if let Some(rule) = rules.get(driver) {
            if let Some(kept) = vet_instance(
                id,
                stored.filter(|v| v["driver"] == driver),
                instance,
                rule,
                windows,
            )? {
                instances.insert(id.clone(), kept);
            }
        } else {
            instances.insert(id.clone(), instance.clone());
        }
    }
    for (project, overrides) in next["projectSettingsOverrides"]
        .as_object()
        .into_iter()
        .flatten()
    {
        for (id, entry) in overrides["providerInstances"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if entry["enabled"] == true
                && current["projectSettingsOverrides"][project]["providerInstances"][id]["enabled"]
                    != true
            {
                let driver = next["providerInstances"][id]["driver"]
                    .as_str()
                    .unwrap_or(id);
                if policy.driver_denial(driver).is_some() {
                    return Err(WriteError::Disallowed {
                        instance_id: id.clone(),
                        driver: driver.into(),
                    });
                }
            }
        }
    }
    let mut result = next.clone();
    result["providerInstances"] = Value::Object(instances);
    Ok(result)
}

const TEXT_FALLBACKS: &[(&str, &str)] = &[
    ("codex", "gpt-6-luna"),
    ("claudeAgent", "claude-haiku-5-5"),
    ("cursor", "composer-2"),
    ("grok", "grok-build"),
    ("muse", "default"),
    ("pi", "default"),
    ("opencode", "opencode/big-pickle"),
    ("antigravity", "antigravity-default"),
];
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c| matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
pub(super) fn text_generation_fallback(
    policy: &Policy,
    settings: &Value,
    include_configured: bool,
) -> Option<Value> {
    let mut candidates = TEXT_FALLBACKS
        .iter()
        .map(|(driver, _)| ((*driver).to_owned(), *driver))
        .collect::<Vec<_>>();
    if include_configured {
        for (id, instance) in settings["providerInstances"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(driver) = instance["driver"].as_str().and_then(|driver| {
                TEXT_FALLBACKS
                    .iter()
                    .find(|(kind, _)| *kind == driver)
                    .map(|(kind, _)| *kind)
            }) {
                candidates.push((id.clone(), driver));
            }
        }
    }
    let (id, driver) = candidates.into_iter().find(|(id, driver)| {
        let configured = settings["providerInstances"].get(id);
        let actual_driver = configured
            .and_then(|entry| entry["driver"].as_str())
            .unwrap_or(driver);
        policy.driver_denial(actual_driver).is_none()
            && configured.map_or_else(
                || crate::provider_policy::instance_enabled(&json!({"driver":driver})),
                crate::provider_policy::instance_enabled,
            )
    })?;
    let selection = &settings["defaultModelSelection"];
    if selection["instanceId"] == id
        && settings["providerInstances"][&id]["config"]["customModels"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| {
                let slug = entry.as_str().or_else(|| entry["slug"].as_str());
                slug.is_some_and(|slug| {
                    !js_trim(slug).is_empty() && selection["model"] == js_trim(slug)
                })
            })
    {
        return Some(selection.clone());
    }
    Some(
        json!({"instanceId":id,"model":TEXT_FALLBACKS.iter().find(|(kind,_)| *kind==driver).unwrap().1}),
    )
}
pub(super) fn resolve_text_generation(policy: &Policy, settings: &Value) -> Value {
    let selection = &settings["textGenerationModelSelection"];
    let enabled = selection["instanceId"].as_str().is_some_and(|id| {
        let instance = settings["providerInstances"].get(id);
        let driver = instance
            .and_then(|entry| entry["driver"].as_str())
            .unwrap_or(id);
        driver != "acpRegistry"
            && policy.driver_denial(driver).is_none()
            && instance.map_or(
                matches!(id, "codex" | "claudeAgent"),
                crate::provider_policy::instance_enabled,
            )
    });
    if enabled {
        return settings.clone();
    }
    let mut resolved = settings.clone();
    if let Some(fallback) = text_generation_fallback(policy, settings, false) {
        resolved["textGenerationModelSelection"] = fallback;
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn native_text_generation_fallback_oracle() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-providers-4262.json"
        ))
        .unwrap();
        for case in fixture["textGenerationFallback"].as_array().unwrap() {
            let policy = Policy::parse(
                Path::new("/fixture/policy.json"),
                &case["policy"].to_string(),
            )
            .unwrap();
            assert_eq!(
                text_generation_fallback(
                    &policy,
                    &case["current"],
                    case["includeConfiguredInstances"].as_bool().unwrap()
                )
                .unwrap_or(Value::Null),
                case["expected"],
                "{case}"
            );
        }
    }
    #[test]
    fn native_provider_overlay_and_write_oracle() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-providers-4262.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let policy = Policy::parse(
                Path::new("/fixture/policy.json"),
                &case["policy"].to_string(),
            )
            .unwrap();
            let windows = case["platform"] == "win32";
            assert_eq!(
                apply(&policy, &case["current"], windows),
                case["shown"],
                "{name}: overlay"
            );
            assert_eq!(
                locked_fields(&policy, windows),
                case["locks"],
                "{name}: locks"
            );
            for change in case["changes"].as_array().unwrap() {
                let expected = &change["expected"];
                let actual = vet(&policy, &case["current"], &change["next"], windows);
                let label = format!("{name}: {}", change["name"]);
                match expected["_tag"].as_str().unwrap() {
                    "Allowed" => assert_eq!(actual, Ok(expected["settings"].clone()), "{label}"),
                    "Locked" => assert_eq!(
                        actual,
                        Err(locked(expected["path"].as_str().unwrap())),
                        "{label}"
                    ),
                    "Disallowed" => {
                        let id = expected["instanceId"].as_str().unwrap();
                        let driver = change["next"]["providerInstances"][id]["driver"]
                            .as_str()
                            .unwrap_or(id);
                        assert_eq!(
                            actual,
                            Err(WriteError::Disallowed {
                                instance_id: id.into(),
                                driver: driver.into()
                            }),
                            "{label}"
                        );
                    }
                    tag => panic!("Unknown native verdict: {tag}"),
                }
            }
        }
    }
}
