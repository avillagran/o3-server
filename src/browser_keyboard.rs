//! Playwright 1.60 keyboard state and Chromium wire events.
//! Original dependency license/notice: docs/licenses/playwright-core-1.60.0/.
use serde_json::{json, Value};
use std::sync::LazyLock;
static DATA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("browser_keyboard/data.json")).expect("pinned keyboard data")
});
const MODIFIERS: &[&str] = &["Alt", "Control", "Meta", "Shift"];

#[derive(Default)]
pub(crate) struct Keyboard {
    modifiers: Vec<String>,
    keys: Vec<String>,
    pub browser_mac: Option<bool>,
}
fn add(set: &mut Vec<String>, value: &str) {
    if !set.iter().any(|key| key == value) {
        set.push(value.to_owned());
    }
}
fn remove(set: &mut Vec<String>, value: &str) {
    set.retain(|key| key != value);
}

/// Native splitting keeps a literal '+' token, including Control++.
pub(crate) fn steps(chord: &str) -> Vec<(bool, String)> {
    let mut tokens = Vec::new();
    let mut building = String::new();
    for ch in chord.chars() {
        if ch == '+' && !building.is_empty() {
            tokens.push(std::mem::take(&mut building));
        } else {
            building.push(ch);
        }
    }
    tokens.push(building);
    tokens
        .iter()
        .map(|token| (true, token.clone()))
        .chain(tokens.iter().rev().map(|token| (false, token.clone())))
        .collect()
}
pub(crate) fn browser_mac(user_agent: &str) -> bool {
    !user_agent.contains("Windows") && user_agent.contains("Macintosh")
}
impl Keyboard {
    fn has(&self, key: &str) -> bool {
        self.modifiers.iter().any(|value| value == key)
    }
    fn description(&self, key: &str, server_mac: bool) -> Result<Value, String> {
        let key = if key == "ControlOrMeta" {
            if server_mac {
                "Meta"
            } else {
                "Control"
            }
        } else {
            key
        };
        let base = DATA["layout"]
            .get(key)
            .ok_or_else(|| format!("Unknown key: \"{key}\""))?;
        let mut description = if self.has("Shift") && base["shifted"].is_object() {
            base["shifted"].clone()
        } else {
            base.clone()
        };
        if self.modifiers.len() > 1 || (!self.has("Shift") && self.modifiers.len() == 1) {
            description["text"] = json!("");
        }
        Ok(description)
    }
    fn commands(&self, code: &str) -> Vec<String> {
        if self.browser_mac != Some(true) {
            return Vec::new();
        }
        let mut parts = ["Shift", "Control", "Alt", "Meta"]
            .into_iter()
            .filter(|key| self.has(key))
            .collect::<Vec<_>>();
        parts.push(code);
        let command = &DATA["macCommands"][parts.join("+")];
        let values = match command {
            Value::String(value) => vec![value.as_str()],
            Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        values
            .into_iter()
            .filter(|value| !value.starts_with("insert"))
            .map(|value| value[..value.len() - 1].to_owned())
            .collect()
    }
    /// State changes precede transport, including a failed send, as in the
    /// original Keyboard.down/up. The caller must send each step before the next.
    pub(crate) fn event(
        &mut self,
        down: bool,
        key: &str,
        server_mac: bool,
    ) -> Result<Value, String> {
        let description = self.description(key, server_mac)?;
        let code = description["code"].as_str().unwrap();
        let key = description["key"].as_str().unwrap();
        let auto_repeat = self.keys.iter().any(|value| value == code);
        if down {
            add(&mut self.keys, code);
            if MODIFIERS.contains(&key) {
                add(&mut self.modifiers, key);
            }
        } else {
            if MODIFIERS.contains(&key) {
                remove(&mut self.modifiers, key);
            }
            remove(&mut self.keys, code);
        }
        let mask = [("Alt", 1), ("Control", 2), ("Meta", 4), ("Shift", 8)]
            .into_iter()
            .filter(|(key, _)| self.has(key))
            .fold(0, |mask, (_, bit)| mask | bit);
        let location = description["location"].as_u64().unwrap();
        let mut params = json!({"type":"keyUp","modifiers":mask,"key":key,"windowsVirtualKeyCode":description["keyCodeWithoutLocation"],"code":code,"location":location});
        if down {
            let text = description["text"].as_str().unwrap();
            params["type"] = json!(if text.is_empty() {
                "rawKeyDown"
            } else {
                "keyDown"
            });
            params["commands"] = json!(self.commands(code));
            params["text"] = json!(text);
            params["unmodifiedText"] = json!(text);
            params["autoRepeat"] = json!(auto_repeat);
            params["isKeypad"] = json!(location == 3);
        }
        Ok(params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_native_keyboard_classes_match_wire_events_and_failure_state() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/browser-keyboard-4314.json"))
                .unwrap();
        assert_eq!(fixture["sourceSha256"], DATA["sourceSha256"]);
        assert_eq!(fixture["cases"].as_array().unwrap().len(), 920);
        for case in fixture["cases"].as_array().unwrap() {
            let mut keyboard = Keyboard {
                browser_mac: case["browserMac"].as_bool(),
                ..Default::default()
            };
            for operation in case["operations"].as_array().unwrap() {
                let mut events = vec![];
                let mut error = None;
                for (down, key) in steps(operation["input"]["key"].as_str().unwrap()) {
                    match keyboard.event(down, &key, case["serverMac"].as_bool().unwrap()) {
                        Ok(params) => {
                            events.push(json!({"method":"Input.dispatchKeyEvent","params":params}));
                            if operation["input"]["failAt"].as_u64()
                                == Some((events.len() - 1) as u64)
                            {
                                error = Some("fixture-send-failed".to_owned());
                                break;
                            }
                        }
                        Err(message) => {
                            error = Some(message);
                            break;
                        }
                    }
                }
                assert_eq!(json!(error), operation["error"], "{case}");
                assert_eq!(json!(events), operation["events"], "{case}");
                assert_eq!(json!(keyboard.modifiers), operation["modifiers"], "{case}");
                assert_eq!(json!(keyboard.keys), operation["keys"], "{case}");
            }
        }
    }
    #[test]
    fn browser_platform_uses_native_user_agent_precedence() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/browser-keyboard-4314.json"))
                .unwrap();
        for case in fixture["platformCases"].as_array().unwrap() {
            assert_eq!(
                browser_mac(case["userAgent"].as_str().unwrap()),
                case["platform"] == "mac",
                "{case}"
            );
        }
    }
}
