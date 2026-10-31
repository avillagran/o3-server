//! Settings remain on disk; unknown keys survive updates.
use crate::store::State;
use serde_json::{json, Value};
pub const SENTINEL: &str = "••••••";
pub fn descriptor(s: &State) -> Value {
    json!({"environmentId":s.environment_id,"label":s.label,"platform":{"os":std::env::consts::OS,"arch":match std::env::consts::ARCH {"x86_64"=>"x64","aarch64"=>"arm64",_=>"other"}},"serverVersion":crate::rpc::SERVER_VERSION,"orchestrationProtocolVersion":2,"capabilities":{"repositoryIdentity":false}})
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
    if p.get("providerInstanceMutation").is_some() {
        return Err("providerInstanceMutation is not implemented".into());
    }
    let patch = p
        .get("patch")
        .filter(|v| v.is_object())
        .ok_or("patch must be an object")?;
    let mut v = read(s)?;
    // Preserve sensitive environment values echoed by clients as valueRedacted.
    let mut patch = patch.clone();
    preserve_environment(&v, &mut patch);
    merge(&mut v, &patch);
    if let Some(instances) = patch.get("providerInstances") {
        let mut replacement = instances.clone();
        retain_sentinels(&v["providerInstances"], &mut replacement);
        v["providerInstances"] = replacement;
    }
    crate::profile::atomic_json(&s.dir.join("settings.json"), &v).map_err(|e| e.to_string())?;
    s.notify();
    redact(&mut v);
    Ok(v)
}
fn preserve_environment(old: &Value, patch: &mut Value) {
    if let Some(instances) = patch["providerInstances"].as_object_mut() {
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
    Ok(
        json!({"environment":descriptor(s),"auth":auth_descriptor(),"cwd":std::env::current_dir().unwrap_or_default(),"keybindingsConfigPath":s.dir.join("keybindings.json"),"keybindings":[],"issues":[],"providers":[],"availableEditors":[],"observability":{"logsDirectoryPath":s.dir.join("logs"),"localTracingEnabled":false,"otlpTracesEnabled":false,"otlpMetricsEnabled":false,"otlpLogsEnabled":false},"settings":get(s)?,"shellResumeCompletionMarker":true,"threadResumeCompletionMarker":true,"shellThreadFamilies":false,"shellOlderSettled":false,"threadSnapshotPagination":false}),
    )
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
