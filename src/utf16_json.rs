//! Structured JSON text for JavaScript strings that may contain lone UTF-16 units.
use serde_json::Value;

pub(crate) enum JsonText {
    Value(Value),
    Utf16(Vec<u16>),
    Object(Vec<(String, JsonText)>),
    Array(Vec<JsonText>),
}

impl JsonText {
    pub(crate) fn stringify(&self) -> String {
        let mut output = String::new();
        self.write(&mut output);
        output
    }

    fn write(&self, output: &mut String) {
        match self {
            Self::Value(value) => output.push_str(&value.to_string()),
            Self::Utf16(units) => quote(units, output),
            Self::Object(fields) => {
                output.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    quote(&key.encode_utf16().collect::<Vec<_>>(), output);
                    output.push(':');
                    value.write(output);
                }
                output.push('}');
            }
            Self::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    value.write(output);
                }
                output.push(']');
            }
        }
    }
}

fn quote(units: &[u16], output: &mut String) {
    use std::fmt::Write;
    output.push('"');
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Err(unit) => {
                write!(output, "\\u{:04x}", unit.unpaired_surrogate()).unwrap();
            }
            Ok('"') => output.push_str("\\\""),
            Ok('\\') => output.push_str("\\\\"),
            Ok('\u{08}') => output.push_str("\\b"),
            Ok('\u{0c}') => output.push_str("\\f"),
            Ok('\n') => output.push_str("\\n"),
            Ok('\r') => output.push_str("\\r"),
            Ok('\t') => output.push_str("\\t"),
            Ok(ch) if ch < '\u{20}' => {
                write!(output, "\\u{:04x}", ch as u32).unwrap();
            }
            Ok(ch) => output.push(ch),
        }
    }
    output.push('"');
}
