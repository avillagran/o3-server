//! Native desktop browser message codecs and attachment/visibility state.
//! Descriptor I/O, CDP relay ownership and preview consumers are separate integration layers.
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq, Eq)]
pub struct TabKey {
    pub thread_id: String,
    pub tab_id: String,
}
impl TabKey {
    pub(crate) fn id(&self) -> String {
        format!("{}\0{}", self.thread_id, self.tab_id)
    }
}

fn string(object: &Map<String, Value>, name: &str, trim: bool) -> Option<Value> {
    let value = object.get(name)?.as_str()?;
    let value = if trim {
        crate::native_goal::trim(value)
    } else {
        value
    };
    if trim && value.is_empty() {
        return None;
    }
    Some(Value::String(value.into()))
}

fn decode(value: &Value, command: bool) -> Option<Value> {
    let object = value.as_object()?;
    let kind = object.get("type")?.as_str()?;
    let valid = if command {
        matches!(kind, "cdp" | "release" | "pointer" | "turn")
    } else {
        matches!(kind, "attached" | "detached" | "cdp" | "shown")
    };
    if !valid {
        return None;
    }
    let mut output = Map::new();
    output.insert("type".into(), Value::String(kind.into()));
    output.insert("threadId".into(), string(object, "threadId", true)?);
    if kind != "turn" {
        output.insert("tabId".into(), string(object, "tabId", true)?);
    }
    match kind {
        "cdp" => {
            output.insert("message".into(), string(object, "message", false)?);
        }
        "shown" | "turn" => {
            let name = if kind == "shown" { "shown" } else { "active" };
            output.insert(name.into(), Value::Bool(object.get(name)?.as_bool()?));
        }
        "pointer" => {
            let phase = object.get("phase")?.as_str()?;
            if !matches!(phase, "move" | "click") {
                return None;
            }
            output.insert("phase".into(), Value::String(phase.into()));
            for name in ["x", "y"] {
                let value = object.get(name)?;
                if !value.as_f64().is_some_and(f64::is_finite) {
                    return None;
                }
                output.insert(name.into(), value.clone());
            }
        }
        _ => {}
    }
    Some(Value::Object(output))
}

pub fn decode_event(value: &Value) -> Option<Value> {
    decode(value, false)
}
pub fn decode_command(value: &Value) -> Option<Value> {
    decode(value, true)
}
pub fn encode_command(value: &Value) -> Option<String> {
    decode_command(value).map(|value| format!("{value}\n"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StreamError {
    pub reason: &'static str,
    pub unterminated: bool,
}
pub struct Decoded {
    pub values: Vec<Value>,
    pub error: Option<StreamError>,
}

#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    max_json_bytes: Option<usize>,
    after_cr: bool,
    first_done: bool,
    failed: bool,
}
impl Decoder {
    /// Bound JSON payload bytes; delimiters are not part of the pending record.
    pub(crate) fn bounded(max_json_bytes: usize) -> Self {
        Self {
            max_json_bytes: Some(max_json_bytes),
            ..Self::default()
        }
    }
    fn line(&mut self, unterminated: bool, output: &mut Decoded) {
        let line = std::mem::take(&mut self.pending);
        let bytes = if !self.first_done && line.starts_with(&[239, 187, 191]) {
            &line[3..]
        } else {
            &line
        };
        self.first_done = true;
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                self.fail("InvalidUtf8", unterminated, output);
                return;
            }
        };
        if text.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            return;
        }
        match serde_json::from_str(text) {
            Ok(value) => output.values.push(value),
            Err(_) => self.fail("InvalidJson", unterminated, output),
        }
    }
    fn fail(&mut self, reason: &'static str, unterminated: bool, output: &mut Decoded) {
        self.failed = true;
        output.error = Some(StreamError {
            reason,
            unterminated,
        });
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Decoded {
        let mut output = Decoded {
            values: Vec::new(),
            error: None,
        };
        if self.failed {
            return output;
        }
        for byte in bytes {
            if matches!(byte, b'\r' | b'\n') {
                if *byte != b'\n' || !self.after_cr {
                    self.line(false, &mut output);
                }
                self.after_cr = *byte == b'\r';
                if self.failed {
                    break;
                }
            } else {
                self.after_cr = false;
                if self
                    .max_json_bytes
                    .is_some_and(|limit| self.pending.len() >= limit)
                {
                    self.pending = Vec::new();
                    self.fail("FrameTooLarge", false, &mut output);
                    break;
                }
                self.pending.push(*byte);
            }
        }
        output
    }
    pub fn finish(&mut self) -> Decoded {
        let mut output = Decoded {
            values: Vec::new(),
            error: None,
        };
        if !self.failed && !self.pending.is_empty() {
            self.line(true, &mut output);
        }
        output
    }
}

pub struct CdpFrame {
    pub key: TabKey,
    pub message: String,
}

