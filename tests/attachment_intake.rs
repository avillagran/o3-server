mod common;
mod support;
use serde_json::{json, Value};
struct Mcp<'a> {
    s: &'a support::Server,
    token: String,
    session: String,
}
impl<'a> Mcp<'a> {
    fn new(s: &'a support::Server, access: &str) -> Self {
        let token = s
            .state
            .issue_mcp_session(access, 120_000, "launch test")
            .unwrap();
        let auth = format!("Bearer {token}");
        let (code,headers,body)=common::http(&s.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json")],json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"launch-test","version":"1"}}}).to_string().as_bytes());
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
        let session = headers
            .lines()
            .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
            .unwrap()
            .into();
        Self { s, token, session }
    }
    fn reply(&self, name: &str, p: Value, mode: Option<&str>) -> Value {
        let auth = format!("Bearer {}", self.token);
        let mut headers = vec![
            ("authorization", auth.as_str()),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-session-id", self.session.as_str()),
        ];
        if let Some(mode) = mode {
            headers.push(("t3-mode-limit", mode));
        }
        let (code,_,body)=common::http(&self.s.base,"POST","/mcp",&headers,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":p}}).to_string().as_bytes());
        assert_eq!(code, 200);
        serde_json::from_slice(&body).unwrap()
    }
    fn ok(&self, name: &str, p: Value) -> Value {
        let v = self.reply(name, p, None);
        assert_eq!(v["result"]["isError"], false, "{v}");
        v["result"]["structuredContent"].clone()
    }
    fn denied(&self, name: &str, p: Value, mode: Option<&str>, code: &str) {
        let v = self.reply(name, p, mode);
        if code == "invalid_request" && v.get("error").is_some() {
            assert_eq!(v["error"]["code"], -32602, "{v}");
            return;
        }
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["structuredContent"]["code"], code, "{v}");
    }
}
fn create(s: &support::Server, tid: &str) {
    s.connect().rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("create-{tid}"),"threadId":tid,"projectId":"p","title":"Attachments","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
}
fn projection(s: &support::Server, tid: &str) -> Value {
    s.connect()
        .rpc("orchestration.getThreadProjection", json!({"threadId":tid}))
        .unwrap()
}
fn prepare(m: &Mcp<'_>, name: &str, bytes: &[u8]) -> Value {
    let r=m.ok("t3_attachment_prepare_upload",json!({"upload":{"type":"file","name":name,"mimeType":"application/octet-stream","sizeBytes":bytes.len(),"_assetSession":"forged"}}));
    let (code, _, body) = common::http(
        &m.s.base,
        "POST",
        r["relativeUrl"].as_str().unwrap(),
        &[],
        bytes,
    );
    assert_eq!(code, 204, "{}", String::from_utf8_lossy(&body));
    json!({"type":"file","id":r["attachmentId"],"name":name,"mimeType":"application/octet-stream","sizeBytes":bytes.len()})
}
#[test]
fn mcp_upload_send_discard_ownership_and_canonical_references() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(std::sync::Arc::new(o3_server::provider::ScriptedProvider))
    });
    create(&s, "a");
    create(&s, "b");
    let m = Mcp::new(&s, "auto");
    let other = Mcp::new(&s, "auto");
    let read = Mcp::new(&s, "read-only");
    read.denied(
        "t3_attachment_prepare_upload",
        json!({"upload":{"name":"a.png","mimeType":"image/png","sizeBytes":8}}),
        None,
        "capability_denied",
    );
    for upload in [
        json!({"name":"a.png","mimeType":"text/plain","sizeBytes":8}),
        json!({"name":"a.png","mimeType":"image/png","sizeBytes":10485761}),
        json!({"type":"file","name":"a.txt","mimeType":"text/plain","sizeBytes":52428801}),
    ] {
        m.denied(
            "t3_attachment_prepare_upload",
            json!({"upload":upload}),
            None,
            "invalid_request",
        );
    }
    m.denied(
        "t3_thread_send_attachments",
        json!({"threadId":"a","attachments":[]}),
        None,
        "invalid_request",
    );
    let file = prepare(&m, "proof.txt", b"proof");
    other.denied(
        "t3_attachment_discard",
        json!({"attachmentId":file["id"]}),
        None,
        "invalid_request",
    );
    m.denied(
        "t3_thread_send_attachments",
        json!({"attachments":[file]}),
        None,
        "target_required",
    );
    let sent = m.ok(
        "t3_thread_send_attachments",
        json!({"threadId":"a","attachments":[file]}),
    );
    assert!(sent["messageId"].is_string());
    let p = projection(&s, "a");
    let message = &p["messages"][0];
    let copy = message["attachments"][0].clone();
    assert_ne!(copy["id"], file["id"]);
    assert_eq!(message["text"], "");
    assert_eq!(message["createdBy"], "agent");
    assert_eq!(message["creationSource"], "mcp");
    let path = o3_server::assets::attachment_path(&s.state, copy["id"].as_str().unwrap()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"proof");
    m.ok("t3_attachment_discard", json!({"attachmentId":file["id"]}));
    assert!(o3_server::assets::attachment_path(&s.state, file["id"].as_str().unwrap()).is_none());
    assert!(path.is_file());
    m.denied(
        "t3_thread_send_attachments",
        json!({"threadId":"b","attachments":[copy]}),
        None,
        "invalid_request",
    );
    let mut forged = copy.clone();
    forged["name"] = json!("forged.txt");
    forged["mimeType"] = json!("evil/mime");
    forged["sizeBytes"] = json!(999);
    let sent2 = other.ok(
        "t3_thread_send_attachments",
        json!({"threadId":"a","message":"reuse","attachments":[forged]}),
    );
    assert_ne!(sent["messageId"], sent2["messageId"]);
    let p = projection(&s, "a");
    assert_eq!(
        p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == sent2["messageId"])
            .unwrap()["attachments"][0],
        copy
    );
    let d = s.connect();
    d.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.archive","commandId":"archive","threadId":"a"}),
    )
    .unwrap();
    m.denied(
        "t3_thread_send_attachments",
        json!({"threadId":"a","attachments":[copy]}),
        None,
        "invalid_request",
    );
}
#[test]
fn web_claims_are_atomic_remap_only_attachment_records_and_replay_after_source_deletion() {
    let s = support::start(|_| {});
    create(&s, "t");
    let c = s.connect();
    let file = c
        .upload_attachment("proof.txt", "text/plain", 5, std::io::Cursor::new(b"proof"))
        .unwrap();
    let command = json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"hi","attachments":[file],"context":{"records":[{"kind":"file","attachmentId":file["id"],"future":"retained"},{"kind":"terminal","attachmentId":file["id"]}]},"dispatchMode":{"type":"defer_start"}});
    s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_attachment BEFORE INSERT ON records WHEN NEW.kind='runs' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(c
        .rpc("orchestration.dispatchCommand", command.clone())
        .is_err());
    let dir = o3_server::assets::attachments_dir(&s.state);
    assert_eq!(
        std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().unwrap().is_file())
            .count(),
        1
    );
    s.state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_attachment;")
        .unwrap();
    c.rpc("orchestration.dispatchCommand", command.clone())
        .unwrap();
    let p = projection(&s, "t");
    let m = &p["messages"][0];
    let id = m["attachments"][0]["id"].clone();
    assert_ne!(id, file["id"]);
    assert_eq!(m["context"]["records"][0]["attachmentId"], id);
    assert_eq!(m["context"]["records"][0]["future"], "retained");
    assert_eq!(m["context"]["records"][1]["attachmentId"], file["id"]);
    c.rpc("attachments.delete", json!({"attachmentId":file["id"]}))
        .unwrap();
    c.rpc("orchestration.dispatchCommand", command).unwrap();
    assert_eq!(projection(&s, "t")["messages"].as_array().unwrap().len(), 1);
    assert!(o3_server::assets::attachment_path(&s.state, id.as_str().unwrap()).is_some());
}
#[test]
fn question_claims_persist_answer_projection_and_extract_readable_sidecars() {
    let s = support::start(|s| s.set_document_worker(env!("CARGO_BIN_EXE_o3-server").into()));
    create(&s, "t");
    let c = s.connect();
    let rtf = br"{\rtf1\ansi Document\par Answer 731}";
    let file = c
        .upload_attachment(
            "answer.rtf",
            "application/rtf",
            rtf.len() as u64,
            std::io::Cursor::new(rtf),
        )
        .unwrap();
    {
        let db = s.state.db.lock().unwrap();
        for (kind, id, body) in [
            (
                "runs",
                "run",
                json!({"id":"run","threadId":"t","status":"completed"}),
            ),
            (
                "runtimeRequests",
                "req",
                json!({"id":"req","threadId":"t","runId":"run","nodeId":"run","status":"pending","kind":"user-input","approvalPolicyKey":null}),
            ),
            (
                "turnItems",
                "approval-req",
                json!({"id":"approval-req","threadId":"t","runId":"run","type":"user_input_request","ordinal":1,"questions":[{"id":"q","question":"Which answer?"}]}),
            ),
        ] {
            db.execute("INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,'t',?3) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",rusqlite::params![kind,id,body.to_string()]).unwrap();
        }
    }
    let command = json!({"type":"runtime-request.respond","commandId":"answer","threadId":"t","requestId":"req","answers":{"q":["selected"]},"attachmentsByQuestionId":{"q":[file]}});
    c.rpc("orchestration.dispatchCommand", command.clone())
        .unwrap();
    let p = projection(&s, "t");
    let item = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "approval-req")
        .unwrap();
    let answer = &item["questionAnswer"];
    assert_eq!(answer["requestId"], "req");
    assert_eq!(answer["questionTextById"]["q"], "Which answer?");
    assert_eq!(answer["answers"]["q"][0], "selected");
    let text = answer["answers"]["q"][1].as_str().unwrap();
    assert!(text.contains("Attached file \"answer.rtf\""), "{item}");
    assert!(text.contains("text extracted to:"), "{item}");
    let claimed = &answer["attachmentsByQuestionId"]["q"][0];
    assert_ne!(claimed["id"], file["id"]);
    let path =
        o3_server::assets::attachment_path(&s.state, claimed["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{}.txt", path.display())).unwrap(),
        "Document\nAnswer 731"
    );
    c.rpc("attachments.delete", json!({"attachmentId":file["id"]}))
        .unwrap();
    c.rpc("orchestration.dispatchCommand", command).unwrap();
    assert!(path.is_file());
}
#[test]
fn queue_edits_claim_once_and_total_question_budget_precedes_file_io() {
    let s = support::start(|_| {});
    create(&s, "t");
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"first","messageId":"m0","threadId":"t","text":"preparing","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"deferred","attachments":[],"dispatchMode":{"type":"defer_start","workspaceStrategy":{"type":"root"}}})).unwrap();
    let p = projection(&s, "t");
    let run = p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "m")
        .unwrap()["runId"]
        .clone();
    let file = c
        .upload_attachment("edit.txt", "text/plain", 4, std::io::Cursor::new(b"edit"))
        .unwrap();
    let edit = json!({"type":"queued-run.edit","commandId":"edit","threadId":"t","runId":run,"text":"revised","attachments":[file],"context":{"records":[{"kind":"file","attachmentId":file["id"]}]}});
    c.rpc("orchestration.dispatchCommand", edit.clone())
        .unwrap();
    let p = projection(&s, "t");
    let message = p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "m")
        .unwrap();
    assert_eq!(message["text"], "revised");
    let copy = message["attachments"][0]["id"].clone();
    assert_ne!(copy, file["id"]);
    assert_eq!(message["context"]["records"][0]["attachmentId"], copy);
    c.rpc("attachments.delete", json!({"attachmentId":file["id"]}))
        .unwrap();
    c.rpc("orchestration.dispatchCommand", edit).unwrap();
    assert!(projection(&s, "t")["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "m" && m["attachments"][0]["id"] == copy));
    let too_many = json!({"type":"runtime-request.respond","commandId":"many","threadId":"t","requestId":"none","answers":{},"attachmentsByQuestionId":{"a":vec![file.clone();50],"b":vec![file.clone();51]}});
    let err = c
        .rpc("orchestration.dispatchCommand", too_many)
        .unwrap_err();
    assert!(err.contains("100 files"), "{err}");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"opaque","messageId":"opaque","threadId":"t","text":"future metadata","attachments":[{"id":"external-future","type":"vendor-unknown","name":"Future","mimeType":"application/octet-stream","sizeBytes":70*1024*1024}],"dispatchMode":{"type":"defer_start","workspaceStrategy":{"type":"root"}}})).unwrap();
    let image = json!({"type":"file","id":file["id"],"mimeType":"image/png","sizeBytes":10*1024*1024,"name":"image.png"});
    let too_large = json!({"type":"message.dispatch","commandId":"large","messageId":"large","threadId":"t","text":"","attachments":vec![image;9],"dispatchMode":{"type":"defer_start"}});
    let err = c
        .rpc("orchestration.dispatchCommand", too_large)
        .unwrap_err();
    assert!(err.contains("80 MiB"), "{err}");
}
#[test]
fn native_attachment_tools_default_to_caller_and_both_transports_read_copies_after_discard() {
    for instance in ["claudeAgent", "codex"] {
        let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mcp-attachments-peer.py");
        let s = support::start(|s| {
            s.set_document_worker(env!("CARGO_BIN_EXE_o3-server").into());
            std::fs::write(
                s.dir.join("settings.json"),
                json!({"enableAgentBrowserAccess":false}).to_string(),
            )
            .unwrap();
            *s.provider.lock().unwrap() =
                Some(std::sync::Arc::new(o3_server::provider::Providers {
                    claude: Some(std::sync::Arc::new(o3_server::claude::ClaudeProvider::new(
                        peer.clone(),
                    ))),
                    codex: std::sync::Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
                }));
        });
        let root = s.dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let c = s.connect();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Native","workspaceRoot":root})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Native attachments","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"start","messageId":"start","threadId":"t","text":"attachment-parent","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(
            support::wait_for(std::time::Duration::from_secs(20), || root
                .join("native-attachment-proof")
                .exists()),
            "{instance}: {}",
            projection(&s, "t")
        );
        let sent: Value =
            serde_json::from_slice(&std::fs::read(root.join("native-attachment.json")).unwrap())
                .unwrap();
        assert_eq!(sent["threadId"], "t");
        let p = projection(&s, "t");
        let message = p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == sent["messageId"])
            .unwrap();
        assert_eq!(message["text"], "");
        assert_eq!(message["senderThreadId"], "t");
        assert_eq!(message["createdBy"], "agent");
        let item = p["turnItems"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["messageId"] == sent["messageId"])
            .unwrap();
        assert_eq!(item["createdBy"], "agent");
        assert_eq!(item["creationSource"], "mcp");
        assert_eq!(item["senderThreadId"], "t");
        assert!(
            std::fs::read_dir(o3_server::assets::attachments_dir(&s.state))
                .unwrap()
                .all(|e| !e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("pending-"))
        );
    }
}
#[test]
fn document_extraction_does_not_hold_the_sqlite_write_lock() {
    use std::os::unix::fs::PermissionsExt;
    let s = support::start(|s| {
        let program = s.dir.join("worker.py");
        std::fs::write(&program,"#!/usr/bin/python3\nimport pathlib,sys,time\nroot=pathlib.Path(sys.argv[2]).parent.parent\n(root/'extract-started').touch()\nend=time.monotonic()+10\nwhile not (root/'extract-release').exists() and time.monotonic()<end: time.sleep(.02)\nprint('Extracted proof',end='')\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        s.set_document_worker(program);
    });
    create(&s, "t");
    let c = s.connect();
    let file = c
        .upload_attachment(
            "slow.rtf",
            "application/rtf",
            5,
            std::io::Cursor::new(b"proof"),
        )
        .unwrap();
    let sending = s.connect();
    let send = std::thread::spawn(move || {
        sending.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"","attachments":[file],"dispatchMode":{"type":"defer_start"}}))
    });
    assert!(support::wait_for(std::time::Duration::from_secs(3), || s
        .dir
        .join("extract-started")
        .exists()));
    let now = std::time::Instant::now();
    create(&s, "independent");
    let elapsed = now.elapsed();
    std::fs::write(s.dir.join("extract-release"), "").unwrap();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "SQLite writer blocked for {elapsed:?}"
    );
    send.join().unwrap().unwrap();
    let p = projection(&s, "t");
    let path = o3_server::assets::attachment_path(
        &s.state,
        p["messages"][0]["attachments"][0]["id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{}.txt", path.display())).unwrap(),
        "Extracted proof"
    );
}
