//! Settings remain on disk; unknown keys survive updates.
use crate::store::State;
use serde_json::{json, Value};
pub const SENTINEL: &str = "••••••";
pub fn descriptor(s: &State) -> Value {
    json!({"environmentId":s.environment_id,"label":s.label,"platform":{"os":std::env::consts::OS,"arch":match std::env::consts::ARCH {"x86_64"=>"x64","aarch64"=>"arm64",_=>"other"}},"serverVersion":crate::rpc::SERVER_VERSION,"orchestrationProtocolVersion":2,"capabilities":{"serverBrowser":crate::server_browser::executable().is_some(),"agentBrowserAccess":crate::agent_browser::enabled(s, ""),"devices":crate::device::available(),"repositoryIdentity":true,"threadSettlement":true,"threadSnooze":true,"threadPinning":true,"threadAutoSettleOptOut":true,"threadTitleRegeneration":true,"attachmentUploads":true,"fileAttachments":{"maxUploadBytes":52428800},"inlineMessageContext":true,"pullRequests":true,"pullRequestChecks":true,"threadAutoSettlement":true,"storageCleanup":true,"storageCleanupRun":true,"projectWorktreeCleanup":true,"worktreesDirectory":true,"projectSettingsOverrides":true,"environmentThemes":true,"usageLimitSources":true,"threadPinReorder":true,"threadActiveReorder":true,"threadVisitedTracking":true,"threadPullRequestLinking":true,"threadPullRequests":true,"threadPullRequestWatch":true,"pullRequestStackActions":true,"otlpEndpointCheck":true,"commandOutputStreaming":true}})
}
pub fn auth_descriptor() -> Value {
    json!({"policy":"loopback-browser","bootstrapMethods":["pairing-token"],"sessionMethods":["bearer-access-token"],"sessionCookieName":"t3code_session"})
}
pub(crate) type Cache = Option<(Option<(std::time::SystemTime, u64)>, Value)>;
pub fn read(s: &State) -> Result<Value, String> {
    let stamp = match std::fs::metadata(s.dir.join("settings.json")) {
        Ok(m) => Some((m.modified().map_err(|e| e.to_string())?, m.len())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let mut cache = s.settings_cache.lock().unwrap();
    if let Some((prior, value)) = &*cache {
        if *prior == stamp {
            return Ok(value.clone());
        }
    }
    let mut v: Value = serde_json::from_str(include_str!("settings-defaults.json")).unwrap();
    match std::fs::read(s.dir.join("settings.json")) {
        Ok(b) => {
            let stored: Value = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
            merge(&mut v, &stored);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    normalize(&mut v, &json!({}))?;
    *cache = Some((stamp, v.clone()));
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
            if !(key == "sidebarAutoSettleAfterDays" && value.is_null())
                && std::mem::discriminant(default) != std::mem::discriminant(value)
            {
                return Err(format!("invalid settings type: {key}"));
            }
        }
    }
    if let Some(overrides) = patch["backgroundActivity"].get("overrides") {
        v["backgroundActivity"]["overrides"] = overrides.clone();
    }
    if let Some(selection) = patch.get("textGenerationModelSelection") {
        if selection.is_object()
            && selection.get("instanceId").is_none()
            && v["textGenerationModelSelection"]
                .get("instanceId")
                .is_none()
        {
            v["textGenerationModelSelection"]["instanceId"] = json!("codex");
        }
        if selection.get("options").is_none()
            && (selection.get("model").is_some() || selection.get("instanceId").is_some())
        {
            v["textGenerationModelSelection"]["options"] = json!([]);
        }
    }
    normalize_preferences(&previous, &mut v, &patch)?;
    normalize(&mut v, &patch)?;
    crate::profile::atomic_json(&s.dir.join("settings.json"), &v).map_err(|e| e.to_string())?;
    s.notify();
    if patch.get("providerInstances").is_some() || p.get("providerInstanceMutation").is_some() {
        if let (Ok(runtime), Some(provider)) = (
            tokio::runtime::Handle::try_current(),
            s.provider.lock().unwrap().clone(),
        ) {
            let changed = s.changed.clone();
            let cwd = std::env::current_dir().unwrap_or_else(|_| s.dir.clone());
            runtime.spawn(async move {
                let _ = provider.discover(cwd).await;
                changed.send_modify(|v| *v = v.wrapping_add(1));
            });
        }
    }
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
    let mut config = json!({"environment":descriptor(s),"directEndpoints":direct_endpoints(s),"auth":auth_descriptor(),"cwd":std::env::current_dir().unwrap_or_default(),"keybindingsConfigPath":s.dir.join("keybindings.json"),"keybindings":keybindings,"issues":issues,"providers":s.provider.lock().unwrap().as_ref().map(|p|p.config()).unwrap_or_default(),"availableEditors":crate::files::available_editors(),"observability":{"logsDirectoryPath":s.dir.join("logs"),"localTracingEnabled":false,"otlpTracesEnabled":false,"otlpMetricsEnabled":false,"otlpLogsEnabled":false},"settings":get(s)?,"shellResumeCompletionMarker":true,"threadResumeCompletionMarker":true,"shellThreadFamilies":false,"shellOlderSettled":false,"threadSnapshotPagination":false});
    config["settingsSupport"] = json!({"enableAgentBrowserAccess":{"supported":true},"projectAgentBrowserAccessOverrides":{"supported":true}});
    startup_catalog(s, &mut config);
    crate::setup::decorate(s, &mut config);
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
            if field.ends_with("ModelSelection") && value.is_object() && old[field].is_object() {
                next[field] = old[field].clone();
                merge(&mut next[field], value);
            } else {
                next[field] = value.clone();
            }
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
    let settings = read(s).unwrap_or(json!({}));
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for provider in config["providers"].as_array_mut().into_iter().flatten() {
        let driver = provider["driver"].as_str().unwrap_or("");
        if !matches!(driver, "codex" | "claudeAgent") {
            continue;
        }
        let id = provider["instanceId"].as_str().unwrap_or("");
        let instance = &settings["providerInstances"][id];
        let native_home = crate::provider::instance_environment(instance)
            .iter()
            .find(|(k, _)| {
                k == if driver == "codex" {
                    "CODEX_HOME"
                } else {
                    "CLAUDE_CONFIG_DIR"
                }
            })
            .map(|(_, v)| std::path::PathBuf::from(v))
            .or_else(|| {
                std::env::var_os(if driver == "codex" {
                    "CODEX_HOME"
                } else {
                    "CLAUDE_CONFIG_DIR"
                })
                .map(std::path::PathBuf::from)
            });
        let mut roots = Vec::new();
        if let Some(native_home) = native_home {
            roots.push((native_home, "native"));
        }
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
                if scope == "native" {
                    root.join("skills")
                } else {
                    root.join(native).join("skills")
                },
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
                    skills.push(json!({"name":name,"description":description,"path":path,"scope":if scope == "native" {"user"} else {scope},"enabled":true}));
                }
            }
            if driver == "claudeAgent" {
                if let Ok(entries) = std::fs::read_dir(if scope == "native" {
                    root.join("commands")
                } else {
                    root.join(native).join("commands")
                }) {
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

// Normalize the representation consumed by native_settings/scoped_settings and the
// provider forms. This also runs on reads, including imported legacy settings.
fn normalize(v: &mut Value, patch: &Value) -> Result<(), String> {
    for value in v.as_object_mut().ok_or("invalid settings")?.values_mut() {
        if let Some(text) = value.as_str() {
            *value = json!(text.trim());
        }
    }
    if let Some(days) = v["sidebarAutoSettleAfterDays"].as_u64() {
        if !(1..=90).contains(&days) {
            return Err("invalid sidebarAutoSettleAfterDays".into());
        }
    } else if !v["sidebarAutoSettleAfterDays"].is_null() {
        return Err("invalid sidebarAutoSettleAfterDays".into());
    }
    for field in [
        "defaultModelSelection",
        "textGenerationModelSelection",
        "sourceControlWriterModelSelection",
        "reviewGuideModelSelection",
    ] {
        if let Some(selection) = v.get_mut(field).filter(|v| !v.is_null()) {
            for key in ["instanceId", "model"] {
                if !selection[key]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    return Err(format!("invalid {field}.{key}"));
                }
            }
            if selection.get("options").is_none() {
                selection["options"] = json!([]);
            }
        }
    }
    for (key, val) in v["storageCleanup"].as_object().into_iter().flatten() {
        if key.ends_with("Days") {
            let min = if key == "worktreeSettledAfterDays" {
                0
            } else {
                1
            };
            if !val.is_null() && !val.as_u64().is_some_and(|n| (min..=3650).contains(&n)) {
                return Err(format!("invalid {key}"));
            }
        } else if key != "worktreeKeepWhen" && !val.is_boolean() {
            return Err(format!("invalid {key}"));
        }
    }
    if let Some(text) = v["sourceControlWritingStyle"].as_str() {
        v["sourceControlWritingStyle"] = json!({"mode":if text.is_empty(){"repo_conventions"}else{"custom"},"customInstructions":text,"followChangeRequestTemplates":true});
    }

    for (key, allowed) in [
        ("responseStreamingMode", &["turn", "paragraph"][..]),
        (
            "defaultRuntimeMode",
            &["full-access", "approval-required", "auto-accept-edits"],
        ),
        ("branchNamingMode", &["static", "semantic", "custom"]),
        ("idleCompaction", &["onReturn", "beforeCacheExpires", "off"]),
    ] {
        if let Some(value) = v.get(key).filter(|v| !v.is_null()) {
            if !value.as_str().is_some_and(|s| allowed.contains(&s)) {
                return Err(format!("invalid {key}"));
            }
        }
    }
    let mode = patch
        .get("idleCompaction")
        .and_then(Value::as_str)
        .or_else(|| {
            patch
                .get("compactIdleThreads")
                .and_then(Value::as_bool)
                .map(|v| if v { "beforeCacheExpires" } else { "onReturn" })
        })
        .or_else(|| v["idleCompaction"].as_str())
        .unwrap_or(if v["compactIdleThreads"] == true {
            "beforeCacheExpires"
        } else {
            "onReturn"
        })
        .to_owned();
    v["compactIdleThreads"] = json!(mode == "beforeCacheExpires");
    v["idleCompaction"] = json!(mode);
    let background = &v["backgroundActivity"];
    if background["schemaVersion"] != 1
        || !matches!(
            background["profile"].as_str(),
            Some("performance" | "balanced" | "battery-saver" | "custom")
        )
    {
        return Err("invalid backgroundActivity".into());
    }
    for (key, val) in background["overrides"].as_object().into_iter().flatten() {
        if key.ends_with("Interval") || key == "idleClientTtl" {
            if !val.is_u64() {
                return Err(format!("invalid background override {key}"));
            }
        } else if !val.is_boolean() {
            return Err(format!("invalid background override {key}"));
        }
    }
    normalize_background(v, patch)?;
    for (legacy, key) in [
        (
            "projectAgentBrowserAccessOverrides",
            "enableAgentBrowserAccess",
        ),
        ("projectAutoPullOverrides", "defaultAutoPull"),
        ("projectScriptOverrides", "defaultProjectScripts"),
    ] {
        for (id, value) in patch[legacy].as_object().into_iter().flatten() {
            if !v["projectSettingsOverrides"][id].is_object() {
                v["projectSettingsOverrides"][id] = json!({});
            }
            v["projectSettingsOverrides"][id][key] = value.clone();
        }
        let mut derived = json!({});
        for (id, entry) in v["projectSettingsOverrides"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(value) = entry.get(key) {
                derived[id] = value.clone();
            }
        }
        v[legacy] = derived;
    }
    if let Some(instances) = v["providerInstances"].as_object_mut() {
        for instance in instances.values_mut() {
            if !instance.is_object() || !instance["driver"].is_string() {
                return Err("invalid provider instance".into());
            }
            if !instance["config"].is_object() {
                instance["config"] = json!({});
            }
            if instance.get("enabled").is_none() {
                instance["enabled"] = json!(true);
            }
            if let Some(enabled) = instance["config"].get("enabled") {
                instance["enabled"] = json!(instance["enabled"] != false && enabled != false);
                instance["config"]
                    .as_object_mut()
                    .unwrap()
                    .remove("enabled");
            }
            if instance["environment"].is_null() {
                instance["environment"] = json!([]);
            }
            for var in instance["environment"]
                .as_array()
                .ok_or("invalid environment")?
            {
                let name = var["name"].as_str().ok_or("invalid environment name")?;
                if name.is_empty() || name.contains(['=', '\0']) || !var["value"].is_string() {
                    return Err("invalid environment variable".into());
                }
            }
            if let Some(args) = instance["config"]["launchArgs"].as_str() {
                crate::provider::launch_args(args)?;
            }
            for key in ["binaryPath", "homePath", "shadowHomePath"] {
                if let Some(value) = instance["config"].get_mut(key) {
                    *value = json!(value.as_str().ok_or("invalid provider path")?.trim());
                }
            }
        }
    }
    Ok(())
}

fn background_preset(profile: &str) -> Value {
    let performance = profile == "performance";
    let battery = profile == "battery-saver";
    json!({"automaticGitFetchInterval":if performance {15000}else if battery {0}else{30000},"providerHealthRefreshInterval":if performance {60000}else if battery {900000}else{300000},"hostPowerMonitorActiveInterval":if battery {60000}else{30000},"hostPowerMonitorIdleInterval":if performance {120000}else if battery {600000}else{300000},"idleClientTtl":45000,"pauseWhenHostLocked":true,"pauseWhenHostLowPower":!performance,"pauseWhenClientLowPower":!performance,"pauseWhenOnBattery":battery})
}
pub(crate) fn background_resolved(v: &Value) -> Value {
    let activity = &v["backgroundActivity"];
    let mut resolved = background_preset(if activity["profile"] == "custom" {
        activity["baseProfile"].as_str().unwrap_or("balanced")
    } else {
        activity["profile"].as_str().unwrap_or("balanced")
    });
    if activity["profile"] == "custom" {
        merge(&mut resolved, &activity["overrides"]);
    }
    resolved
}
fn normalize_background(v: &mut Value, patch: &Value) -> Result<(), String> {
    let mut activity = v["backgroundActivity"].clone();
    if patch.get("backgroundActivity").is_none()
        && [
            "backgroundActivityProfile",
            "automaticGitFetchInterval",
            "providerHealthRefreshInterval",
        ]
        .iter()
        .any(|k| patch.get(*k).is_some())
    {
        let base = patch["backgroundActivityProfile"]
            .as_str()
            .or_else(|| activity["baseProfile"].as_str())
            .or_else(|| activity["profile"].as_str())
            .unwrap_or("balanced");
        let overrides: serde_json::Map<String, Value> =
            ["automaticGitFetchInterval", "providerHealthRefreshInterval"]
                .into_iter()
                .filter_map(|k| patch.get(k).map(|v| (k.to_owned(), v.clone())))
                .collect();
        activity = if overrides.is_empty() {
            json!({"schemaVersion":1,"profile":base,"overrides":{}})
        } else {
            json!({"schemaVersion":1,"profile":"custom","baseProfile":base,"overrides":overrides})
        };
    }
    let profile = activity["profile"]
        .as_str()
        .unwrap_or("balanced")
        .to_owned();
    let base = if profile == "custom" {
        activity["baseProfile"].as_str().unwrap_or("balanced")
    } else {
        &profile
    }
    .to_owned();
    if !matches!(base.as_str(), "balanced" | "performance" | "battery-saver") {
        return Err("invalid background baseProfile".into());
    }
    let mut resolved = background_preset(&base);
    if profile == "custom" {
        merge(&mut resolved, &activity["overrides"]);
    }
    let matching = [base.as_str(), "balanced", "performance", "battery-saver"]
        .into_iter()
        .find(|p| background_preset(p) == resolved);
    v["backgroundActivity"] = if let Some(profile) = matching {
        json!({"schemaVersion":1,"profile":profile,"overrides":{}})
    } else {
        let preset = background_preset(&base);
        let overrides: serde_json::Map<String, Value> = resolved
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, val)| preset[*k] != **val)
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect();
        json!({"schemaVersion":1,"profile":"custom","baseProfile":base,"overrides":overrides})
    };
    v["backgroundActivityProfile"] = json!(matching.unwrap_or(&base));
    v["automaticGitFetchInterval"] = resolved["automaticGitFetchInterval"].clone();
    v["providerHealthRefreshInterval"] = resolved["providerHealthRefreshInterval"].clone();
    Ok(())
}
