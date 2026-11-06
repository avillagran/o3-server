//! Provider-native goal commands and projections, pinned to T3 nightly 4262.
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub(crate) enum Command {
    Show,
    Clear,
    Pause,
    Resume,
    Set(String),
}

// JavaScript's trim and /\s/u use this set, which differs from Rust whitespace.
fn whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(whitespace)
}

pub(crate) fn parse(text: &str, attachments: &Value) -> Option<Command> {
    if attachments
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        return None;
    }
    let argument = text.trim_matches(whitespace).strip_prefix("/goal")?;
    if argument.chars().next().is_some_and(|c| !whitespace(c)) {
        return None;
    }
    let argument = argument.trim_matches(whitespace);
    Some(match argument.to_lowercase().as_str() {
        "" | "edit" => Command::Show,
        "clear" => Command::Clear,
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        _ => Command::Set(argument.into()),
    })
}

pub(crate) fn codex(goal: &Value) -> Result<Value, String> {
    if goal.is_null() {
        return Ok(Value::Null);
    }
    let objective = goal["objective"]
        .as_str()
        .ok_or("Codex goal has no objective")?
        .trim_matches(whitespace);
    if objective.is_empty() {
        return Ok(Value::Null);
    }
    let status = match goal["status"].as_str() {
        Some("active") => "active",
        Some("paused") => "paused",
        Some("blocked") => "blocked",
        Some("usageLimited") => "usage_limited",
        Some("budgetLimited") => "budget_limited",
        Some("complete") => "complete",
        _ => return Err("Codex returned an unknown goal status".into()),
    };
    let nonnegative = |key: &str| -> Result<Value, String> {
        if let Some(n) = goal[key].as_u64() {
            Ok(json!(n))
        } else if let Some(n) = goal[key].as_i64() {
            Ok(json!(n.max(0)))
        } else {
            goal[key]
                .as_f64()
                .map(|n| json!(n.max(0.0)))
                .ok_or_else(|| format!("Codex goal has invalid {key}"))
        }
    };
    Ok(
        json!({"objective":objective,"status":status,"tokensUsed":nonnegative("tokensUsed")?,
        "tokenBudget":goal["tokenBudget"],"timeUsedSeconds":nonnegative("timeUsedSeconds")?}),
    )
}

pub(crate) fn claude_text(message: &Value) -> String {
    if let Some(text) = message["message"]["content"].as_str() {
        return text.into();
    }
    message["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect()
}

pub(crate) fn claude(current: &Value, message: &Value) -> Option<Value> {
    if !message["parent_tool_use_id"].is_null() {
        return None;
    }
    let text = claude_text(message);
    if message["type"] == "assistant" && message["message"]["model"] == "<synthetic>" {
        let text = text.trim_matches(whitespace);
        if let Some(objective) = text.strip_prefix("Goal set: ") {
            let objective = objective.trim_matches(whitespace);
            return (!objective.is_empty())
                .then(|| json!({"objective":objective,"status":"active","checks":0}));
        }
        if text.starts_with("Goal cleared: ") || text.starts_with("No goal set") {
            return Some(Value::Null);
        }
        static ACTIVE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let captures = ACTIVE
            .get_or_init(|| {
                regex::Regex::new(
                    r"^Goal active: ([\s\S]+?) \((?:not yet evaluated|([0-9]+) turns?)\)",
                )
                .unwrap()
            })
            .captures(text)?;
        let objective = captures.get(1)?.as_str();
        let mut goal = if current["objective"] == objective {
            current.clone()
        } else {
            json!({})
        };
        goal["objective"] = json!(objective);
        goal["status"] = json!("active");
        goal["checks"] = json!(captures
            .get(2)
            .map(|n| n.as_str().parse::<u64>())
            .transpose()
            .ok()?
            .unwrap_or(0));
        return Some(goal);
    }
    if message["type"] != "user" || message["isSynthetic"] != true || current.is_null() {
        return None;
    }
    let prefix = format!(
        "Stop hook feedback:\n[{}]: ",
        current["objective"].as_str()?
    );
    let feedback = text.strip_prefix(&prefix)?;
    let mut goal = current.clone();
    goal["status"] = json!("active");
    goal["checks"] = json!(current["checks"].as_u64().unwrap_or(0).saturating_add(1));
    goal["lastCheck"] = json!(feedback.trim_matches(whitespace));
    Some(goal)
}

pub(crate) fn publish(
    db: &rusqlite::Connection,
    tid: &str,
    thread: &Value,
    native: &str,
    goal: Value,
    at: &str,
) -> Result<(), String> {
    let Some(id) = thread["activeProviderThreadId"].as_str() else {
        return Ok(());
    };
    let mut provider = crate::events::get(db, "providerThreads", id)?;
    // A retired conversation must never overwrite the replacement's goal.
    if provider["nativeThreadRef"]["nativeId"] != native || provider["goal"] == goal {
        return Ok(());
    }
    provider["goal"] = goal;
    provider["updatedAt"] = json!(at);
    crate::events::put(db, "providerThreads", id, tid, &provider)?;
    crate::events::append(db, tid, "provider-thread.updated", provider, thread)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_upstream_case_whitespace_and_attachment_rules() {
        for (text, expected) in [
            ("/goal", Some(Command::Show)),
            ("\u{feff}/goal\tEDIT\n", Some(Command::Show)),
            ("/goal Clear", Some(Command::Clear)),
            ("/goal\npause", Some(Command::Pause)),
            ("/goal RESUME", Some(Command::Resume)),
            (
                "/goal implement\nall",
                Some(Command::Set("implement\nall".into())),
            ),
            ("/GOAL x", None),
            ("/goals", None),
            ("/goal\u{85}x", None),
        ] {
            assert_eq!(parse(text, &json!([])), expected, "{text:?}");
        }
        assert_eq!(parse("/goal clear", &json!([{"id":"image"}])), None);
    }

    #[test]
    fn matches_executed_nightly_4262_goal_functions() {
        let cases: Value =
            serde_json::from_str(include_str!("../tests/fixtures/native-goals.json")).unwrap();
        for case in cases["commands"].as_array().unwrap() {
            let actual = match parse(
                case["input"]["text"].as_str().unwrap(),
                &case["input"]["attachments"],
            ) {
                None => Value::Null,
                Some(Command::Show) => json!({"type":"show"}),
                Some(Command::Clear) => json!({"type":"clear"}),
                Some(Command::Pause) => json!({"type":"pause"}),
                Some(Command::Resume) => json!({"type":"resume"}),
                Some(Command::Set(objective)) => json!({"type":"set","objective":objective}),
            };
            assert_eq!(actual, case["expected"], "{case}");
        }
        for case in cases["goals"].as_array().unwrap() {
            assert_eq!(codex(&case["input"]).unwrap(), case["expected"], "{case}");
        }
        for case in cases["claudeGoals"].as_array().unwrap() {
            let actual = claude(&case["current"], &case["message"]);
            assert_eq!(actual.is_some(), case["changed"] == true, "{case}");
            assert_eq!(actual.unwrap_or(Value::Null), case["expected"], "{case}");
        }
    }
}
