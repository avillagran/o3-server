//! Native terminal launch environment; runtime variables stay out of snapshot metadata.
use crate::store::State;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

const BUILT_INS: &[&str] = &[
    "codex",
    "claudeAgent",
    "cursor",
    "grok",
    "fx",
    "opencode",
    "antigravity",
    "kiro",
    "pi",
    "muse",
];
fn preparation(id: &str, cause: Value) -> Value {
    json!({"_tag":"TerminalProviderEnvironmentError","providerInstanceId":id,"cause":cause,
        "message":format!("Could not prepare the terminal environment for provider instance: {id}")})
}
fn normalize_join(home: &Path, suffix: &str) -> String {
    let joined = if suffix.is_empty() {
        home.to_path_buf()
    } else {
        home.join(suffix.trim_start_matches('/'))
    };
    let rooted = joined.is_absolute();
    let trailing = joined.as_os_str().as_encoded_bytes().ends_with(b"/");
    let mut result = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    result.pop();
                } else if !rooted {
                    result.push("..");
                }
            }
            part => result.push(part.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    if trailing && result != Path::new("/") {
        result.push("");
    }
    result.to_string_lossy().into_owned()
}
fn expand(value: &str, home: &Path) -> String {
    if value == "~" {
        home.to_string_lossy().into_owned()
    } else if value.starts_with("~/") || value.starts_with("~\\") {
        normalize_join(home, &value[2..])
    } else {
        value.to_owned()
    }
}
pub(crate) fn resolve_with(
    settings: &Value,
    id: &str,
    mut env: BTreeMap<String, String>,
    home: &Path,
    cwd: &Path,
) -> Result<BTreeMap<String, String>, Value> {
    let fallback;
    let entry = if let Some(entry) = settings["providerInstances"].get(id) {
        entry
    } else if BUILT_INS.contains(&id) {
        fallback = json!({"driver":id});
        &fallback
    } else if matches!(
        id,
        "constructor"
            | "toString"
            | "toLocaleString"
            | "valueOf"
            | "hasOwnProperty"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
    ) {
        // The native lookup uses a plain object; inherited methods have no environment.
        return Ok(env);
    } else {
        return Err(
            json!({"_tag":"TerminalProviderInstanceNotFoundError","providerInstanceId":id,
        "message":format!("Provider instance is not available: {id}")}),
        );
    };
    for variable in entry["environment"].as_array().into_iter().flatten() {
        if let (Some(name), Some(value)) = (variable["name"].as_str(), variable["value"].as_str()) {
            let value = if matches!(name, "CODEX_HOME" | "CLAUDE_CONFIG_DIR") {
                expand(value, home)
            } else {
                value.to_owned()
            };
            // Native assignment cannot create this inherited setter as a string property.
            if name != "__proto__" || env.contains_key(name) {
                env.insert(name.to_owned(), value);
            }
        }
    }
    let driver = entry["driver"].as_str().unwrap_or("");
    if matches!(driver, "codex" | "claudeAgent") {
        let config = entry
            .get("config")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Ok(config) = crate::managed_settings_codec::decode_provider_config(driver, config) {
            let primary = config["homePath"].as_str().unwrap_or("");
            let shadow = crate::native_goal::trim(config["shadowHomePath"].as_str().unwrap_or(""));
            let configured = if driver == "codex" && !shadow.is_empty() {
                Some(shadow)
            } else if !crate::native_goal::trim(primary).is_empty() {
                Some(if driver == "claudeAgent" {
                    crate::native_goal::trim(primary)
                } else {
                    primary
                })
            } else {
                None
            };
            if let Some(value) = configured {
                let path = crate::provider_home::absolute(Path::new(&expand(value, home)), cwd);
                env.insert(
                    if driver == "codex" {
                        "CODEX_HOME"
                    } else {
                        "CLAUDE_CONFIG_DIR"
                    }
                    .into(),
                    path.to_string_lossy().into_owned(),
                );
            }
        }
    }
    Ok(env)
}
pub(crate) fn resolve(
    state: &State,
    id: &str,
    env: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Value> {
    let settings =
        crate::settings::read_materialized(state).map_err(|cause| preparation(id, json!(cause)))?;
    let home = crate::provider_home::host_home().map_err(|cause| preparation(id, json!(cause)))?;
    let cwd = std::env::current_dir().map_err(|cause| preparation(id, json!(cause.to_string())))?;
    resolve_with(&settings, id, env, &home, &cwd)
}
