//! `server.upsertKeybinding` / `server.removeKeybinding` and the keybindings in `server.getConfig`.
mod common;
use common::*;
use serde_json::{json, Value};

fn rules(e: &Env) -> Value {
    serde_json::from_str(&std::fs::read_to_string(e.state.join("keybindings.json")).unwrap())
        .unwrap()
}
fn command_count(config: &Value, command: &str) -> usize {
    config["keybindings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|k| k["command"] == command)
        .count()
}

#[test]
fn custom_rules_override_defaults_and_persist() {
    let e = setup("keys");
    let c = &e.conn;
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert!(
        config["keybindings"].as_array().unwrap().len() > 60,
        "T3's defaults are served"
    );
    assert_eq!(config["issues"], json!([]));
    assert!(
        !e.state.join("keybindings.json").exists(),
        "reading never writes the file"
    );
    let sidebar = config["keybindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["command"] == "sidebar.toggle")
        .unwrap()
        .clone();
    assert_eq!(
        sidebar["shortcut"],
        json!({"key":"b","metaKey":false,"ctrlKey":false,"shiftKey":false,"altKey":false,"modKey":true})
    );

    // Upsert replaces the default of the same command and keeps the file to the user's rules only.
    let out = c
        .rpc(
            "server.upsertKeybinding",
            json!({"key":" Mod+Shift+K ","command":"sidebar.toggle","when":"!terminalFocus"}),
        )
        .unwrap();
    assert_eq!(command_count(&out, "sidebar.toggle"), 1);
    let mine = out["keybindings"].as_array().unwrap().last().unwrap();
    assert_eq!(
        (
            mine["command"].as_str(),
            mine["shortcut"]["key"].as_str(),
            mine["shortcut"]["shiftKey"].as_bool()
        ),
        (Some("sidebar.toggle"), Some("k"), Some(true))
    );
    assert_eq!(
        mine["whenAst"],
        json!({"type":"not","node":{"type":"identifier","name":"terminalFocus"}})
    );
    assert_eq!(
        rules(&e),
        json!([{"key":"Mod+Shift+K","command":"sidebar.toggle","when":"!terminalFocus"}])
    );
    assert_eq!(out["issues"], json!([]));

    // `replace` swaps one rule for another; scripts are bindable.
    c.rpc(
        "server.upsertKeybinding",
        json!({"key":"mod+9","command":"script.build.run"}),
    )
    .unwrap();
    c.rpc("server.upsertKeybinding", json!({"key":"mod+8","command":"script.build.run","replace":{"key":"mod+9","command":"script.build.run"}})).unwrap();
    assert_eq!(rules(&e).as_array().unwrap().len(), 2);
    assert_eq!(rules(&e)[1]["key"], "mod+8");
    // The same config comes back through getConfig.
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(command_count(&config, "script.build.run"), 1);

    // Remove deletes only the matching rule; the default returns.
    let out = c
        .rpc(
            "server.removeKeybinding",
            json!({"key":"Mod+Shift+K","command":"sidebar.toggle","when":"!terminalFocus"}),
        )
        .unwrap();
    assert_eq!(command_count(&out, "sidebar.toggle"), 1);
    assert_eq!(rules(&e).as_array().unwrap().len(), 1);

    // Invalid rules never reach the file.
    for bad in [
        json!({"key":"mod+x","command":"nope.nothing"}),
        json!({"key":"a+b","command":"chat.new"}),
        json!({"key":"mod+x","command":"chat.new","when":"a &&"}),
        json!({"key":"","command":"chat.new"}),
    ] {
        assert!(
            c.rpc("server.upsertKeybinding", bad.clone()).is_err(),
            "{bad}"
        );
    }
    assert_eq!(rules(&e).as_array().unwrap().len(), 1);
}

#[test]
fn a_malformed_file_is_reported_and_never_overwritten() {
    let e = setup("keys-bad");
    let c = &e.conn;
    std::fs::write(e.state.join("keybindings.json"), "{not json").unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(config["issues"][0]["kind"], "keybindings.malformed-config");
    assert!(
        config["keybindings"].as_array().unwrap().len() > 60,
        "defaults still apply"
    );
    assert!(c
        .rpc(
            "server.upsertKeybinding",
            json!({"key":"mod+x","command":"chat.new"})
        )
        .is_err());
    assert_eq!(
        std::fs::read_to_string(e.state.join("keybindings.json")).unwrap(),
        "{not json"
    );
    // Invalid entries are skipped individually.
    std::fs::write(
        e.state.join("keybindings.json"),
        r#"[{"key":"mod+y","command":"chat.new"},{"key":"x","command":"bogus"}]"#,
    )
    .unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(config["issues"][0]["kind"], "keybindings.invalid-entry");
    assert_eq!(config["issues"][0]["index"], 1);
    assert_eq!(command_count(&config, "chat.new"), 1);
}
