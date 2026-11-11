mod support;
use serde_json::json;
use t3_protocol::api::orchestration;

#[test]
fn native_fork_does_not_apply_fallback_attachment_budget() {
    let s = support::start(|_| {});
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Native fork","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.providerInstanceId','codex','$.codexNativeThreadId','native-proof') WHERE kind='thread' AND id='t'", []).unwrap();
        let run = json!({"id":"r","threadId":"t","ordinal":1,"status":"completed"});
        db.execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES ('runs','r','t',?)",
            [run.to_string()],
        )
        .unwrap();
        let media: Vec<_> = (0..33)
            .map(|n| json!({"id":format!("a{n}"),"type":"image","name":"proof.png"}))
            .collect();
        db.execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES ('messages','m','t',?)",
            [
                json!({"id":"m","runId":"r","role":"user","text":"proof","attachments":media})
                    .to_string(),
            ],
        )
        .unwrap();
    }
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"t","targetThreadId":"fork","sourcePoint":{"type":"run","runId":"r"}})).unwrap();
    let p = orchestration::thread_projection(&c, "fork").unwrap();
    assert_eq!(p["thread"]["nativeForkSource"], "native-proof");
    // An older boundary cannot use the latest native session.
    s.state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES ('runs','r2','t',?)",
            [json!({"id":"r2","threadId":"t","ordinal":2,"status":"completed"}).to_string()],
        )
        .unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.fork","commandId":"historical","sourceThreadId":"t","targetThreadId":"historical","sourcePoint":{"type":"run","runId":"r"}})).unwrap();
    let p = orchestration::thread_projection(&c, "historical").unwrap();
    let media = p["thread"]["inheritedProviderAttachments"]
        .as_array()
        .unwrap();
    assert_eq!(media.len(), 32);
    assert_eq!(media[0]["id"], "a1");
    assert_eq!(media[31]["id"], "a32");
    assert!(p["thread"]["inheritedProviderContext"]
        .as_str()
        .unwrap()
        .contains("attachments truncated"));
}
