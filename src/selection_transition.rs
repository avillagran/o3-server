//! Selection/session policy adapted from nightly4179 ProviderSessionTransitionPolicy (MIT).
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Selection {
    ApplyOnNextTurn,
    RestartSession,
    CreateWithHandoff,
    Reject { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transition {
    Reuse,
    SwitchModelInSession,
    RestartAndResume,
    CreateWithHandoff,
    Reject { reason: String },
}
fn options(selection: &Value) -> Vec<(&str, &Value)> {
    let mut options: Vec<_> = selection["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| (o["id"].as_str().unwrap_or(""), &o["value"]))
        .collect();
    // Stable sorting preserves typed values whose string forms tie (e.g. true and "true").
    options.sort_by(|(a, av), (b, bv)| {
        a.cmp(b)
            .then_with(|| option_string(av).cmp(&option_string(bv)))
    });
    options
}
fn option_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
pub(crate) fn option_key(value: &Value) -> Value {
    let selection = json!({"options":value});
    serde_json::to_value(options(&selection)).unwrap()
}
pub fn model_selections_equal(left: &Value, right: &Value) -> bool {
    left["instanceId"] == right["instanceId"]
        && left["model"] == right["model"]
        && options(left) == options(right)
}
pub fn acp(input: &Value) -> Selection {
    if input["current"]["model"] != input["target"]["model"]
        && input["sessionCapabilities"]["sessions"]["supportsModelSwitchInSession"] != true
    {
        Selection::Reject {
            reason: "The active ACP session does not expose a model-switch capability.".into(),
        }
    } else {
        Selection::ApplyOnNextTurn
    }
}
pub fn decide(input: &Value) -> Transition {
    let current = &input["current"];
    let target = &input["target"];
    if target["available"] != true {
        return Transition::Reject {
            reason: "The target provider instance is unavailable.".into(),
        };
    }
    if current.is_null() {
        return Transition::CreateWithHandoff;
    }
    if current["driver"] != target["driver"]
        || current["continuationIdentity"]["driverKind"]
            != target["continuationIdentity"]["driverKind"]
        || current["continuationIdentity"]["continuationKey"]
            != target["continuationIdentity"]["continuationKey"]
    {
        return Transition::CreateWithHandoff;
    }
    let instance_changed =
        current["modelSelection"]["instanceId"] != target["modelSelection"]["instanceId"];
    let changed = !model_selections_equal(&current["modelSelection"], &target["modelSelection"]);
    let selection = input
        .get("selectionTransition")
        .cloned()
        .and_then(|v| serde_json::from_value::<Selection>(v).ok());
    if changed && !instance_changed {
        match &selection {
            Some(Selection::CreateWithHandoff) => return Transition::CreateWithHandoff,
            Some(Selection::Reject { reason }) => {
                return Transition::Reject {
                    reason: reason.clone(),
                }
            }
            None => {
                return Transition::Reject {
                    reason: "The provider adapter did not classify the selection change.".into(),
                }
            }
            _ => {}
        }
    }
    if instance_changed
        || current["runtimeMode"] != target["runtimeMode"]
        || current["workspace"] != target["workspace"]
    {
        return Transition::RestartAndResume;
    }
    if changed {
        match selection {
            Some(Selection::ApplyOnNextTurn) => return Transition::SwitchModelInSession,
            Some(Selection::RestartSession) => return Transition::RestartAndResume,
            _ => {}
        }
    }
    Transition::Reuse
}
/// Native adapters expose this same turn-scoped classification independently of cached capabilities.
pub(crate) fn turn_scoped() -> Option<Selection> {
    Some(Selection::ApplyOnNextTurn)
}
pub(crate) fn native_capabilities(instance: &str) -> Option<Value> {
    match instance {
        "codex" => Some(serde_json::from_str(include_str!("codex-capabilities.json")).unwrap()),
        "claudeAgent" | "claude" | "claude-code" => {
            Some(serde_json::from_str(include_str!("claude-capabilities.json")).unwrap())
        }
        _ => None,
    }
}
/// Preserve runtime policy for the admitted attempt when thread preferences change before spawn.
pub(crate) fn freeze_policy(thread: &Value, run: &mut Value) {
    run["executionPolicy"] =
        json!({"runtimeMode":thread["runtimeMode"],"interactionMode":thread["interactionMode"]});
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_typescript_policy_classifiers_and_equality_match() {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/selection-transition.json"))
                .unwrap();
        for c in v["cases"].as_array().unwrap() {
            assert_eq!(
                serde_json::to_value(decide(&c["input"])).unwrap(),
                c["expected"],
                "{}",
                c["name"]
            );
        }
        for c in v["equality"].as_array().unwrap() {
            assert_eq!(
                model_selections_equal(&c["left"], &c["right"]),
                c["expected"].as_bool().unwrap(),
                "{c}"
            );
        }
        for c in v["acp"].as_array().unwrap() {
            assert_eq!(
                serde_json::to_value(acp(&c["input"])).unwrap(),
                c["expected"],
                "{c}"
            );
        }
    }
}
