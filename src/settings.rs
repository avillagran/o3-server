//! Settings remain on disk; unknown keys survive updates.
use crate::store::State;
use serde_json::{json, Value};
pub const SENTINEL: &str = "••••••";
pub fn descriptor(s: &State) -> Value {
    json!({"environmentId":s.environment_id,"label":s.label,"platform":{"os":std::env::consts::OS,"arch":match std::env::consts::ARCH {"x86_64"=>"x64","aarch64"=>"arm64",_=>"other"}},"serverVersion":crate::rpc::SERVER_VERSION,"orchestrationProtocolVersion":2,"capabilities":{"repositoryIdentity":true,"threadSettlement":true,"threadSnooze":true,"threadPinning":true,"threadAutoSettleOptOut":true,"threadTitleRegeneration":true}})
}
pub fn auth_descriptor() -> Value {
    json!({"policy":"loopback-browser","bootstrapMethods":["pairing-token"],"sessionMethods":["bearer-access-token"],"sessionCookieName":"t3code_session"})
}
pub fn read(s: &State) -> Result<Value, String> {
    let mut v: Value = serde_json::from_str(include_str!("settings-defaults.json")).unwrap();
    match std::fs::read(s.dir.join("settings.json")) {
        Ok(b) => {
            let stored: Value = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
            merge(&mut v, &stored);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(v)
}
pub fn merge(dst: &mut Value, patch: &Value) {
    if let (Some(d), Some(p)) = (dst.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if v == SENTINEL {
                continue;
            }
            if v.is_object() {
                merge(d.entry(k).or_insert(json!({})), v)
            } else {
                d.insert(k.clone(), v.clone());
            }
        }
    } else {
        *dst = patch.clone();
    }
}
fn sensitive(k: &str) -> bool {
    matches!(
        k,
        "apiKey"
            | "accessToken"
            | "refreshToken"
            | "token"
            | "password"
            | "managementKey"
            | "clientSecret"
            | "appPassword"
            | "personalAccessToken"
    )
}
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if m.get("sensitive") == Some(&Value::Bool(true)) {
                m.insert("value".into(), json!(""));
                m.insert("valueRedacted".into(), json!(true));
            }
            for (k, v) in m {
                if sensitive(k) {
                    redact_secret(v);
                } else {
                    redact(v)
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                redact(v)
            }
        }
        _ => {}
    }
}
pub fn get(s: &State) -> Result<Value, String> {
    let mut v = read(s)?;
    redact(&mut v);
    Ok(v)
}
pub fn update(s: &State, p: &Value) -> Result<Value, String> {
    let _guard = s.settings_lock.lock().unwrap();
    let patch = p
        .get("patch")
        .filter(|v| v.is_object())
        .ok_or("patch must be an object")?;
    let mut v = read(s)?;
    let previous = v.clone();
    // Preserve sensitive environment values echoed by clients as valueRedacted.
    let mut patch = patch.clone();
    preserve_environment(&v, &mut patch);
    merge(&mut v, &patch);
    apply_entry_patches(&previous, &mut v, &patch)?;
    if let Some(instances) = patch.get("providerInstances") {
        let mut replacement = instances.clone();
        retain_sentinels(&previous["providerInstances"], &mut replacement);
        v["providerInstances"] = replacement;
    }
    if let Some(m) = p.get("providerInstanceMutation") {
        let id = m["instanceId"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id.as_bytes()[0].is_ascii_alphabetic()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .ok_or("invalid provider instance id")?;
        if !v["providerInstances"].is_object() {
            v["providerInstances"] = json!({});
        }
        let exists = v["providerInstances"].get(id).is_some();
        match m["operation"].as_str() {
            Some("remove") => {
                v["providerInstances"].as_object_mut().unwrap().remove(id);
            }
            Some("create" | "upsert") => {
                if m["operation"] == "create" && exists {
                    return Err("provider instance already exists".into());
                }
                if !m["instance"].is_object() || !m["instance"]["driver"].is_string() {
                    return Err("provider instance driver required".into());
                }
                let mut instance = m["instance"].clone();
                let mut wrapper = json!({"providerInstances":{id:instance}});
                preserve_environment(&previous, &mut wrapper);
                instance = wrapper["providerInstances"][id].take();
                retain_sentinels(&previous["providerInstances"][id], &mut instance);
                v["providerInstances"][id] = instance;
            }
            _ => return Err("invalid provider instance operation".into()),
        }
    }
    let defaults: Value = serde_json::from_str(include_str!("settings-defaults.json")).unwrap();
    for (key, value) in patch.as_object().unwrap() {
        if let Some(default) = defaults.get(key).filter(|v| !v.is_null()) {
            if std::mem::discriminant(default) != std::mem::discriminant(value) {
                return Err(format!("invalid settings type: {key}"));
            }
        }
    }
    normalize_preferences(&previous, &mut v, &patch)?;
    crate::profile::atomic_json(&s.dir.join("settings.json"), &v).map_err(|e| e.to_string())?;
    s.notify();
    redact(&mut v);
    Ok(v)
}
fn preserve_environment(old: &Value, patch: &mut Value) {
    if let Some(instances) = patch
        .get_mut("providerInstances")
        .and_then(Value::as_object_mut)
    {
        for (id, instance) in instances {
            if let Some(vars) = instance["environment"].as_array_mut() {
                for var in vars {
                    if var["valueRedacted"] == true {
                        if let Some(prev) = old["providerInstances"][id]["environment"]
                            .as_array()
                            .and_then(|a| a.iter().find(|p| p["name"] == var["name"]))
                        {
                            var["value"] = prev["value"].clone();
                        } else {
                            var["value"] = json!("");
                        }
                        var.as_object_mut().unwrap().remove("valueRedacted");
                    }
                }
            }
        }
    }
}
pub fn config(s: &State) -> Result<Value, String> {
    let (keybindings, issues) = crate::keybind::resolved(s);
    let mut config = json!({"environment":descriptor(s),"directEndpoints":direct_endpoints(s),"auth":auth_descriptor(),"cwd":std::env::current_dir().unwrap_or_default(),"keybindingsConfigPath":s.dir.join("keybindings.json"),"keybindings":keybindings,"issues":issues,"providers":s.provider.lock().unwrap().as_ref().map(|p|p.config()).unwrap_or_default(),"availableEditors":[],"observability":{"logsDirectoryPath":s.dir.join("logs"),"localTracingEnabled":false,"otlpTracesEnabled":false,"otlpMetricsEnabled":false,"otlpLogsEnabled":false},"settings":get(s)?,"shellResumeCompletionMarker":true,"threadResumeCompletionMarker":true,"shellThreadFamilies":false,"shellOlderSettled":false,"threadSnapshotPagination":false});
    startup_catalog(s, &mut config);
    Ok(config)
}

fn redact_secret(v: &mut Value) {
    match v {
        Value::String(s) if !s.is_empty() => *s = SENTINEL.into(),
        Value::Object(m) => {
            for v in m.values_mut() {
                redact_secret(v)
            }
        }
        _ => {}
    }
}

fn retain_sentinels(old: &Value, new: &mut Value) {
    if new == SENTINEL {
        *new = old.clone();
        return;
    }
    if let Some(map) = new.as_object_mut() {
        for (k, v) in map {
            retain_sentinels(&old[k], v);
        }
    }
}

fn apply_entry_patches(old: &Value, next: &mut Value, patch: &Value) -> Result<(), String> {
    for field in [
        "usageLimitSources",
        "usagePriceOverrides",
        "usageModelAliases",
        "projectSettingsOverrides",
    ] {
        if let Some(entries) = patch.get(field) {
            let entries = entries
                .as_object()
                .ok_or("settings entry patch must be an object")?;
            let mut result = old[field].as_object().cloned().unwrap_or_default();
            for (id, entry) in entries {
                if entry.is_null() {
                    result.remove(id);
                } else {
                    let mut entry = entry.clone();
                    if field == "projectSettingsOverrides" {
                        if !entry.is_object() {
                            return Err("project settings override must be an object".into());
                        }
                        if entry.get("providerInstances").is_none() {
                            if let Some(instances) = old[field][id].get("providerInstances") {
                                entry["providerInstances"] = instances.clone();
                            }
                        }
                    }
                    result.insert(id.clone(), entry);
                }
            }
            next[field] = Value::Object(result);
        }
    }
    for field in [
        "defaultModelSelection",
        "sourceControlWriterModelSelection",
        "reviewGuideModelSelection",
        "defaultProjectScripts",
    ] {
        if let Some(value) = patch.get(field) {
            next[field] = value.clone();
        }
    }
    if let Some(path) = patch["worktreesDirectory"]
        .as_str()
        .filter(|p| Some(*p) != old["worktreesDirectory"].as_str())
    {
        let mut prior = old["previousWorktreesDirectories"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        prior.retain(|p| p != path);
        if let Some(old_path) = old["worktreesDirectory"].as_str().filter(|p| !p.is_empty()) {
            if !prior.iter().any(|p| p == old_path) {
                prior.push(json!(old_path));
            }
        }
        next["previousWorktreesDirectories"] = json!(prior);
    }
    Ok(())
}

fn direct_endpoints(s: &State) -> Vec<Value> {
    #[cfg(feature = "bridge")]
    {
        s.bridge_endpoints.lock().unwrap().clone()
    }
    #[cfg(not(feature = "bridge"))]
    {
        let _ = s;
        Vec::new()
    }
}

fn normalize_preferences(old: &Value, next: &mut Value, patch: &Value) -> Result<(), String> {
    let migration = &patch["migrateModelPreferences"];
    for key in ["favorites", "providerModelPreferences"] {
        next[key] = patch
            .get(key)
            .or_else(|| old.get(key).filter(|v| !v.is_null()))
            .or_else(|| migration.get(key))
            .cloned()
            .unwrap_or(Value::Null);
    }
    for (key, flag) in [
        ("setModelFavorites", "favorite"),
        ("setModelsHidden", "hidden"),
    ] {
        if let Some(changes) = patch.get(key) {
            for change in changes.as_array().ok_or("model changes must be an array")? {
                let provider = change["provider"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or("provider required")?;
                let model = change["model"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or("model required")?;
                let enabled = change[flag].as_bool().ok_or("model flag must be boolean")?;
                if key == "setModelFavorites" {
                    let mut favorites = next["favorites"].as_array().cloned().unwrap_or_default();
                    let exists = favorites
                        .iter()
                        .any(|v| v["provider"] == provider && v["model"] == model);
                    if enabled && !exists {
                        favorites.push(json!({"provider":provider,"model":model}));
                    }
                    if !enabled {
                        favorites.retain(|v| v["provider"] != provider || v["model"] != model);
                    }
                    next["favorites"] = json!(favorites);
                } else {
                    if !next["providerModelPreferences"].is_object() {
                        next["providerModelPreferences"] = json!({});
                    }
                    if !next["providerModelPreferences"][provider].is_object() {
                        next["providerModelPreferences"][provider] =
                            json!({"hiddenModels":[],"modelOrder":[]});
                    }
                    let mut hidden = next["providerModelPreferences"][provider]["hiddenModels"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    hidden.retain(|v| v != model);
                    if enabled {
                        hidden.push(json!(model));
                    }
                    next["providerModelPreferences"][provider]["hiddenModels"] = json!(hidden);
                }
            }
        }
    }
    if let Some(order) = patch.get("setProviderModelOrder") {
        let provider = order["provider"].as_str().ok_or("provider required")?;
        let models = order["modelOrder"]
            .as_array()
            .filter(|a| a.iter().all(Value::is_string))
            .ok_or("modelOrder must contain strings")?;
        if !next["providerModelPreferences"].is_object() {
            next["providerModelPreferences"] = json!({});
        }
        if !next["providerModelPreferences"][provider].is_object() {
            next["providerModelPreferences"][provider] = json!({"hiddenModels":[]});
        }
        next["providerModelPreferences"][provider]["modelOrder"] = json!(models);
    }
    for key in [
        "setModelFavorites",
        "setModelsHidden",
        "setProviderModelOrder",
        "migrateModelPreferences",
    ] {
        next.as_object_mut().unwrap().remove(key);
    }
    if let Some(instances) = next["providerInstances"].as_object_mut() {
        for instance in instances.values_mut() {
            if let Some(enabled) = instance["config"]["enabled"].as_bool() {
                instance["enabled"] = json!(instance["enabled"] != false && enabled);
                instance["config"]
                    .as_object_mut()
                    .unwrap()
                    .remove("enabled");
            }
        }
    }
    Ok(())
}

fn startup_catalog(s: &State, config: &mut Value) {
    let projects = crate::projects::snapshot(s).unwrap_or(Value::Null);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for provider in config["providers"].as_array_mut().into_iter().flatten() {
        let driver = provider["driver"].as_str().unwrap_or("");
        if !matches!(driver, "codex" | "claudeAgent") {
            continue;
        }
        let mut roots = Vec::new();
        if let Some(home) = &home {
            roots.push((home.clone(), "user"));
        }
        for p in projects["projects"].as_array().into_iter().flatten() {
            if let Some(root) = p["workspaceRoot"].as_str() {
                roots.push((root.into(), "project"));
            }
        }
        let mut skills = provider["skills"].as_array().cloned().unwrap_or_default();
        let mut commands = provider["slashCommands"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (root, scope) in roots {
            let native = if driver == "codex" {
                ".codex"
            } else {
                ".claude"
            };
            for directory in [
                root.join(native).join("skills"),
                root.join(".agents/skills"),
            ] {
                let Ok(entries) = std::fs::read_dir(directory) else {
                    continue;
                };
                for entry in entries.take(512).flatten() {
                    let path = entry.path().join("SKILL.md");
                    let Some((name, description)) =
                        catalog_header(&path, entry.file_name().to_string_lossy().as_ref())
                    else {
                        continue;
                    };
                    if skills.iter().any(|v| v["name"] == name) {
                        continue;
                    }
                    skills.push(json!({"name":name,"description":description,"path":path,"scope":scope,"enabled":true}));
                }
            }
            if driver == "claudeAgent" {
                if let Ok(entries) = std::fs::read_dir(root.join(native).join("commands")) {
                    for entry in entries.take(512).flatten() {
                        let path = entry.path();
                        if path.extension().is_none_or(|e| e != "md") {
                            continue;
                        }
                        let name = path.file_stem().unwrap_or_default().to_string_lossy();
                        if let Some((name, description)) = catalog_header(&path, &name) {
                            if !commands.iter().any(|v| v["name"] == name) {
                                commands.push(json!({"name":name,"description":description}));
                            }
                        }
                    }
                }
            }
        }
        skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        commands.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        provider["skills"] = json!(skills);
        provider["slashCommands"] = json!(commands);
    }
}
fn catalog_header(path: &std::path::Path, fallback: &str) -> Option<(String, String)> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(8192).read_to_string(&mut text).ok()?;
    let mut name = fallback.to_owned();
    let mut description = String::new();
    if let Some(header) = text
        .strip_prefix("---\n")
        .and_then(|t| t.split_once("\n---").map(|p| p.0))
    {
        for line in header.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches(['\'', '"']);
                match key.trim() {
                    "name" => name = value.to_owned(),
                    "description" => description = value.to_owned(),
                    _ => {}
                }
            }
        }
    }
    (!name.trim().is_empty()).then_some((name, description))
}
