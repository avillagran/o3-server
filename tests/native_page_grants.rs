//! Captured grant policy only. Actual browser/actor transport acceptance is separate.
use o3_server::native_page_worker;
#[path = "../src/server_browser/native_grant.rs"]
mod grant;
#[path = "support/sandbox.rs"]
mod sandbox;
use grant::{Denied, State};
use native_page_worker::{cdp::Packet, JavaScriptJson};
fn json(raw: &str) -> JavaScriptJson {
    JavaScriptJson::parse(raw.into()).unwrap()
}
fn packet(raw: &str) -> Packet {
    Packet::decode(raw.as_bytes(), 65536).unwrap()
}
fn root() -> State {
    State::captured("owned-target".into()).unwrap()
}
fn attach(state: &mut State) {
    let request = state
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"owned-target","flatten":true}"#),
            None,
        )
        .unwrap();
    state.started(&request);
    state
        .completed(
            &request,
            &packet(r#"{"id":1,"result":{"sessionId":"owned-session"}}"#),
        )
        .unwrap();
}
#[test]
fn captured_target_and_root_policy_never_admit_another_thread_or_browser_global_mutation() {
    sandbox::assert_isolated();
    let mut state = root();
    for (method, params, session) in [
        (
            "Target.attachToTarget",
            r#"{"targetId":"other-thread","flatten":true}"#,
            None,
        ),
        (
            "Target.getTargetInfo",
            r#"{"targetId":"other-thread"}"#,
            None,
        ),
        ("Target.getTargets", "{}", None),
        ("Target.createTarget", r#"{"url":"about:blank"}"#, None),
        ("Browser.close", "{}", None),
        (
            "Browser.setDownloadBehavior",
            r#"{"downloadPath":"/foreign"}"#,
            None,
        ),
        (
            "Target.attachToTarget",
            r#"{"targetId":"owned-target","flatten":true,"browserContextId":"other"}"#,
            None,
        ),
        (
            "Runtime.evaluate",
            r#"{"expression":"1"}"#,
            Some("other-session"),
        ),
    ] {
        assert!(matches!(
            state.reserve(method, &json(params), session),
            Err(Denied::Scope)
        ));
    }
    state
        .reserve("Browser.getVersion", &json("{}"), None)
        .unwrap();
    attach(&mut state);
    for method in [
        "Network.getAllCookies",
        "Network.clearBrowserCache",
        "Network.setCookie",
        "Page.close",
        "Page.crash",
        "DOM.setFileInputFiles",
    ] {
        assert!(matches!(
            state.reserve(method, &json("{}"), Some("owned-session")),
            Err(Denied::Scope)
        ));
    }
    state
        .reserve(
            "Runtime.evaluate",
            &json(r#"{"expression":"'\ud800'"}"#),
            Some("owned-session"),
        )
        .unwrap();
}
#[test]
fn child_grants_require_an_owned_parent_and_refused_children_still_get_owned_cleanup() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    let foreign = packet(
        r#"{"method":"Target.attachedToTarget","sessionId":"foreign","params":{"sessionId":"unowned","targetInfo":{"targetId":"foreign-target","type":"iframe"}}}"#,
    );
    assert!(!state.event(&foreign).unwrap());
    let child = packet(
        r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"child","targetInfo":{"targetId":"child-target","type":"iframe","title":"\ud800"}}}"#,
    );
    assert!(state.event(&child).unwrap());
    state
        .reserve("Runtime.enable", &json("{}"), Some("child"))
        .unwrap();
    let refused = packet(
        r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"refused-child","targetInfo":{"targetId":"shared-worker-target","type":"service_worker"}}}"#,
    );
    assert!(!state.event(&refused).unwrap());
    assert_eq!(state.refused_sessions(), vec!["refused-child"]);
    assert!(matches!(
        state.reserve("Runtime.enable", &json("{}"), Some("refused-child")),
        Err(Denied::Scope)
    ));
    assert!(matches!(
        state.reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"shared-worker-target","flatten":true}"#),
            None
        ),
        Err(Denied::Scope)
    ));
    let detach = state.cleanup_detach("refused-child").unwrap();
    state
        .completed(&detach, &packet(r#"{"id":2,"result":{}}"#))
        .unwrap();
    assert!(state.refused_sessions().is_empty());
    let raw = r#"{"method":"Runtime.consoleAPICalled","sessionId":"child","params":{"args":[{"value":{"\ud800":"\udfff"}}]}}"#;
    let event = packet(raw);
    assert!(state.event(&event).unwrap());
    assert_eq!(serde_json::to_string(&event).unwrap(), raw);
    assert!(!state
        .event(&packet(
            r#"{"method":"Target.targetCreated","params":{"targetInfo":{"targetId":"other"}}}"#
        ))
        .unwrap());
    assert!(state
        .event(&packet(
            r#"{"method":"Target.detachedFromTarget","params":{"sessionId":"child"}}"#
        ))
        .unwrap());
    assert!(matches!(
        state.reserve("Runtime.enable", &json("{}"), Some("child")),
        Err(Denied::Scope)
    ));
}
#[test]
fn close_before_attach_reply_keeps_a_cleanup_obligation_and_unknown_is_sticky() {
    sandbox::assert_isolated();
    let mut state = root();
    let request = state
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"owned-target","flatten":true}"#),
            None,
        )
        .unwrap();
    state.started(&request);
    state.cancelled(&request);
    state.close();
    assert_eq!(state.attachments_pending(), 1);
    assert!(!state.clean());
    state
        .completed(
            &request,
            &packet(r#"{"id":1,"result":{"sessionId":"late"}}"#),
        )
        .unwrap();
    assert_eq!(state.detach_sessions(), vec!["late"]);
    assert!(!state.clean());
    let detach = state
        .reserve(
            "Target.detachFromTarget",
            &json(r#"{"sessionId":"late"}"#),
            None,
        )
        .unwrap();
    state
        .completed(&detach, &packet(r#"{"id":2,"result":{}}"#))
        .unwrap();
    assert!(state.clean());
    assert_eq!(state.receipt_totals()[0..2], [1, 1]);
    let mut unknown = root();
    let request = unknown
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"owned-target","flatten":true}"#),
            None,
        )
        .unwrap();
    unknown.started(&request);
    unknown.wire_unknown(&request);
    unknown.close();
    unknown
        .completed(
            &request,
            &packet(r#"{"id":1,"result":{"sessionId":"late"}}"#),
        )
        .unwrap();
    let detach = unknown.cleanup_detach("late").unwrap();
    unknown
        .completed(&detach, &packet(r#"{"id":2,"result":{}}"#))
        .unwrap();
    assert!(!unknown.clean());
    let mut unsent = root();
    let request = unsent
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"owned-target","flatten":true}"#),
            None,
        )
        .unwrap();
    unsent.cancelled(&request);
    unsent.close();
    assert!(unsent.clean());
}
#[test]
fn malformed_attachment_and_detach_failures_never_certify_cleanup() {
    sandbox::assert_isolated();
    let mut state = root();
    let request = state
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"owned-target","flatten":true}"#),
            None,
        )
        .unwrap();
    state.started(&request);
    assert_eq!(
        state.completed(&request, &packet(r#"{"id":1,"result":{}}"#)),
        Err(Denied::Protocol)
    );
    state.close();
    assert!(!state.clean());
    let mut failed = root();
    attach(&mut failed);
    failed.close();
    let detach = failed.cleanup_detach("owned-session").unwrap();
    failed
        .completed(
            &detach,
            &packet(r#"{"id":2,"error":{"code":-32000,"message":"refused"}}"#),
        )
        .unwrap();
    assert!(!failed.clean());
    assert!(matches!(
        failed.reserve("Runtime.enable", &json("{}"), Some("owned-session")),
        Err(Denied::Closed)
    ));
}
#[test]
fn window_metadata_is_captured_and_security_fields_cannot_be_ambiguous() {
    sandbox::assert_isolated();
    let mut state = root();
    let window = state
        .reserve(
            "Browser.getWindowForTarget",
            &json(r#"{"targetId":"owned-target"}"#),
            None,
        )
        .unwrap();
    state
        .completed(
            &window,
            &packet(r#"{"id":1,"result":{"windowId":17,"bounds":{}}}"#),
        )
        .unwrap();
    state
        .reserve("Browser.getWindowBounds", &json(r#"{"windowId":17}"#), None)
        .unwrap();
    assert!(matches!(
        state.reserve("Browser.getWindowBounds", &json(r#"{"windowId":18}"#), None),
        Err(Denied::Scope)
    ));
    assert!(matches!(
        state.reserve(
            "Target.getTargetInfo",
            &json(r#"{"targetId":"owned-target","targetId":"other"}"#),
            None
        ),
        Err(Denied::Protocol)
    ));
    state.mark_unknown();
    state.close();
    assert!(!state.clean());
}

#[test]
fn parent_detach_does_not_invent_a_child_cleanup_receipt() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    state.event(&packet(r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"child","targetInfo":{"targetId":"child-target","type":"iframe"}}}"#)).unwrap();
    state.close();
    let detach = state.cleanup_detach("owned-session").unwrap();
    state
        .completed(&detach, &packet(r#"{"id":2,"result":{}}"#))
        .unwrap();
    assert!(!state.clean());
    assert_eq!(state.detach_sessions(), vec!["child"]);
    assert!(!state.event(&packet(r#"{"method":"Target.detachedFromTarget","sessionId":"owned-session","params":{"sessionId":"child"}}"#)).unwrap());
    assert!(state.clean());
}

#[test]
fn explicit_child_attachment_keeps_its_parent_witness_after_a_late_reply() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    state.event(&packet(r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"child-witness","targetInfo":{"targetId":"child-target","type":"iframe"}}}"#)).unwrap();
    let request = state
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"child-target","flatten":true}"#),
            None,
        )
        .unwrap();
    state.started(&request);
    state
        .event(&packet(
            r#"{"method":"Target.detachedFromTarget","params":{"sessionId":"child-witness"}}"#,
        ))
        .unwrap();
    state
        .completed(
            &request,
            &packet(r#"{"id":2,"result":{"sessionId":"late-child"}}"#),
        )
        .unwrap();
    assert_eq!(state.refused_sessions(), vec!["late-child"]);
    assert!(matches!(
        state.reserve("Runtime.enable", &json("{}"), Some("late-child")),
        Err(Denied::Scope)
    ));
    let detach = state.cleanup_detach("late-child").unwrap();
    state
        .completed(&detach, &packet(r#"{"id":3,"result":{}}"#))
        .unwrap();
}

#[test]
fn close_drains_all_entered_commands_before_certifying_owned_cleanup() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    let operation = state
        .reserve(
            "Target.setAutoAttach",
            &json(r#"{"flatten":true,"autoAttach":true,"waitForDebuggerOnStart":true}"#),
            Some("owned-session"),
        )
        .unwrap();
    assert!(state.may_start(&operation, false));
    assert_eq!(state.requests_pending(), 1);
    state.started(&operation);
    state.cancelled(&operation);
    state.close();
    assert!(!state.may_start(&operation, false));
    let detach = state.cleanup_detach("owned-session").unwrap();
    state
        .completed(&detach, &packet(r#"{"id":2,"result":{}}"#))
        .unwrap();
    assert!(!state.clean());
    state
        .completed(&operation, &packet(r#"{"id":3,"result":{}}"#))
        .unwrap();
    assert!(state.clean());

    let mut unsent = root();
    let operation = unsent
        .reserve("Browser.getVersion", &json("{}"), None)
        .unwrap();
    unsent.cancelled(&operation);
    unsent.close();
    assert!(unsent.clean());

    let mut ambiguous = root();
    let operation = ambiguous
        .reserve("Browser.getVersion", &json("{}"), None)
        .unwrap();
    ambiguous.started(&operation);
    ambiguous.wire_unknown(&operation);
    ambiguous
        .completed(&operation, &packet(r#"{"id":4,"result":{}}"#))
        .unwrap();
    ambiguous.close();
    assert!(!ambiguous.clean());
}

#[test]
fn cleanup_distinguishes_protocol_parent_from_an_explicit_attach_lineage_witness() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    state.event(&packet(r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"auto-child","targetInfo":{"targetId":"child-target","type":"worker"}}}"#)).unwrap();
    assert_eq!(
        state
            .cleanup_detach("auto-child")
            .unwrap()
            .session
            .as_deref(),
        Some("owned-session")
    );
    let request = state
        .reserve(
            "Target.attachToTarget",
            &json(r#"{"targetId":"child-target","flatten":true}"#),
            None,
        )
        .unwrap();
    state
        .completed(
            &request,
            &packet(r#"{"id":2,"result":{"sessionId":"explicit-child"}}"#),
        )
        .unwrap();
    assert_eq!(
        state.cleanup_detach("explicit-child").unwrap().session,
        None
    );
    let sessions = state.detach_sessions();
    assert_eq!(sessions.last().unwrap(), "owned-session");
    assert!(matches!(
        state.reserve(
            "Target.detachFromTarget",
            &json(r#"{"sessionId":"auto-child"}"#),
            Some("explicit-child")
        ),
        Err(Denied::Scope)
    ));
    state
        .reserve(
            "Target.detachFromTarget",
            &json(r#"{"sessionId":"auto-child"}"#),
            Some("owned-session"),
        )
        .unwrap();
}

#[test]
fn root_event_identity_is_raw_and_missing_parent_detach_remains_owned() {
    sandbox::assert_isolated();
    let mut state = root();
    attach(&mut state);
    let own = packet(
        r#"{"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"owned-target","title":"\ud800"}}}"#,
    );
    assert!(state.related_root_event(&own).unwrap());
    assert!(state.event(&own).unwrap());
    assert!(!state.related_root_event(&packet(r#"{"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"foreign","title":"\ud800"}}}"#)).unwrap());
    state.event(&packet(r#"{"method":"Target.attachedToTarget","sessionId":"owned-session","params":{"sessionId":"child","targetInfo":{"targetId":"child-target","type":"worker"}}}"#)).unwrap();
    let detach = state.cleanup_detach("owned-session").unwrap();
    state
        .completed(&detach, &packet(r#"{"id":3,"result":{}}"#))
        .unwrap();
    let late_child = packet(
        r#"{"method":"Target.detachedFromTarget","sessionId":"owned-session","params":{"sessionId":"child"}}"#,
    );
    assert!(state.related_session_event(&late_child).unwrap());
    assert!(state.event(&late_child).unwrap());
    state.close();
    assert!(state.clean());
}
