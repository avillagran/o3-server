#[path = "../src/cdp_dialog_json.rs"]
mod decoder;

#[test]
fn actual_javascript_wire_preserves_dialog_units_and_duplicate_key_semantics() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/dialog-wire-utf16.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let packet = decoder::decode(row["raw"].as_str().unwrap().as_bytes()).unwrap();
        let text = packet.dialog.unwrap();
        let expected = |key: &str| {
            row[key].as_array().map(|items| {
                items
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u16)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(text.message, expected("message"), "{}", row["raw"]);
        assert_eq!(text.default_prompt, expected("defaultPrompt"));
        let compatibility: serde_json::Value =
            serde_json::from_str(row["compat"].as_str().unwrap()).unwrap();
        assert_eq!(packet.value, compatibility);
        assert_eq!(
            packet.value.to_string(),
            row["compat"].as_str().unwrap(),
            "native insertion order"
        );
    }
}

#[test]
fn other_events_keep_strict_scalar_rejection_and_invalid_json_never_becomes_a_dialog() {
    for raw in [
        r#"{"method":"Runtime.other","params":{"message":"\ud800"}}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":{"message":"valid","unknown":"\ud800"}}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":{"message":"\q"}}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":{"message":"\u12"}}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":{"message":"\ud800"}} trailing"#,
    ] {
        assert!(decoder::decode(raw.as_bytes()).is_err(), "{raw}");
    }
    for raw in [
        "[]",
        "null",
        r#"{"id":1,"result":{"value":"😀"}}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":null}"#,
        r#"{"method":"Page.javascriptDialogOpening","params":{"message":"ignored"},"method":"Other"}"#,
    ] {
        let packet = decoder::decode(raw.as_bytes()).unwrap();
        assert!(packet.dialog.is_none());
        assert_eq!(
            packet.value,
            serde_json::from_str::<serde_json::Value>(raw).unwrap()
        );
    }
}
