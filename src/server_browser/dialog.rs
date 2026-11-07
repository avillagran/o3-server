//! Native pending-dialog identity and handled-before-send semantics.
use serde_json::{json, Value};
#[derive(Clone)]
struct Pending {
    token: u64,
    kind: String,
    message: Vec<u16>,
    default_value: Vec<u16>,
    handled: bool,
}
#[derive(Clone, Default)]
pub(crate) struct State {
    next_token: u64,
    pending: Option<Pending>,
    pub broadcast_revision: u64,
}
impl State {
    #[cfg(test)]
    pub fn open(&mut self, kind: &str, message: &str, default_value: &str) {
        self.open_units(
            kind,
            &message.encode_utf16().collect::<Vec<_>>(),
            &default_value.encode_utf16().collect::<Vec<_>>(),
        );
    }
    pub fn open_units(&mut self, kind: &str, message: &[u16], default_value: &[u16]) {
        self.next_token = self.next_token.wrapping_add(1);
        self.pending = Some(Pending {
            token: self.next_token,
            kind: kind.into(),
            message: message.iter().copied().take(4000).collect(),
            default_value: default_value.iter().copied().take(4000).collect(),
            handled: false,
        });
        self.broadcast_revision = self.broadcast_revision.wrapping_add(1);
    }
    pub fn external_close(&mut self) {
        // The native desktop closure callback clears status without broadcasting control.
        self.pending = None;
    }
    pub fn status(&self) -> Value {
        self.pending.as_ref().map_or(Value::Null, |pending| {
            json!({
                "type":pending.kind,
                "message":String::from_utf16_lossy(&pending.message),
                "defaultValue":String::from_utf16_lossy(&pending.default_value)
            })
        })
    }
    pub(crate) fn json_text(&self) -> crate::utf16_json::JsonText {
        use crate::utf16_json::JsonText;
        self.pending
            .as_ref()
            .map_or(JsonText::Value(Value::Null), |pending| {
                JsonText::Object(vec![
                    ("type".into(), JsonText::Value(json!(pending.kind))),
                    ("message".into(), JsonText::Utf16(pending.message.clone())),
                    (
                        "defaultValue".into(),
                        JsonText::Utf16(pending.default_value.clone()),
                    ),
                ])
            })
    }
    pub fn begin(
        &mut self,
        accept: bool,
        prompt: Option<&str>,
    ) -> Result<(u64, Value), &'static str> {
        let pending = self.pending.as_mut().ok_or("No dialog is pending.")?;
        if pending.handled {
            return Err(if accept {
                "Cannot accept dialog which is already handled!"
            } else {
                "Cannot dismiss dialog which is already handled!"
            });
        }
        pending.handled = true;
        let mut params = json!({"accept":accept});
        if let Some(prompt) = prompt.filter(|_| accept) {
            params["promptText"] = json!(prompt);
        }
        Ok((pending.token, params))
    }
    pub fn finish(&mut self, token: u64) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.token == token)
        {
            self.pending = None;
        }
        self.broadcast_revision = self.broadcast_revision.wrapping_add(1);
    }
}

#[derive(Debug)]
pub(crate) struct Input {
    pub tab: Option<String>,
    pub accept: bool,
    pub prompt: Option<String>,
}
impl Input {
    pub fn decode(value: &Value) -> Result<Self, &'static str> {
        let object = value.as_object().ok_or("Expected dialog input object")?;
        let accept = object
            .get("accept")
            .and_then(Value::as_bool)
            .ok_or("Expected boolean accept")?;
        let prompt = object
            .get("promptText")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("Expected string promptText")
            })
            .transpose()?;
        let tab=object.get("tabId").map(|value|{
            let tab=value.as_str().ok_or("Expected string tabId")?.trim_matches(|ch|matches!(ch,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'));
            if tab.is_empty() || tab.encode_utf16().count()>128 { return Err("Invalid tabId"); }
            Ok(tab.to_owned())
        }).transpose()?;
        Ok(Self {
            tab,
            accept,
            prompt,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn units(state: &State) -> Value {
        state.pending.as_ref().map_or(Value::Null, |pending| json!({"type":pending.kind,"messageUnits":pending.message,"defaultValueUnits":pending.default_value}))
    }
    #[test]
    fn actual_native_dialog_status_answer_and_broadcast_revisions() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/native-preview-dialog-4288.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 125);
        for case in cases {
            let input = &case["input"];
            let expected = &case["expected"];
            let mut state = State::default();
            if input["absent"] != true && input["frameExists"] != false {
                state.open(
                    input["type"].as_str().unwrap(),
                    input["message"].as_str().unwrap(),
                    input["defaultValue"].as_str().unwrap(),
                );
            }
            if input["closeOnly"] == true {
                state.external_close();
            }
            assert_eq!(units(&state), expected["before"], "before {input}");
            let mut commands = Vec::new();
            let mut outcomes = Vec::new();
            let mut replaced = false;
            for _ in 0..if input["repeat"] == true { 2 } else { 1 } {
                match state.begin(
                    input["accept"].as_bool().unwrap(),
                    input["promptText"].as_str(),
                ) {
                    Err(message) => outcomes.push(json!({"ok":false,"message":message})),
                    Ok((token, params)) => {
                        commands
                            .push(json!({"method":"Page.handleJavaScriptDialog","params":params}));
                        if input["mode"] == "fail" {
                            outcomes.push(
                                json!({"ok":false,"message":"Synthetic dialog command rejected"}),
                            );
                            continue;
                        }
                        if input["mode"] == "replace" && !replaced {
                            state.open("confirm", "Replacement 1", "replacement");
                            replaced = true;
                        }
                        if input["mode"] == "externalClose" {
                            state.external_close();
                        }
                        state.finish(token);
                        outcomes.push(json!({"ok":true}));
                    }
                }
            }
            assert_eq!(units(&state), expected["after"], "after {input}");
            assert_eq!(json!(commands), expected["commands"], "commands {input}");
            assert_eq!(json!(outcomes), expected["outcomes"], "outcomes {input}");
            assert_eq!(
                state.broadcast_revision as usize * 2,
                expected["controls"].as_array().unwrap().len(),
                "broadcast revisions {input}"
            );
        }
    }
    #[test]
    fn actual_native_dialog_input_outcomes_and_decoded_fields() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/native-preview-dialog-4288.json"
        ))
        .unwrap();
        let cases = fixture["schemaCases"].as_array().unwrap();
        assert_eq!(cases.len(), 13);
        for case in cases {
            let decoded = Input::decode(&case["input"]);
            assert_eq!(
                decoded.is_ok(),
                case["expected"]["ok"].as_bool().unwrap(),
                "{}",
                case["input"]
            );
            if let Ok(input) = decoded {
                let mut actual = json!({"accept":input.accept});
                if let Some(tab) = input.tab {
                    actual["tabId"] = json!(tab);
                }
                if let Some(prompt) = input.prompt {
                    actual["promptText"] = json!(prompt);
                }
                assert_eq!(actual, case["expected"]["value"]);
            }
        }
    }
}
