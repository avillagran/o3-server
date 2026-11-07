//! Preserve JavaScript UTF-16 dialog strings before serde's scalar conversion.
//! Other CDP values deliberately keep the existing strict serde boundary.
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{value::RawValue, Map, Value};

struct RawObject(Vec<(String, Box<RawValue>)>);
impl<'de> Deserialize<'de> for RawObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = RawObject;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("CDP object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut entries: Vec<(String, Box<RawValue>)> = Vec::new();
                let mut positions = std::collections::HashMap::new();
                while let Some((key, raw)) = map.next_entry::<String, Box<RawValue>>()? {
                    if let Some(&index) = positions.get(&key) {
                        entries[index] = (key, raw);
                    } else {
                        positions.insert(key.clone(), entries.len());
                        entries.push((key, raw));
                    }
                }
                Ok(RawObject(entries))
            }
        }
        deserializer.deserialize_map(Object)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct DialogText {
    pub message: Option<Vec<u16>>,
    pub default_prompt: Option<Vec<u16>>,
}

#[derive(Debug)]
pub(crate) struct Packet {
    pub value: Value,
    pub dialog: Option<DialogText>,
}

fn invalid() -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom("Invalid CDP dialog string")
}

/// JSON.parse strings are sequences of UTF-16 units, including lone surrogates.
/// RawValue validates JSON grammar before this decoder sees a string token.
fn units(raw: &str) -> Result<Vec<u16>, serde_json::Error> {
    let body = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(invalid)?;
    let mut chars = body.chars();
    let mut output = Vec::new();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            if ch == '"' || ch < ' ' {
                return Err(invalid());
            }
            output.extend(ch.encode_utf16(&mut [0; 2]).iter().copied());
            continue;
        }
        let next = chars.next().ok_or_else(invalid)?;
        match next {
            '"' | '\\' | '/' => output.push(next as u16),
            'b' => output.push(8),
            'f' => output.push(12),
            'n' => output.push(10),
            'r' => output.push(13),
            't' => output.push(9),
            'u' => {
                let mut unit = 0u16;
                for _ in 0..4 {
                    let digit = chars
                        .next()
                        .and_then(|c| c.to_digit(16))
                        .ok_or_else(invalid)?;
                    unit = (unit << 4) | digit as u16;
                }
                output.push(unit);
            }
            _ => return Err(invalid()),
        }
    }
    Ok(output)
}

/// The lossy Value is only the compatibility view; the sidecar is authoritative
/// for dialog text. Callers must carry it with this exact event, never look up a
/// later dialog by session or substitute a reserved field into untrusted JSON.
pub(crate) fn decode(raw: &[u8]) -> Result<Packet, serde_json::Error> {
    if let Ok(value) = serde_json::from_slice::<Value>(raw) {
        if value["method"] != "Page.javascriptDialogOpening" || !value["params"].is_object() {
            return Ok(Packet {
                value,
                dialog: None,
            });
        }
    }
    let envelope: RawObject = serde_json::from_slice(raw)?;
    if !envelope
        .0
        .iter()
        .rev()
        .find(|(key, _)| key == "method")
        .is_some_and(|(_, m)| {
            serde_json::from_str::<String>(m.get())
                .is_ok_and(|m| m == "Page.javascriptDialogOpening")
        })
    {
        return Ok(Packet {
            value: serde_json::from_slice(raw)?,
            dialog: None,
        });
    }
    let mut value = Map::new();
    let mut dialog = DialogText {
        message: None,
        default_prompt: None,
    };
    for (key, raw) in envelope.0 {
        if key != "params" {
            value.insert(key, serde_json::from_str(raw.get())?);
            continue;
        }
        dialog = DialogText {
            message: None,
            default_prompt: None,
        };
        let params: RawObject = serde_json::from_str(raw.get())?;
        let mut converted = Map::new();
        for (key, raw) in params.0 {
            if matches!(key.as_str(), "message" | "defaultPrompt") && raw.get().starts_with('"') {
                let text = units(raw.get())?;
                converted.insert(key.clone(), Value::String(String::from_utf16_lossy(&text)));
                if key == "message" {
                    dialog.message = Some(text);
                } else {
                    dialog.default_prompt = Some(text);
                }
            } else {
                if key == "message" {
                    dialog.message = None;
                }
                if key == "defaultPrompt" {
                    dialog.default_prompt = None;
                }
                converted.insert(key, serde_json::from_str(raw.get())?);
            }
        }
        value.insert(key, Value::Object(converted));
    }
    Ok(Packet {
        value: Value::Object(value),
        dialog: Some(dialog),
    })
}
