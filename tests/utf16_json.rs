#[path = "../src/utf16_json.rs"]
mod utf16_json;
use serde_json::{json, Value};
use utf16_json::JsonText;
#[test]
fn native_stringify_and_dialog_clipping() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/native-utf16-json-4301.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let units: Vec<u16> = case["units"]
            .as_array()
            .unwrap()
            .iter()
            .take(4000)
            .map(|v| v.as_u64().unwrap() as u16)
            .collect();
        assert_eq!(json!(units), case["clipped"]);
        let quoted = JsonText::Utf16(units.clone()).stringify();
        assert_eq!(quoted, case["quoted"].as_str().unwrap(), "{}", case["name"]);
        let text = JsonText::Object(vec![
            (
                "dialog".into(),
                JsonText::Object(vec![
                    ("type".into(), JsonText::Value(json!("prompt"))),
                    ("message".into(), JsonText::Utf16(units.clone())),
                    ("defaultValue".into(), JsonText::Utf16(units)),
                ]),
            ),
            (
                "tabs".into(),
                JsonText::Array(vec![
                    JsonText::Value(Value::Null),
                    JsonText::Value(json!(true)),
                ]),
            ),
        ])
        .stringify();
        assert_eq!(text, case["structured"].as_str().unwrap());
        let outer = json!({"content":[{"type":"text","text":text}]});
        let roundtrip: Value = serde_json::from_str(&outer.to_string()).unwrap();
        assert_eq!(roundtrip["content"][0]["text"], case["structured"]);
    }
}
