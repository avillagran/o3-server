//! Native managed settings overlay; encoded policy values never replace stored user choices.
use crate::{
    managed_policy::{RuntimeMode, Setting},
    managed_settings_codec as codec,
};
use indexmap::IndexMap;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    segments: Vec<String>,
    value: Value,
    decoded: Option<Value>,
}
#[derive(Clone, Debug)]
pub(crate) struct ManagedSettings {
    defaults: Vec<Entry>,
    locks: Vec<Entry>,
    defaults_tree: Value,
    pub(crate) defaults_value: Value,
    ceiling: Option<RuntimeMode>,
}
#[derive(Debug, PartialEq)]
pub(crate) enum VetWrite {
    Allowed(Value),
    Locked { path: String },
}
fn get<'a>(tree: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut v = tree;
    for key in path {
        v = v.as_object()?.get(key)?
    }
    Some(v)
}
fn set(tree: &mut Value, path: &[String], value: Option<Value>) {
    if path.is_empty() {
        *tree = value.unwrap_or(Value::Null);
        return;
    }
    if !tree.is_object() {
        *tree = json!({})
    }
    let obj = tree.as_object_mut().unwrap();
    if path.len() == 1 {
        if let Some(v) = value {
            obj.insert(path[0].clone(), v);
        } else {
            obj.remove(&path[0]);
        }
    } else {
        set(
            obj.entry(path[0].clone()).or_insert(Value::Null),
            &path[1..],
            value,
        )
    }
}
fn fill(target: &Value, defaults: &Value) -> Value {
    let (Some(t), Some(d)) = (target.as_object(), defaults.as_object()) else {
        return target.clone();
    };
    let mut out = t.clone();
    for (k, v) in d {
        out.insert(
            k.clone(),
            t.get(k).map_or_else(|| v.clone(), |t| fill(t, v)),
        );
    }
    Value::Object(out)
}
fn adds(existing: &Value, value: &Value) -> bool {
    match (existing.as_object(), value.as_object()) {
        (Some(e), Some(v)) => v.iter().any(|(k, v)| e.get(k).is_none_or(|e| adds(e, v))),
        _ => false,
    }
}
fn matches(value: &Value, lock: Option<&Value>) -> bool {
    match (value.as_object(), lock.and_then(Value::as_object)) {
        (Some(v), Some(l)) => v.iter().all(|(k, v)| matches(v, l.get(k))),
        _ => Some(value) == lock,
    }
}
fn sync_idle(mut settings: Value) -> Value {
    if let Some(mode) = settings.get("idleCompaction") {
        let legacy = mode == "beforeCacheExpires";
        settings["compactIdleThreads"] = json!(legacy);
    }
    settings
}
fn above(value: &Value, ceiling: RuntimeMode) -> bool {
    serde_json::from_value::<RuntimeMode>(value.clone()).is_ok_and(|mode| mode > ceiling)
}
fn targets(tree: &Value, entry: &Entry) -> Vec<Vec<String>> {
    let mut out = vec![entry.segments.clone()];
    let head = &entry.segments[0];
    if codec::project_override_keys().iter().any(|key| key == head) {
        if let Some(projects) = tree["projectSettingsOverrides"].as_object() {
            for (id, overrides) in projects {
                if overrides.as_object().is_some_and(|v| v.contains_key(head)) {
                    let mut p = vec!["projectSettingsOverrides".into(), id.clone()];
                    p.extend(entry.segments.clone());
                    out.push(p);
                }
            }
        }
    }
    out
}
fn credential(v: &Value) -> bool {
    if let Some(s) = v.as_str() {
        return !s.is_empty() && s != "••••••";
    }
    if codec::is_secret_reference(v) {
        return false;
    }
    v.as_object().is_some_and(|v| {
        v.values()
            .any(|v| v.as_str().is_some_and(|s| !s.is_empty() && s != "••••••"))
    })
}
fn holds_credential(settings: &Value) -> bool {
    let descriptors = codec::secret_fields();
    let has_fields = |value: &Value, fields: &Value| {
        fields
            .as_array()
            .into_iter()
            .flatten()
            .any(|key| value.get(key.as_str().unwrap()).is_some_and(credential))
    };
    if has_fields(&settings["cloudSpeechCredentials"], &descriptors["voice"]) {
        return true;
    }
    if settings["providerInstances"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.values())
        .any(|instance| {
            has_fields(
                &instance["config"],
                &descriptors["providers"][instance["driver"].as_str().unwrap_or("")],
            ) || instance["environment"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|v| {
                    v["sensitive"] == true
                        && v["valueRedacted"] != true
                        && v["value"].as_str().is_some_and(|s| !s.is_empty())
                })
        })
    {
        return true;
    }
    if settings["sourceControlHosts"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.iter())
        .any(|(kind, v)| has_fields(v, &descriptors["hosts"][kind]))
    {
        return true;
    }
    for (group, fields) in [
        ("usageLimitSources", "usage"),
        ("easDeviceHosts", "eas"),
        ("simlockDeviceHosts", "simlock"),
    ] {
        let values: Vec<&Value> = if let Some(v) = settings[group].as_object() {
            v.values().collect()
        } else {
            settings[group].as_array().into_iter().flatten().collect()
        };
        if values.iter().any(|v| has_fields(v, &descriptors[fields])) {
            return true;
        }
    }
    false
}
impl ManagedSettings {
    pub(crate) fn make(
        settings: &IndexMap<String, Setting>,
        ceiling: Option<RuntimeMode>,
    ) -> Result<Self, String> {
        let mut defaults = Vec::new();
        let mut locks = Vec::new();
        let mut entries: Vec<_> = settings.iter().collect();
        entries.sort_by_key(|(key, _)| codec::array_index(key).map_or((1, 0), |v| (0, v)));
        for (path, setting) in entries {
            let segments: Vec<String> = path.split('.').map(str::to_owned).collect();
            let head = &segments[0];
            if segments.iter().any(String::is_empty) {
                return Err(format!("\"{path}\" is not a settings path"));
            }
            if !codec::fields().contains_key(head)
                || [
                    "projectSettingsOverrides",
                    "projectAgentBrowserAccessOverrides",
                    "projectAutoPullOverrides",
                    "projectScriptOverrides",
                ]
                .contains(&head.as_str())
            {
                return Err(format!(
                    "\"{path}\" is not a setting this version can manage"
                ));
            }
            if head == "observability" {
                return Err(format!("\"{path}\" is resolved at startup from each user's environment, settings file and desktop bootstrap input, which a policy cannot override"));
            }
            if head == "compactIdleThreads" {
                return Err("\"compactIdleThreads\" follows \"idleCompaction\"; manage \"idleCompaction\" instead".into());
            }
            let e = Entry {
                path: path.clone(),
                segments,
                value: setting.value.clone(),
                decoded: None,
            };
            if setting.locked {
                locks.push(e)
            } else {
                defaults.push(e)
            }
        }
        for lock in &locks {
            if (lock.path.starts_with("storageCleanup.worktree")
                || "storageCleanup.worktree".starts_with(&(lock.path.clone() + ".")))
                && !locks.iter().any(|l| l.path == "worktreeCleanup")
            {
                return Err(format!(
                    "\"{}\" can be overridden by \"worktreeCleanup\"; lock \"worktreeCleanup\" too",
                    lock.path
                ));
            }
            if lock.segments[0] == "providerInstances" && lock.segments.len() > 2 {
                return Err(format!("\"{}\" is one field of a provider instance whose fields override each other; lock \"providerInstances.{}\" as a whole, or pin fields with limits.providers.instances",lock.path,lock.segments[1]));
            }
        }
        let built_in = codec::encoded_defaults();
        let all: Vec<_> = defaults.iter().chain(&locks).collect();
        for entry in &all {
            for i in 1..entry.segments.len() {
                let mut probe = built_in.clone();
                set(&mut probe, &entry.segments[..i], Some(Value::Null));
                if codec::decode(probe).is_ok() {
                    return Err(format!(
                        "\"{}\" is inside a setting that can be empty; manage \"{}\" as a whole",
                        entry.path, entry.segments[0]
                    ));
                }
            }
        }
        let mut applied = built_in.clone();
        for e in &all {
            set(&mut applied, &e.segments, Some(e.value.clone()))
        }
        let decoded = codec::decode(applied)
            .map_err(|_| "a managed setting has a value of the wrong type".to_owned())?;
        let round_trip = codec::encode(decoded.clone())
            .map_err(|_| "a managed setting has a value of the wrong type".to_owned())?;
        for e in &all {
            if get(&round_trip, &e.segments) != Some(&e.value) {
                return Err(format!(
                    "\"{}\" is not a setting this version can manage, or its value is invalid",
                    e.path
                ));
            }
        }
        if holds_credential(&decoded) {
            return Err("a managed setting holds a credential, which every user can read in the policy; deploy credentials another way".into());
        }
        for e in &defaults {
            if get(&built_in, &e.segments)
                .map_or(e.segments.len() > 1, |existing| adds(existing, &e.value))
            {
                return Err(format!(
                    "\"{}\" adds entries users could not remove; lock it instead",
                    e.path
                ));
            }
        }
        if ceiling.is_some_and(|c| {
            all.iter().any(|e| e.path == "defaultRuntimeMode")
                && above(&decoded["defaultRuntimeMode"], c)
        }) {
            return Err("\"defaultRuntimeMode\" is above \"limits.maxRuntimeMode\"".into());
        }
        let mut tree = json!({});
        let mut defaults_value = built_in;
        for e in &defaults {
            set(&mut tree, &e.segments, Some(e.value.clone()));
            set(&mut defaults_value, &e.segments, Some(e.value.clone()));
        }
        for e in &mut locks {
            e.decoded = get(&decoded, &e.segments).cloned()
        }
        Ok(Self {
            defaults,
            locks,
            defaults_tree: tree,
            defaults_value: sync_idle(
                codec::decode(defaults_value).expect("validated policy defaults"),
            ),
            ceiling,
        })
    }
    pub(crate) fn locked_paths(&self) -> Vec<String> {
        self.locks.iter().map(|e| e.path.clone()).collect()
    }
    pub(crate) fn with_defaults(&self, persisted: &Value) -> Value {
        let mut tree = self.defaults_tree.clone();
        if persisted.get("compactIdleThreads").is_some()
            && persisted.get("idleCompaction").is_none()
        {
            tree.as_object_mut().unwrap().remove("idleCompaction");
        }
        fill(persisted, &tree)
    }
    pub(crate) fn apply_locks(&self, current: &Value) -> Result<Value, ()> {
        let mut locked = codec::encode(current.clone())?;
        for e in &self.locks {
            for t in targets(&locked, e) {
                set(&mut locked, &t, Some(e.value.clone()));
            }
        }
        let mut locked = if self.locks.is_empty() {
            current.clone()
        } else {
            codec::decode(locked)?
        };
        if self.locks.iter().any(|e| e.path == "idleCompaction") {
            locked = sync_idle(locked)
        }
        if let Some(ceiling) = self.ceiling {
            let value = serde_json::to_value(ceiling).unwrap();
            if above(&locked["defaultRuntimeMode"], ceiling) {
                locked["defaultRuntimeMode"] = value.clone()
            }
            if let Some(projects) = locked["projectSettingsOverrides"].as_object_mut() {
                for project in projects.values_mut() {
                    if above(&project["defaultRuntimeMode"], ceiling) {
                        project["defaultRuntimeMode"] = value.clone()
                    }
                }
            }
        }
        Ok(locked)
    }
    pub(crate) fn vet_write(
        &self,
        current: &Value,
        next: &Value,
        patch: &Value,
    ) -> Result<VetWrite, ()> {
        if self.locks.is_empty() {
            return Ok(VetWrite::Allowed(next.clone()));
        }
        let before = codec::encode(current.clone())?;
        let mut after = codec::encode(next.clone())?;
        for e in &self.locks {
            if targets(patch, e)
                .iter()
                .any(|t| get(patch, t).is_some_and(|v| !matches(v, e.decoded.as_ref())))
                || targets(&after, e).iter().any(|t| {
                    get(&after, t)
                        .is_some_and(|v| Some(v) != get(&before, t) && !matches(v, Some(&e.value)))
                })
            {
                return Ok(VetWrite::Locked {
                    path: e.path.clone(),
                });
            }
        }
        for e in &self.locks {
            for t in targets(&after, e) {
                if !(1..t.len()).any(|i| get(&before, &t[..i]).is_none()) {
                    set(&mut after, &t, get(&before, &t).cloned());
                }
            }
        }
        Ok(VetWrite::Allowed(codec::decode(after)?))
    }
    pub(crate) fn has_defaults(&self) -> bool {
        !self.defaults.is_empty()
    }
    pub(crate) fn keep_stored(
        &self,
        mut stripped: Value,
        settings: &Value,
        stored: &Value,
        is_atomic: impl Fn(&str) -> bool,
    ) -> Result<Value, ()> {
        let encoded = codec::encode(settings.clone())?;
        let mut paths: Vec<Vec<String>> = self
            .defaults
            .iter()
            .map(|e| {
                let end = e
                    .segments
                    .iter()
                    .position(|s| is_atomic(s))
                    .map_or(e.segments.len(), |i| i + 1);
                e.segments[..end].to_vec()
            })
            .collect();
        if self.defaults.iter().any(|e| e.path == "idleCompaction") {
            paths.push(vec!["compactIdleThreads".into()]);
        }
        fn restore(
            tree: &mut Value,
            path: &[String],
            settings: &Value,
            encoded: &Value,
            stored: &Value,
            atomic: &impl Fn(&str) -> bool,
        ) {
            let Some(previous) = get(stored, path) else {
                return;
            };
            if let Some(object) = previous
                .as_object()
                .filter(|_| !atomic(path.last().map_or("", String::as_str)))
            {
                for k in object.keys() {
                    let mut p = path.to_vec();
                    p.push(k.clone());
                    restore(tree, &p, settings, encoded, stored, atomic)
                }
            } else if get(tree, path).is_none() && Some(previous) == get(encoded, path) {
                set(tree, path, get(settings, path).cloned())
            }
        }
        for p in paths {
            restore(&mut stripped, &p, settings, &encoded, stored, &is_atomic)
        }
        Ok(stripped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn merged(base: &Value, patch: &Value) -> Value {
        let mut out = base.clone();
        for (k, v) in patch.as_object().unwrap() {
            out[k] = v.clone()
        }
        out
    }
    fn expanded(delta: &Value) -> Value {
        let mut out = codec::encoded_defaults();
        for op in delta.as_array().unwrap() {
            let path = op["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            set(
                &mut out,
                &path,
                if op["missing"] == true {
                    None
                } else {
                    Some(op["value"].clone())
                },
            );
        }
        out
    }
    #[test]
    fn pinned_native_overlay_and_writes() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/managed-settings-4314.json"))
                .unwrap();
        for c in fixtures["cases"].as_array().unwrap() {
            let label = c["label"].as_str().unwrap();
            let settings = serde_json::from_value(c["settings"].clone()).unwrap();
            let max = c
                .get("maxRuntimeMode")
                .map(|v| serde_json::from_value(v.clone()).unwrap());
            let made = ManagedSettings::make(&settings, max);
            if let Some(reason) = c["invalid"].as_str() {
                assert_eq!(
                    made.err().as_deref(),
                    Some(reason),
                    "{label}: invalid reason"
                );
                continue;
            }
            let policy =
                made.unwrap_or_else(|error| panic!("{label}: unexpectedly invalid: {error}"));
            assert_eq!(
                policy.locked_paths(),
                serde_json::from_value::<Vec<String>>(c["lockedPaths"].clone()).unwrap(),
                "{label}: paths"
            );
            assert_eq!(
                codec::encode(policy.defaults_value.clone()).unwrap(),
                expanded(&c["defaults"]),
                "{label}: defaults"
            );
            let current = codec::decode(merged(&codec::encoded_defaults(), &c["current"]))
                .unwrap_or_else(|()| panic!("{label}: current invalid"));
            assert_eq!(
                codec::encode(policy.apply_locks(&current).unwrap()).unwrap(),
                expanded(&c["applied"]),
                "{label}: applied"
            );
            for fill in c["fills"].as_array().unwrap() {
                assert_eq!(
                    policy.with_defaults(&fill["value"]),
                    fill["result"],
                    "{label}: fill"
                )
            }
            for write in c["writes"].as_array().unwrap() {
                let next = codec::decode(merged(
                    &codec::encode(current.clone()).unwrap(),
                    &write["next"],
                ))
                .unwrap();
                let verdict = policy.vet_write(&current, &next, &write["patch"]).unwrap();
                if let Some(path) = write["locked"].as_str() {
                    assert_eq!(
                        verdict,
                        VetWrite::Locked {
                            path: path.to_owned()
                        },
                        "{label}: write {write}"
                    )
                } else {
                    let VetWrite::Allowed(result) = verdict else {
                        panic!("{label}: unexpectedly locked {write}")
                    };
                    assert_eq!(
                        codec::encode(result).unwrap(),
                        expanded(&write["allowed"]),
                        "{label}: kept for {write}"
                    )
                }
            }
            for keep in c["keeps"].as_array().unwrap() {
                let settings = codec::decode(policy.with_defaults(&keep["stored"])).unwrap();
                let kept = policy
                    .keep_stored(json!({}), &settings, &keep["stored"], |key| {
                        keep["atomic"].as_array().unwrap().iter().any(|v| v == key)
                    })
                    .unwrap();
                assert_eq!(kept, keep["result"], "{label}: keep {keep}")
            }
        }
    }
}
