//! Settings remain on disk; unknown keys survive updates.
use crate::store::State;
use serde_json::{json, Value};
pub const SENTINEL: &str = "••••••";
pub fn descriptor(s: &State) -> Value {
    json!({"environmentId":s.environment_id,"label":s.label,"platform":{"os":std::env::consts::OS,"arch":match std::env::consts::ARCH {"x86_64"=>"x64","aarch64"=>"arm64",_=>"other"}},"serverVersion":crate::rpc::SERVER_VERSION,"orchestrationProtocolVersion":2,"capabilities":{"serverBrowser":s.streams.p2.preview.desktop_available() || crate::server_browser::executable().is_some(),"agentBrowserAccess":crate::agent_browser::enabled(s, ""),"devices":crate::device::available_for(s),"repositoryIdentity":true,"threadSettlement":true,"threadSnooze":true,"threadPinning":true,"threadAutoSettleOptOut":true,"threadTitleRegeneration":true,"attachmentUploads":true,"fileAttachments":{"maxUploadBytes":52428800},"inlineMessageContext":true,"pullRequests":crate::executable::resolve(std::path::Path::new("gh")).is_some(),"pullRequestChecks":true,"threadAutoSettlement":true,"storageCleanup":true,"storageCleanupRun":true,"projectWorktreeCleanup":true,"worktreesDirectory":true,"projectSettingsOverrides":true,"environmentThemes":true,"usageLimitSources":true,"threadPinReorder":true,"threadActiveReorder":true,"threadVisitedTracking":true,"threadPullRequestLinking":true,"threadPullRequests":true,"threadPullRequestWatch":true,"pullRequestStackActions":true,"otlpEndpointCheck":true,"commandOutputStreaming":true,"freshContext":true,"scheduledTaskThreadModes":true}})
}
pub fn auth_descriptor() -> Value {
    json!({"policy":"loopback-browser","bootstrapMethods":["pairing-token"],"sessionMethods":["bearer-access-token"],"sessionCookieName":"t3code_session"})
}
pub fn auth_descriptor_for(state: &State) -> Value {
    let mut descriptor = auth_descriptor();
    descriptor["policy"] = serde_json::to_value(state.auth_policy()).unwrap();
    descriptor["bootstrapMethods"] = if state
        .listener_auth
        .get()
        .is_some_and(|listener| listener.desktop)
    {
        if matches!(
            state.auth_policy(),
            crate::managed_limits::AuthPolicy::Remote
        ) {
            json!(["desktop-bootstrap", "one-time-token"])
        } else {
            json!(["desktop-bootstrap"])
        }
    } else {
        json!(["one-time-token"])
    };
    descriptor
}
pub(crate) type Cache = Option<(Option<(std::time::SystemTime, u64)>, Value)>;
pub fn read(s: &State) -> Result<Value, String> {
    s.managed_policy.effective_settings(&read_stored(s)?)
}
pub(crate) fn read_stored(s: &State) -> Result<Value, String> {
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
    if s.managed_policy.settings_controls()?.is_some() {
        v = with_native_fields(&v, crate::managed_settings_codec::encoded_defaults());
    }
    match std::fs::read(s.dir.join("settings.json")) {
        Ok(b) => {
            let stored: Value = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
            let filled = match s.managed_policy.settings_controls()? {
                Some(controls) => controls.with_defaults(&stored),
                None => stored.clone(),
            };
            merge(&mut v, &filled);
            // These complete source records contain public saved-secret markers.
            // Generic legacy merge skips markers, which would erase their slots.
            for key in ["sourceControlHosts", "usageLimitSources", "secretProviders"] {
                if let Some(value) = filled.get(key) {
                    v[key] = value.clone();
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(controls) = s.managed_policy.settings_controls()? {
                merge(&mut v, &controls.with_defaults(&json!({})));
            }
        }
        Err(e) => return Err(e.to_string()),
    }
    if s.managed_policy.settings_controls()?.is_some() {
        let native = crate::managed_settings_codec::decode_persisted(v.clone())
            .and_then(crate::managed_settings_codec::encode)
            .map_err(|_| "Invalid managed settings input")?;
        v = with_native_fields(&v, native);
    }
    normalize(&mut v, &json!({}))?;
    *cache = Some((stamp, v.clone()));
    Ok(v)
}
/// Keep O3 extension keys outside the pinned native schema projection.
pub(crate) fn with_native_fields(original: &Value, mut native: Value) -> Value {
    for (key, value) in original.as_object().into_iter().flatten() {
        if !crate::managed_settings_codec::fields().contains_key(key) {
            native[key] = value.clone();
        }
    }
    native
}
fn strip_defaults(
    current: &Value,
    defaults: &Value,
    is_atomic: &impl Fn(&str) -> bool,
) -> Option<Value> {
    if let (Some(current), Some(defaults)) = (current.as_object(), defaults.as_object()) {
        let mut stripped = serde_json::Map::new();
        for (key, value) in current {
            if is_atomic(key) {
                if Some(value) != defaults.get(key) {
                    stripped.insert(key.clone(), value.clone());
                }
            } else if let Some(value) =
                strip_defaults(value, defaults.get(key).unwrap_or(&Value::Null), is_atomic)
            {
                stripped.insert(key.clone(), value);
            }
        }
        (!stripped.is_empty()).then_some(Value::Object(stripped))
    } else {
        (current != defaults).then(|| current.clone())
    }
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
                if matches!(k.as_str(), "tokens" | "signIns") {
                    if let Some(entries) = v.as_object_mut() {
                        for value in entries.values_mut() {
                            if value.is_string() {
                                redact_secret(value);
                            } else {
                                redact(value);
                            }
                        }
                    }
                } else if sensitive(k) {
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
/// Capture settings and private slots under the same lock as write/rollback.
pub(crate) fn read_materialized(s: &State) -> Result<Value, String> {
    let _guard = s.settings_lock.lock().unwrap();
    crate::settings_secrets::materialize_profile(&s.dir, &read(s)?)
}
fn private_projection(value: &Value) -> Result<Value, String> {
    let providers = value["providerInstances"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, entry)| matches!(entry["driver"].as_str(), Some("codex" | "claudeAgent")))
        .map(|(id, entry)| (id.clone(), entry.clone()))
        .collect::<serde_json::Map<_, _>>();
    let mut decoded=crate::managed_settings_codec::decode_patch(json!({"providerInstances":providers,"sourceControlHosts":value["sourceControlHosts"],"usageLimitSources":value["usageLimitSources"]}))
        .map_err(|_|"Invalid private settings fields".to_owned())?;
    decoded["easDeviceHosts"] = json!([]);
    decoded["simlockDeviceHosts"] = json!([]);
    decoded["secretProviders"] = crate::managed_settings_codec::decode_field(
        "secretProviders",
        value.get("secretProviders").cloned(),
    )
    .map_err(|_| "Invalid private settings fields")?;
    Ok(decoded)
}
fn clear_removed_provider_references(current: &Value, next: &Value) -> Result<Value, String> {
    let removed = current["secretProviders"]
        .as_object()
        .into_iter()
        .flatten()
        .any(|(id, _)| next["secretProviders"].get(id).is_none());
    if !removed {
        return Ok(next.clone());
    }
    let transform = |settings: &Value, encode: bool| -> Result<Value, String> {
        let mut result = settings.clone();
        for (id, entry) in settings["providerInstances"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if let Some(environment) = entry.get("environment") {
                result["providerInstances"][id]["environment"] =
                    crate::managed_settings_codec::provider_environment(
                        environment.clone(),
                        encode,
                    )
                    .map_err(|_| "Invalid provider environment")?;
            }
        }
        Ok(result)
    };
    // Native cleanup consumes decoded reference values; shared-profile files
    // encode those same provider variables with a separate `secret` member.
    let decoded = transform(next, false)?;
    transform(
        &crate::settings_secrets::without_removed_providers(current, &decoded),
        true,
    )
}
fn apply_private_projection(value: &mut Value, projected: &Value) -> Result<(), String> {
    let encoded = crate::managed_settings_codec::encode_sparse(projected.clone())
        .map_err(|_| "Invalid private settings encoding".to_owned())?;
    for key in ["sourceControlHosts", "usageLimitSources", "secretProviders"] {
        value[key] = encoded[key].clone();
    }
    for (id, entry) in encoded["providerInstances"]
        .as_object()
        .into_iter()
        .flatten()
    {
        value["providerInstances"][id] = entry.clone();
    }
    Ok(())
}
fn redact_client(value: &mut Value) -> Result<(), String> {
    // Full client reference capability negotiation remains gated.
    let projected = crate::settings_secret_transaction::redact(&private_projection(value)?, false);
    redact(value);
    let encoded = crate::managed_settings_codec::encode_sparse(projected)
        .map_err(|_| "Invalid private settings encoding".to_owned())?;
    for key in ["sourceControlHosts", "usageLimitSources", "secretProviders"] {
        value[key] = encoded[key].clone();
    }
    for (id, entry) in encoded["providerInstances"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if let Some(environment) = entry.get("environment") {
            value["providerInstances"][id]["environment"] = environment.clone();
        }
    }
    Ok(())
}
fn private_error(s: &State, error: &crate::settings_secret_transaction::TransactionError) -> Value {
    for failure in &error.rollback_errors {
        eprintln!(
            "Failed to roll back settings secret: {} ({})",
            failure.resource, failure.tag
        );
    }
    let message = format!(
        "Server settings {} failed at {}.",
        error.operation,
        s.dir.join("settings.json").display()
    );
    json!({"operation":error.operation,"message":message,"cause":{"_tag":error.cause.tag,"resource":error.cause.resource}})
}
pub fn get(s: &State) -> Result<Value, String> {
    let mut v = read(s)?;
    redact_client(&mut v)?;
    Ok(v)
}
pub fn update(s: &State, p: &Value) -> Result<Value, String> {
    update_inner(s, p, &mut None)
}
pub(crate) fn update_rpc(s: &State, p: &Value) -> Result<Value, Value> {
    let mut policy_error = None;
    update_inner(s, p, &mut policy_error).map_err(|message| {
        let mut error =
            policy_error.unwrap_or_else(|| json!({"operation":"write","message":message}));
        error["_tag"] = json!("ServerSettingsError");
        error["settingsPath"] = json!(s.dir.join("settings.json"));
        error
    })
}
fn update_inner(s: &State, p: &Value, policy_error: &mut Option<Value>) -> Result<Value, String> {
    // Never acquire the provider pointer mutex while holding the settings lock:
    // provider inspection takes the shared settings snapshot lock itself.
    let provider = s.provider.lock().unwrap().clone();
    let _guard = s.settings_lock.lock().unwrap();
    let patch = p
        .get("patch")
        .filter(|v| v.is_object())
        .ok_or("patch must be an object")?;
    let mut v = read_stored(s)?;
    let previous = v.clone();
    // Preserve sensitive environment values echoed by clients as valueRedacted.
    let mut patch = patch.clone();
    if let Some(entries) = patch.get("secretProviders") {
        let decoded =
            crate::managed_settings_codec::decode_patch(json!({"secretProviders":entries}))
                .map_err(|_| "Invalid secret provider patch")?;
        for (id, entry) in decoded["secretProviders"].as_object().unwrap() {
            if entry.is_null() {
                continue;
            }
            let kind = entry["kind"].as_str().unwrap();
            let problem = match s.secret_resolver.validates_config(kind, &entry["config"]) {
                None => Some(format!("Unsupported secret provider kind: {kind}.")),
                Some(false) => Some(format!("Invalid {kind} config for secret provider {id}.")),
                Some(true) => None,
            };
            if let Some(problem) = problem {
                let message = format!(
                    "Server settings write-secret-provider failed at {}.",
                    s.dir.join("settings.json").display()
                );
                *policy_error = Some(
                    json!({"operation":"write-secret-provider","message":message,"cause":{"_tag":"Error","message":problem}}),
                );
                return Err(message);
            }
        }
        patch["secretProviders"] = decoded["secretProviders"].clone();
    }
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
    v = crate::settings_secret_transaction::echo(&previous, &v);
    normalize_preferences(&previous, &mut v, &patch)?;
    normalize(&mut v, &patch)?;
    if let Some(controls) = s.managed_policy.settings_controls()? {
        let current = crate::managed_settings_codec::decode(previous.clone())
            .map_err(|_| "Invalid current managed settings")?;
        let next = crate::managed_settings_codec::decode(v.clone())
            .map_err(|_| "Invalid updated managed settings")?;
        let decoded_patch = crate::managed_settings_codec::decode_patch(patch.clone())
            .map_err(|_| "Invalid managed settings patch")?;
        let vetted = match controls
            .vet_write(&current, &next, &decoded_patch)
            .map_err(|_| "Invalid managed settings write")?
        {
            crate::managed_settings::VetWrite::Allowed(value) => value,
            crate::managed_settings::VetWrite::Locked { path } => {
                let organization = s
                    .managed_policy
                    .document
                    .as_ref()
                    .unwrap()
                    .organization
                    .as_str();
                let message = format!("{path} is managed by {organization} and cannot be changed.");
                *policy_error = Some(
                    json!({"operation":"write-managed-setting","managedSetting":path,"managedBy":organization,"message":message}),
                );
                return Err(message);
            }
        };
        v = with_native_fields(
            &v,
            crate::managed_settings_codec::encode(vetted)
                .map_err(|_| "Invalid managed settings encoding")?,
        );
    }
    v = s
        .managed_policy
        .vet_provider_write_rpc(&previous, &v)
        .map_err(|error| {
            let message = error["message"].as_str().unwrap().to_owned();
            *policy_error = Some(error);
            message
        })?;
    v = clear_removed_provider_references(&previous, &v)?;
    let mut previous_private = private_projection(&previous)?;
    let next_private = private_projection(&v).map_err(|_| {
        let message = format!(
            "Server settings normalize failed at {}.",
            s.dir.join("settings.json").display()
        );
        *policy_error = Some(json!({"operation":"normalize","message":message}));
        message
    })?;
    // Switching to an unsupported driver leaves its opaque private state alone.
    // Its storage/launch activation is a separate consumer boundary.
    if let Some(entries) = previous_private["providerInstances"].as_object_mut() {
        entries.retain(|id, _| {
            v["providerInstances"].get(id).is_none()
                || next_private["providerInstances"].get(id).is_some()
        });
    }
    let (private_settings, private_changes) =
        crate::settings_secret_transaction::plan(&previous_private, &next_private);
    apply_private_projection(&mut v, &private_settings)?;
    let persisted = if let Some(controls) = s.managed_policy.settings_controls()? {
        let decoded = crate::managed_settings_codec::decode(v.clone())
            .map_err(|_| "Invalid managed settings persistence")?;
        let defaults = crate::managed_settings_codec::defaults();
        let is_atomic = |key: &str| {
            matches!(
                key,
                "backgroundActivity"
                    | "automaticGitFetchInterval"
                    | "providerHealthRefreshInterval"
                    | "sourceControlWriterModelSelection"
                    | "reviewGuideModelSelection"
                    | "textGenerationModelSelection"
                    | "fallbackModelSelection"
                    | "pullRequestMergeMethod"
            ) || defaults.get(key).is_some_and(Value::is_null)
        };
        let stripped = strip_defaults(&decoded, &controls.defaults_value, &is_atomic)
            .unwrap_or_else(|| json!({}));
        let raw = match std::fs::read(s.dir.join("settings.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => return Err(error.to_string()),
        };
        let kept = if controls.has_defaults() {
            controls
                .keep_stored(stripped, &decoded, &raw, is_atomic)
                .map_err(|_| "Invalid managed settings stored defaults")?
        } else {
            stripped
        };
        with_native_fields(
            &v,
            crate::managed_settings_codec::encode_sparse(kept)
                .map_err(|_| "Invalid managed settings sparse encoding")?,
        )
    } else {
        v.clone()
    };
    use crate::server_secret_store::FileSecretStore;
    let private_store = if private_changes.is_empty() {
        None
    } else {
        Some(FileSecretStore::open(&s.dir.join("secrets")).map_err(|cause| {
            let error=json!({"operation":"write-secret","message":format!("Failed to secure private settings storage ({}).",cause.tag),"cause":{"_tag":cause.tag,"resource":cause.resource}});
            let message=error["message"].as_str().unwrap().to_owned();*policy_error=Some(error);message
        })?)
    };
    let applied = if let Some(store) = &private_store {
        let applied = crate::settings_secret_transaction::apply(store, &private_changes).map_err(
            |cause| {
                let error = private_error(s, &cause);
                let message = error["message"].as_str().unwrap().to_owned();
                *policy_error = Some(error);
                message
            },
        )?;
        // Verify the exact selected snapshot before publishing its public markers.
        if let Err(cause) =
            crate::settings_secret_transaction::materialize(store, &private_settings)
        {
            let failures = crate::settings_secret_transaction::rollback(store, &applied);
            let mut cause = cause;
            cause.rollback_errors.extend(failures);
            let error = private_error(s, &cause);
            let message = error["message"].as_str().unwrap().to_owned();
            *policy_error = Some(error);
            return Err(message);
        }
        applied
    } else {
        Vec::new()
    };
    if let Err(error) = crate::profile::atomic_json(&s.dir.join("settings.json"), &persisted) {
        if let Some(store) = &private_store {
            for failure in crate::settings_secret_transaction::rollback(store, &applied) {
                eprintln!(
                    "Failed to roll back settings secret: {} ({})",
                    failure.resource, failure.tag
                );
            }
        }
        return Err(error.to_string());
    }
    *s.settings_cache.lock().unwrap() = None;
    if private_changes
        .iter()
        .any(|change| change.secret_name.starts_with("usage-limit-source-"))
    {
        crate::config_sources::private_settings_changed(s);
    }
    s.notify();
    s.streams.ops.scheduler_wake.notify_one();
    if patch.get("providerInstances").is_some()
        || patch.get("projectSettingsOverrides").is_some()
        || p.get("providerInstanceMutation").is_some()
    {
        if let (Ok(runtime), Some(provider)) = (tokio::runtime::Handle::try_current(), provider) {
            let changed = s.changed.clone();
            let cwd = std::env::current_dir().unwrap_or_else(|_| s.dir.clone());
            runtime.spawn(async move {
                let _ = provider.discover(cwd).await;
                changed.send_modify(|v| *v = v.wrapping_add(1));
            });
        }
    }
    v = s.managed_policy.effective_settings(&v)?;
    redact_client(&mut v)?;
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
                        let previous = old["providerInstances"][id]["environment"]
                            .as_array()
                            .and_then(|a| a.iter().find(|p| p["name"] == var["name"]));
                        var["value"] = previous
                            .map(|prev| prev["value"].clone())
                            .unwrap_or_else(|| json!(""));
                        if previous.is_none_or(|prev| prev["valueRedacted"] != true) {
                            var.as_object_mut().unwrap().remove("valueRedacted");
                        }
                    }
                }
            }
        }
    }
}
pub fn config(s: &State) -> Result<Value, String> {
    let (keybindings, issues) = crate::keybind::resolved(s);
    let provider = s.provider.lock().unwrap().clone();
    let providers = provider
        .map(|provider| provider.config())
        .unwrap_or_default();
    let mut config = json!({"environment":descriptor(s),"directEndpoints":direct_endpoints(s),"auth":auth_descriptor_for(s),"cwd":std::env::current_dir().unwrap_or_default(),"keybindingsConfigPath":s.dir.join("keybindings.json"),"keybindings":keybindings,"issues":issues,"providers":providers,"availableEditors":crate::files::available_editors(),"observability":{"logsDirectoryPath":s.dir.join("logs"),"localTracingEnabled":false,"otlpTracesEnabled":false,"otlpMetricsEnabled":false,"otlpLogsEnabled":false},"settings":get(s)?,"shellResumeCompletionMarker":true,"threadResumeCompletionMarker":true,"shellThreadFamilies":false,"shellOlderSettled":false,"threadSnapshotPagination":false});
    config["settingsSupport"] = json!({"enableAgentBrowserAccess":{"supported":true},"projectAgentBrowserAccessOverrides":{"supported":true}});
    if let Some(summary) = s.managed_policy.basic_summary() {
        config["managedPolicy"] = summary;
    }
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
        "secretProviders",
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
        "fallbackModelSelection",
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
            crate::provider_policy::fold_enabled(instance);
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
        "fallbackModelSelection",
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
            crate::provider_policy::fold_enabled(instance);
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
