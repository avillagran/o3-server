mod support;
use serde_json::json;

#[test]
fn codex_generation_setting_saves_but_explicit_generation_explains_limit() {
    let s = support::start(|_| {});
    let c = s.connect();
    t3_protocol::api::server::update_settings(
        &c,
        &json!({"textGenerationModelSelection":{"instanceId":"codex","model":"test"}}),
        None,
    )
    .unwrap();
    let settings = t3_protocol::api::server::settings(&c).unwrap();
    assert_eq!(
        settings["textGenerationModelSelection"]["instanceId"],
        "codex"
    );
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Title","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"msg","threadId":"t","messageId":"m","text":"Title proof","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    let err = c.rpc("orchestration.dispatchCommand", json!({"type":"thread.metadata.update","commandId":"regen","threadId":"t","regenerateTitle":true})).unwrap_err();
    assert!(err.to_string().contains("Select a Claude"), "{err}");
}
