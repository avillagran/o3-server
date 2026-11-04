//! Shared provider-instance enable policy from nightly4165. Project overrides
//! are authoritative, while registry hydration probes the union of enabled uses.
use serde_json::{json, Value};
pub(crate) fn disabled(project: &str, instance: &str) -> String {
    json!({"_tag":"ThreadManagementProviderInstanceDisabledError","projectId":project,"instanceId":instance,"message":format!("Provider instance \"{instance}\" is disabled for project {project}.")}).to_string()
}
pub(crate) fn disabled_error(message: &str) -> Option<Value> {
    serde_json::from_str::<Value>(message)
        .ok()
        .filter(|e| e["_tag"] == "ThreadManagementProviderInstanceDisabledError")
}
pub(crate) fn display_error(message: String) -> String {
    disabled_error(&message)
        .and_then(|e| e["message"].as_str().map(str::to_owned))
        .unwrap_or(message)
}
pub(crate) fn instance_enabled(instance: &Value) -> bool {
    let outer = instance["enabled"].as_bool();
    let nested = instance["config"]["enabled"].as_bool();
    if outer == Some(false) || nested == Some(false) {
        return false;
    }
    outer.or(nested).unwrap_or(!matches!(
        instance["driver"].as_str(),
        Some("cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity")
    ))
}
/// Exact configured aliases own their settings. Legacy aliases otherwise route
/// through the same canonical entry as the native provider registry.
pub(crate) fn configured_id<'a>(settings: &Value, id: &'a str) -> &'a str {
    if settings["providerInstances"].get(id).is_none() && matches!(id, "claude" | "claude-code") {
        "claudeAgent"
    } else {
        id
    }
}
pub(crate) fn project_enabled(settings: &Value, project: Option<&str>, id: &str) -> bool {
    if let Some(enabled) = project.and_then(|p| {
        settings["projectSettingsOverrides"][p]["providerInstances"][id]["enabled"].as_bool()
    }) {
        return enabled;
    }
    let canonical = configured_id(settings, id);
    if canonical != id {
        if let Some(enabled) = project.and_then(|p| {
            settings["projectSettingsOverrides"][p]["providerInstances"][canonical]["enabled"]
                .as_bool()
        }) {
            return enabled;
        }
    }
    settings["providerInstances"]
        .get(canonical)
        .map(instance_enabled)
        .unwrap_or(matches!(canonical, "codex" | "claudeAgent"))
}
/// Model-valued overrides cannot select a project-disabled instance. Provider
/// instance overrides remain in their own map, never replacing driver configs.
pub(crate) fn project_settings(settings: &Value, project: &str) -> Value {
    let mut effective = settings.clone();
    for (key, value) in settings["projectSettingsOverrides"][project]
        .as_object()
        .into_iter()
        .flatten()
    {
        const KEYS: &[&str] = &[
            "worktreeCleanup",
            "defaultModelSelection",
            "defaultRuntimeMode",
            "defaultThreadEnvMode",
            "newWorktreesStartFromOrigin",
            "worktreeSubmodules",
            "defaultAutoPull",
            "defaultProjectScripts",
            "enableAgentBrowserAccess",
            "enableAgentDeviceAccess",
            "textGenerationModelSelection",
            "threadTitleInstructions",
            "sourceControlWriterModelSelection",
            "reviewGuideModelSelection",
            "reviewGuideInstructions",
            "sourceControlWritingStyle",
            "removeAgentCreditsOnMerge",
            "branchNamingMode",
            "branchNamePrefix",
            "branchNameInstructions",
            "pullRequestMergeMethod",
            "sidebarAutoSettleOnMerge",
            "sidebarAutoSettleAfterDays",
            "sidebarAutoSettleKeepPullRequests",
            "continueThreadsAfterServerUpdate",
            "responseStreamingMode",
        ];
        if !KEYS.contains(&key.as_str()) {
            continue;
        }
        if matches!(
            key.as_str(),
            "defaultModelSelection" | "textGenerationModelSelection"
        ) && !value.is_null()
            && !project_enabled(
                settings,
                Some(project),
                value["instanceId"].as_str().unwrap_or(""),
            )
        {
            continue;
        }
        effective[key] = value.clone();
    }
    effective
}
/// Legacy flags fold only when boolean; invalid driver-specific fields survive.
pub(crate) fn fold_enabled(instance: &mut Value) {
    if let Some(enabled) = instance["config"]["enabled"].as_bool() {
        instance["enabled"] = json!(instance["enabled"] != false && enabled);
        instance["config"]
            .as_object_mut()
            .unwrap()
            .remove("enabled");
    }
}
pub(crate) fn hydrate(settings: &Value) -> Value {
    let mut configured = settings["providerInstances"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for project in settings["projectSettingsOverrides"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.values())
    {
        for (id, entry) in project["providerInstances"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if entry["enabled"] != true {
                continue;
            }
            let Some(instance) = configured.get_mut(id) else {
                continue;
            };
            if instance_enabled(instance) {
                continue;
            }
            instance["enabled"] = json!(true);
            if instance["config"]["enabled"] == false {
                instance["config"]["enabled"] = json!(true)
            }
        }
    }
    Value::Object(configured)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_aliases_share_canonical_policy_unless_they_own_an_exact_entry() {
        let mut settings = json!({"providerInstances":{"claudeAgent":{"driver":"claudeAgent","enabled":false}},"projectSettingsOverrides":{"p":{"providerInstances":{"claudeAgent":{"enabled":true}}}}});
        assert_eq!(configured_id(&settings, "claude"), "claudeAgent");
        assert!(!project_enabled(&settings, None, "claude"));
        assert!(project_enabled(&settings, Some("p"), "claude"));
        settings["projectSettingsOverrides"]["p"]["providerInstances"]["claude"] =
            json!({"enabled":false});
        assert!(!project_enabled(&settings, Some("p"), "claude"));
        settings["providerInstances"]["claude"] = json!({"driver":"codex","enabled":true});
        assert_eq!(configured_id(&settings, "claude"), "claude");
        assert!(project_enabled(&settings, None, "claude"));
        assert!(!project_enabled(&settings, None, "unregistered"));
    }
    #[test]
    fn source_defaults_conflicts_project_precedence_and_hydration() {
        for (driver, default) in [
            ("codex", true),
            ("claudeAgent", true),
            ("ollama", true),
            ("grok", false),
            ("cursor", false),
            ("fx", false),
            ("kiro", false),
            ("muse", false),
            ("pi", false),
            ("opencode", false),
            ("antigravity", false),
        ] {
            for outer in [None, Some(false), Some(true)] {
                for nested in [None, Some(false), Some(true)] {
                    let mut entry = json!({"driver":driver,"config":{}});
                    if let Some(v) = outer {
                        entry["enabled"] = json!(v)
                    }
                    if let Some(v) = nested {
                        entry["config"]["enabled"] = json!(v)
                    }
                    let expected = if outer == Some(false) || nested == Some(false) {
                        false
                    } else {
                        outer.or(nested).unwrap_or(default)
                    };
                    assert_eq!(instance_enabled(&entry), expected, "{entry}");
                    let raw = entry.clone();
                    fold_enabled(&mut entry);
                    assert_eq!(instance_enabled(&entry), expected, "fold {raw}");
                    for project in [None, Some(false), Some(true)] {
                        let mut settings = json!({"providerInstances":{"work":raw},"projectSettingsOverrides":{"p":{}}});
                        if let Some(v) = project {
                            settings["projectSettingsOverrides"]["p"]["providerInstances"] =
                                json!({"work":{"enabled":v}})
                        }
                        assert_eq!(
                            project_enabled(&settings, Some("p"), "work"),
                            project.unwrap_or(expected)
                        );
                        let original = settings.clone();
                        let registry = hydrate(&settings);
                        assert_eq!(
                            instance_enabled(&registry["work"]),
                            expected || project == Some(true)
                        );
                        assert_eq!(settings, original);
                    }
                }
            }
        }
        let settings = json!({"providerInstances":{},"projectSettingsOverrides":{"p":{"providerInstances":{"missing":{"enabled":true}}}}});
        assert!(hydrate(&settings).get("missing").is_none());
        assert!(project_enabled(&settings, Some("p"), "missing"));
        assert!(!project_enabled(&settings, None, "missing"));
        assert!(project_enabled(&settings, None, "codex"));
        assert!(project_enabled(&settings, None, "claudeAgent"));
        assert!(!project_enabled(&settings, None, "opencode"));
        let mut invalid = json!({"driver":"cursor","config":{"enabled":"nope"}});
        fold_enabled(&mut invalid);
        assert!(invalid.get("enabled").is_none());
        assert_eq!(invalid["config"]["enabled"], "nope");
        assert!(!instance_enabled(&invalid));
        let settings = json!({"providerInstances":{"off":{"driver":"codex","enabled":false}},"textGenerationModelSelection":{"instanceId":"claudeAgent","model":"global"},"defaultModelSelection":{"instanceId":"codex","model":"global"},"projectSettingsOverrides":{"p":{"providerInstances":{"off":{"enabled":false}},"textGenerationModelSelection":{"instanceId":"off","model":"project"},"defaultModelSelection":null,"branchNamePrefix":"project/","futureOpaque":true,"projectSettingsOverrides":{},"sourceControlWritingStyle":{"mode":"custom","customInstructions":"project"}}}});
        let effective = project_settings(&settings, "p");
        assert_eq!(effective["textGenerationModelSelection"]["model"], "global");
        assert!(effective["defaultModelSelection"].is_null());
        assert_eq!(effective["branchNamePrefix"], "project/");
        assert_eq!(
            effective["providerInstances"],
            settings["providerInstances"]
        );
        assert_eq!(
            effective["projectSettingsOverrides"],
            settings["projectSettingsOverrides"]
        );
        assert!(effective.get("futureOpaque").is_none());
    }
}
