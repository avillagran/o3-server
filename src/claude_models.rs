//! Claude model catalog. The pinned T3 manifest supplies curated profiles (effort maps,
//! context windows, extra options); the models the installed Claude CLI reports in its
//! `initialize` response are merged on top, so a model the CLI ships is listed without a
//! server release.
use serde_json::{json, Value};
fn manifest() -> Value {
    serde_json::from_str(include_str!("claude-models.json")).unwrap()
}

/// `claude-haiku-4-5-20251001` -> `claude-haiku-4-5`.
fn undated(slug: &str) -> &str {
    match slug.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => slug,
    }
}

fn effort_label(level: &str) -> String {
    match level {
        "xhigh" => "Extra High".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// Option descriptors for a model the pinned manifest does not know, built from the
/// capability flags the CLI reports for it.
fn reported_capabilities(model: &Value) -> Value {
    let mut descriptors = Vec::new();
    let levels: Vec<&str> = model["supportedEffortLevels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if model["supportsEffort"] == true && !levels.is_empty() {
        let default = if levels.contains(&"medium") {
            "medium"
        } else {
            levels[0]
        };
        let mut options: Vec<Value> = levels
            .iter()
            .map(|level| {
                let mut option = json!({"id":level,"label":effort_label(level)});
                if *level == default {
                    option["isDefault"] = json!(true);
                }
                option
            })
            .collect();
        if levels.contains(&"xhigh") {
            options.push(json!({"id":"ultracode","label":"Ultracode","description":"xhigh effort plus multi-agent workflow orchestration"}));
        }
        let mut descriptor =
            json!({"id":"effort","label":"Reasoning","type":"select","options":options});
        if model["supportsAdaptiveThinking"] == true {
            descriptor["options"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"ultrathink","label":"Ultrathink"}));
            descriptor["promptInjectedValues"] = json!(["ultrathink"]);
        }
        descriptors.push(descriptor);
    } else if model["supportsThinkingOff"] == true {
        descriptors.push(json!({"id":"thinking","label":"Thinking","type":"boolean"}));
    }
    if model["supportsFastMode"] == true {
        descriptors.push(json!({"id":"fastMode","label":"Fast Mode","type":"boolean"}));
    }
    json!({"optionDescriptors":descriptors})
}

/// Pinned entries with the CLI-reported models merged in. A reported model the manifest
/// knows keeps its curated profile; a reported alias (`fable`, `opus`) moves to the model
/// the CLI resolves it to; a model the manifest lacks is added with reported capabilities.
fn catalog(reported: &[Value]) -> Vec<Value> {
    let m = manifest();
    let mut entries: Vec<Value> = m["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            entry["capabilities"] =
                m["profiles"][entry["profile"].as_str().unwrap()]["capabilities"].clone();
            if !entry["aliases"].is_array() {
                entry["aliases"] = json!([]);
            }
            entry
        })
        .collect();
    let mut added = Vec::new();
    for model in reported {
        let Some(resolved) = model["resolvedModel"].as_str() else {
            continue;
        };
        let value = model["value"].as_str().unwrap_or(resolved);
        if value == "default" || !resolved.starts_with("claude-") || resolved.contains('[') {
            continue;
        }
        let matches = |e: &Value| {
            e["slug"] == resolved
                || e["slug"] == undated(resolved)
                || e["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a == resolved)
        };
        let index = match entries.iter().position(matches) {
            Some(index) => index,
            None => {
                let name = model["displayName"].as_str().unwrap_or(resolved);
                let name = if name.starts_with("Claude") {
                    name.to_owned()
                } else {
                    format!("Claude {name}")
                };
                entries.push(json!({"slug":resolved,"name":name,"aliases":[],"status":"current","capabilities":reported_capabilities(model)}));
                added.push(resolved.to_owned());
                entries.len() - 1
            }
        };
        if value != resolved {
            for entry in entries.iter_mut() {
                entry["aliases"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|a| a != value);
            }
            entries[index]["aliases"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!(value));
        }
    }
    // Newly reported models are the newest: list them first, in the CLI's order.
    let (mut new, pinned): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|e| added.iter().any(|slug| e["slug"] == slug.as_str()));
    new.extend(pinned);
    new
}

fn find<'a>(entries: &'a [Value], name: &str) -> Option<&'a Value> {
    entries.iter().find(|e| {
        e["slug"] == name
            || e["aliases"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a == name))
    })
}

