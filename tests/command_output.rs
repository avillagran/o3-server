//! `orchestration.subscribeCommandOutput`, driven by a provider that reports a command.
mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use support::{start, subscribe, wait_for};

const T: Duration = Duration::from_secs(10);

#[test]
fn command_output_replaces_then_appends_then_settles() {
    let s = start(|state| {
        *state.provider.lock().unwrap() =
            Some(Arc::new(o3_server::command_output::ScriptedCommandProvider));
    });
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":"c1","threadId":"t1","projectId":"p1","title":"T","modelSelection":{"instanceId":"scripted","model":"m","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();

    // Before any command exists the subscription is refused.
    let missing = subscribe(
        c.clone(),
        "orchestration.subscribeCommandOutput",
        json!({"threadId":"t1","itemId":"cmd-1"}),
    );
    assert!(
        wait_for(T, || missing.ended().is_some()),
        "unknown item must fail the stream"
    );

    c.rpc("orchestration.dispatchCommand", json!({"type":"message.dispatch","commandId":"m1","messageId":"m1","threadId":"t1","text":"go","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let snap = |c: &t3_protocol::Connection| -> Value {
        c.request_json("GET", "/api/orchestration/threads/t1/bounded", None, true)
            .unwrap()
    };
    assert!(wait_for(T, || snap(&c)["projection"]["turnItems"]
        .as_array()
        .is_some_and(|a| a
            .iter()
            .any(|i| i["type"] == "command_execution"))));

    let mut out = subscribe(
        c.clone(),
        "orchestration.subscribeCommandOutput",
        json!({"threadId":"t1","itemId":"cmd-1"}),
    );
    let first = out.next(T);
    assert_eq!(
        first,
        json!({"kind":"replace","text":"hello\n","truncated":false,"running":true})
    );

    // The command is still running: its item says so, and the transcript has no output events for it.
    let item = snap(&c)["projection"]["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "command_execution")
        .cloned()
        .unwrap();
    assert_eq!(
        (item["status"].as_str(), item["input"].as_str()),
        (Some("running"), Some("echo hello; echo world"))
    );
    assert!(item.get("output").is_none());

    c.rpc("orchestration.dispatchCommand", json!({"type":"runtime-request.respond","commandId":"ok","threadId":"t1","requestId":"approval-1","decision":"accept"})).unwrap();
    let frames = out.until(T, |f| f["running"] == false);
    let mut text = first["text"].as_str().unwrap().to_owned();
    for f in &frames {
        match f["kind"].as_str().unwrap() {
            "replace" => text = f["text"].as_str().unwrap().to_owned(),
            _ => text.push_str(f["text"].as_str().unwrap()),
        }
    }
    assert_eq!(text, "hello\nworld\n");
    let mut reader_frames = vec![first.clone()];
    reader_frames.extend(frames.clone());
    let reader = t3_protocol::parity::output(&json!({
        "reference":{"threadId":"t1","itemId":"cmd-1"},
        "frames":reader_frames,"closed":true
    }))
    .unwrap();
    assert_eq!(reader["text"], "hello\nworld\n");
    assert_eq!(reader["running"], false);
    let last = frames.last().unwrap();
    assert_eq!(
        (
            last["kind"].as_str(),
            last["running"].as_bool(),
            last["truncated"].as_bool()
        ),
        (Some("replace"), Some(false), Some(false))
    );
    // The stream completes after its settled frame.
    assert!(wait_for(T, || out.ended().is_some()));
    let done = snap(&c)["projection"]["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "command_execution")
        .cloned()
        .unwrap();
    assert_eq!(
        (
            done["status"].as_str(),
            done["exitCode"].as_i64(),
            done["output"].as_str()
        ),
        (Some("completed"), Some(0), Some("hello\nworld\n"))
    );
    // A late subscriber gets the persisted output and ends.
    let mut late = subscribe(
        c.clone(),
        "orchestration.subscribeCommandOutput",
        json!({"threadId":"t1","itemId":"cmd-1"}),
    );
    assert_eq!(
        late.next(T),
        json!({"kind":"replace","text":"hello\nworld\n","truncated":false,"running":false})
    );
}
