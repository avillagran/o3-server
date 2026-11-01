//! Pinned T3 manifest data; refresh/version negotiation is deliberately separate.
use serde_json::{json, Value};
fn manifest() -> Value {
    serde_json::from_str(include_str!("claude-models.json")).unwrap()
}
pub(crate) fn models() -> Value {
    let m = manifest();
    json!(m["models"].as_array().unwrap().iter().map(|entry| {
        let profile=&m["profiles"][entry["profile"].as_str().unwrap()];
        json!({"slug":entry["slug"],"name":entry["name"],"aliases":entry.get("aliases").cloned().unwrap_or(json!([])),"status":entry["status"],"isDefault":entry["slug"]==m["defaults"]["chat"],"isCustom":false,"capabilities":profile["capabilities"]})
    }).collect::<Vec<_>>())
}
pub(crate) fn compile(selection: &Value) -> Result<(String, Option<String>, Value, bool), String> {
    let m = manifest();
    let name = selection["modelId"]
        .as_str()
        .or_else(|| selection["model"].as_str())
        .unwrap_or("haiku");
    let entry = m["models"].as_array().unwrap().iter().find(|v| {
        v["slug"] == name
            || v["aliases"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a == name))
    });
    let profile = entry.map(|e| &m["profiles"][e["profile"].as_str().unwrap()]);
    let mut model = entry
        .map(|e| e["slug"].as_str().unwrap())
        .unwrap_or(name)
        .to_owned();
    let mut effort = None;
    let mut settings = json!({});
    let mut prompt_effort = false;
    for option in selection["options"].as_array().into_iter().flatten() {
        let id = option["id"].as_str().ok_or("option id required")?;
        let value = &option["value"];
        match id {
            "effort" => {
                let raw = value.as_str().ok_or("effort must be text")?;
                if raw == "ultrathink" {
                    prompt_effort = true;
                    continue;
                }
                let mapped = profile
                    .and_then(|p| p["adapter"]["claudeCode"]["effortMap"].get(raw))
                    .and_then(Value::as_str)
                    .unwrap_or(raw);
                if !matches!(mapped, "low" | "medium" | "high" | "xhigh" | "max") {
                    return Err("unsupported Claude effort".into());
                }
                effort = Some(mapped.to_owned());
                if raw == "ultracode" {
                    settings["ultracode"] = json!(true);
                }
            }
            "fastMode" | "thinking" => {
                let value = value.as_bool().ok_or("model option must be boolean")?;
                settings[if id == "thinking" {
                    "alwaysThinkingEnabled"
                } else {
                    "fastMode"
                }] = json!(value);
            }
            "contextWindow" => {
                if let Some(suffix) = profile
                    .and_then(|p| {
                        p["adapter"]["claudeCode"]["modelSuffixes"][id]
                            .get(value.as_str().unwrap_or(""))
                    })
                    .and_then(Value::as_str)
                {
                    model.push_str(suffix);
                }
            }
            _ => return Err(format!("unsupported Claude model option: {id}")),
        }
    }
    Ok((model, effort, settings, prompt_effort))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_catalog_and_effort_maps_compile() {
        let catalog = models();
        assert!(catalog.as_array().unwrap().len() > 3);
        let (model,effort,settings,prompt)=compile(&json!({"model":"claude-opus-4-7","options":[{"id":"effort","value":"xhigh"},{"id":"fastMode","value":true}]})).unwrap();
        assert_eq!(model, "claude-opus-4-7");
        assert_eq!(effort.as_deref(), Some("max"));
        assert_eq!(settings["fastMode"], true);
        assert!(!prompt);
        assert!(
            compile(&json!({"model":"custom","options":[{"id":"effort","value":"invalid"}]}))
                .is_err()
        );
    }
}