pub(crate) fn models(reported: &[Value]) -> Value {
    let default = manifest()["defaults"]["chat"].clone();
    json!(catalog(reported).iter().map(|entry| {
        json!({"slug":entry["slug"],"name":entry["name"],"aliases":entry["aliases"],"status":entry["status"],"isDefault":entry["slug"]==default,"isCustom":false,"capabilities":entry["capabilities"]})
    }).collect::<Vec<_>>())
}
pub(crate) fn compile(
    selection: &Value,
    reported: &[Value],
) -> Result<(String, Option<String>, Value, bool), String> {
    let m = manifest();
    let entries = catalog(reported);
    let name = selection["modelId"]
        .as_str()
        .or_else(|| selection["model"].as_str())
        .unwrap_or("haiku");
    let entry = find(&entries, name);
    let profile = entry
        .and_then(|e| e["profile"].as_str())
        .map(|p| &m["profiles"][p]);
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
                    .unwrap_or(if raw == "ultracode" { "xhigh" } else { raw });
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

pub(crate) fn context_window(selection: &Value) -> Option<u64> {
    let m = manifest();
    let name = selection["model"].as_str()?;
    let entry = m["models"].as_array()?.iter().find(|e| {
        e["slug"] == name
            || e["aliases"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a == name))
    })?;
    let adapter = &m["profiles"][entry["profile"].as_str()?]["adapter"]["claudeCode"];
    adapter["fixedContextWindowTokens"].as_u64().or_else(|| {
        let option = selection["options"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["id"] == "contextWindow")
            .and_then(|o| o["value"].as_str())
            .unwrap_or("200k");
        adapter["contextWindowTokens"][option].as_u64()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_catalog_and_effort_maps_compile() {
        let catalog = models(&[]);
        assert!(catalog.as_array().unwrap().len() > 3);
        let (model,effort,settings,prompt)=compile(&json!({"model":"claude-opus-4-7","options":[{"id":"effort","value":"xhigh"},{"id":"fastMode","value":true}]}), &[]).unwrap();
        assert_eq!(model, "claude-opus-4-7");
        assert_eq!(effort.as_deref(), Some("max"));
        assert_eq!(settings["fastMode"], true);
        assert!(!prompt);
        assert!(compile(
            &json!({"model":"custom","options":[{"id":"effort","value":"invalid"}]}),
            &[]
        )
        .is_err());
    }

    /// Shape of the `models` list in the CLI's `initialize` response (2.1.322).
    fn reported() -> Vec<Value> {
        let levels = json!(["low", "medium", "high", "xhigh", "max"]);
        vec![
            json!({"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)"}),
            json!({"value":"fable","resolvedModel":"claude-fable-5-2","displayName":"Fable 5.2","supportsEffort":true,"supportedEffortLevels":levels,"supportsAdaptiveThinking":true,"supportsFastMode":true}),
            json!({"value":"claude-fable-5-1","resolvedModel":"claude-fable-5-1","displayName":"Fable 5.1","supportsEffort":true,"supportedEffortLevels":levels}),
            json!({"value":"claude-haiku-4-5-20251001","resolvedModel":"claude-haiku-4-5-20251001","displayName":"Haiku 4.5","supportsThinkingOff":true}),
        ]
    }

    #[test]
    fn models_the_cli_reports_are_listed_without_a_release() {
        let catalog = models(&reported());
        let list = catalog.as_array().unwrap();
        let fable = &list[0];
        assert_eq!(fable["slug"], "claude-fable-5-2");
        assert_eq!(fable["name"], "Claude Fable 5.2");
        assert_eq!(fable["status"], "current");
        assert_eq!(fable["aliases"], json!(["fable"]));
        let ids: Vec<_> = fable["capabilities"]["optionDescriptors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["effort", "fastMode"]);
        let efforts: Vec<_> = fable["capabilities"]["optionDescriptors"][0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            efforts,
            [
                "low",
                "medium",
                "high",
                "xhigh",
                "max",
                "ultracode",
                "ultrathink"
            ]
        );
        // The alias moved off the pinned model; the pinned profile and dated ids are kept.
        let fable_51 = list
            .iter()
            .find(|m| m["slug"] == "claude-fable-5-1")
            .unwrap();
        assert!(!fable_51["aliases"]
            .as_array()
            .unwrap()
            .contains(&json!("fable")));
        assert_eq!(
            fable_51["capabilities"],
            models(&[])
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["slug"] == "claude-fable-5-1")
                .unwrap()["capabilities"]
        );
        assert_eq!(
            list.iter()
                .filter(|m| m["slug"].as_str().unwrap().starts_with("claude-haiku-4-5"))
                .count(),
            1
        );
        assert_eq!(list.len(), models(&[]).as_array().unwrap().len() + 1);

        let (model, effort, settings, prompt) = compile(&json!({"model":"fable","options":[{"id":"effort","value":"ultracode"},{"id":"fastMode","value":true}]}), &reported()).unwrap();
        assert_eq!(model, "claude-fable-5-2");
        assert_eq!(effort.as_deref(), Some("xhigh"));
        assert_eq!(settings["fastMode"], true);
        assert_eq!(settings["ultracode"], true);
        assert!(!prompt);
        assert!(compile(&json!({"model":"claude-fable-5-2","options":[{"id":"effort","value":"ultrathink"}]}), &reported()).unwrap().3);
        // Without a CLI report the pinned alias still applies.
        assert_eq!(
            compile(&json!({"model":"fable"}), &[]).unwrap().0,
            "claude-fable-5-1"
        );
    }
}