pub struct Kernel {
    available: bool,
    attached: HashSet<String>,
    shown: HashMap<String, bool>,
}
impl Kernel {
    pub fn new(input_present: bool, control_present: bool) -> Self {
        Self {
            available: input_present && control_present,
            attached: HashSet::new(),
            shown: HashMap::new(),
        }
    }
    pub fn available(&self) -> bool {
        self.available
    }
    pub fn is_attached(&self, key: &TabKey) -> bool {
        self.available && self.attached.contains(&key.id())
    }
    pub fn is_shown(&self, key: &TabKey) -> bool {
        self.available && self.shown.get(&key.id()).copied().unwrap_or(true)
    }
    /// The runtime dispatches a returned frame only to an active endpoint queue.
    pub fn accept(&mut self, value: &Value) -> Option<CdpFrame> {
        if !self.available {
            return None;
        }
        let event = decode_event(value)?;
        let key = TabKey {
            thread_id: event["threadId"].as_str()?.into(),
            tab_id: event["tabId"].as_str()?.into(),
        };
        let id = key.id();
        match event["type"].as_str()? {
            "attached" => {
                self.attached.insert(id);
            }
            "detached" => {
                self.attached.remove(&id);
                self.shown.remove(&id);
            }
            "shown" => {
                self.shown.insert(id, event["shown"].as_bool()?);
            }
            "cdp" => {
                return Some(CdpFrame {
                    key,
                    message: event["message"].as_str()?.into(),
                })
            }
            _ => unreachable!("validated desktop event"),
        }
        None
    }
    /// Native stream shutdown clears visibility reports while retaining attachment knowledge.
    pub fn stream_closed(&mut self) {
        self.shown.clear();
    }
    pub fn command(&self, value: &Value) -> Option<String> {
        if self.available {
            encode_command(value)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/desktop-browser-4288.json")).unwrap()
    }
    #[test]
    fn native_browser_schema_oracle() {
        for case in fixture()["schemaCases"].as_array().unwrap() {
            let result = if case["kind"] == "event" {
                decode_event(&case["input"])
            } else {
                decode_command(&case["input"])
            };
            assert_eq!(result.unwrap_or(Value::Null), case["expected"], "{case}");
        }
    }
    #[test]
    fn native_ndjson_byte_stream_oracle() {
        for case in fixture()["transportCases"].as_array().unwrap() {
            let mut decoder = Decoder::default();
            let mut values = Vec::new();
            let mut error = None;
            for chunk in case["chunks"].as_array().unwrap() {
                let bytes: Vec<u8> = serde_json::from_value(chunk.clone()).unwrap();
                let batch = decoder.feed(&bytes);
                values.extend(batch.values);
                error = batch.error;
                if error.is_some() {
                    break;
                }
            }
            if error.is_none() {
                let batch = decoder.finish();
                values.extend(batch.values);
                error = batch.error;
            }
            let mut actual = json!({"values":values});
            if let Some(error) = error {
                actual["error"] = json!(error.reason);
                actual["unterminated"] = json!(error.unterminated);
            }
            assert_eq!(actual, case["expected"], "{case}");
        }
    }
    #[test]
    fn full_native_channel_state_and_command_oracle() {
        let key = TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        };
        let other = TabKey {
            thread_id: "other".into(),
            tab_id: "tab".into(),
        };
        for case in fixture()["stateCases"].as_array().unwrap() {
            let config = &case["config"];
            let mut kernel = Kernel::new(
                config.get("desktopBrowserFd").is_some(),
                config.get("desktopBrowserControlFd").is_some(),
            );
            let snapshot = |kernel: &Kernel| json!({"attached":kernel.is_attached(&key),"shown":kernel.is_shown(&key),"otherAttached":kernel.is_attached(&other),"otherShown":kernel.is_shown(&other)});
            let mut snapshots = vec![snapshot(&kernel)];
            if kernel.available() {
                for event in case["events"].as_array().unwrap() {
                    kernel.accept(event);
                    snapshots.push(snapshot(&kernel));
                }
            }
            assert_eq!(json!(snapshots), case["expected"]["snapshots"], "{case}");
            let mut writes = Vec::new();
            for command in [
                json!({"type":"turn","threadId":"thread","active":true}),
                json!({"type":"pointer","threadId":"thread","tabId":"tab","phase":"move","x":-1.5,"y":2.5}),
                json!({"type":"turn","threadId":"thread","active":false}),
            ] {
                if let Some(line) = kernel.command(&command) {
                    writes.push(json!({"fd":42,"line":line}));
                }
            }
            assert_eq!(json!(writes), case["expected"]["writes"], "{case}");
            kernel.stream_closed();
            assert_eq!(
                json!({"attached":kernel.is_attached(&key),"shown":kernel.is_shown(&key),"otherShown":kernel.is_shown(&other)}),
                case["expected"]["afterClose"],
                "{case}"
            );
        }
    }
    #[test]
    fn configured_json_byte_bound_preserves_default_native_decoding() {
        crate::test_sandbox::assert_isolated();
        let text = json!({"message":"界🙂"}).to_string();
        let wire = format!("{text}\n");
        let mut bounded = Decoder::bounded(text.len());
        assert_eq!(bounded.feed(wire.as_bytes()).values.len(), 1);
        let mut small = Decoder::bounded(text.len() - 1);
        let failed = small.feed(wire.as_bytes());
        assert_eq!(failed.error.unwrap().reason, "FrameTooLarge");
        assert!(small.feed(b"{}\n").values.is_empty());
        assert!(small.finish().values.is_empty());
        assert_eq!(Decoder::default().feed(wire.as_bytes()).values.len(), 1);
    }
}
