//! Conformance: the real client library against a real o3-server.
use serde_json::json;
use t3_protocol::{
    fetch_descriptor, pair_bearer, Connection, ConnectionOptions, Credential, PairingClient,
};

/// `O3_CONFORMANCE_BASE` + `O3_CONFORMANCE_PAIRING` aim this suite at any server, such as
/// a real T3 headless: the same assertions must hold on both.
fn target() -> (String, String) {
    match std::env::var("O3_CONFORMANCE_BASE") {
        Ok(base) => (
            base,
            std::env::var("O3_CONFORMANCE_PAIRING").expect("O3_CONFORMANCE_PAIRING"),
        ),
        Err(_) => spawn_server(),
    }
}

fn spawn_server() -> (String, String) {
    spawn_with_provider(false)
}

fn spawn_with_provider(fake: bool) -> (String, String) {
    let dir = std::env::temp_dir().join(format!(
        "o3-conformance-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let state = o3_server::store::State::open(&dir).unwrap();
    let pairing = state.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(state);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let state = o3_server::store::State::open(&dir).unwrap();
        if fake {
            *state.provider.lock().unwrap() =
                Some(std::sync::Arc::new(o3_server::provider::ScriptedProvider));
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .thread_stack_size(512 * 1024)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                o3_server::serve(
                    tokio::net::TcpListener::from_std(listener).unwrap(),
                    std::sync::Arc::new(state),
                )
                .await
                .unwrap();
            })
    });
    (base, pairing)
}

fn pair(base: &str, secret: &str) -> t3_protocol::Result<Connection> {
    let paired = pair_bearer(
        base,
        secret,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )?;
    paired.connection(&ConnectionOptions::default())
}

#[test]
fn pairing_is_one_use_and_session_authenticates_rpc() {
    let (base, secret) = target();
    assert_eq!(
        fetch_descriptor(&base).unwrap()["orchestrationProtocolVersion"],
        2
    );
    let conn = pair(&base, &secret).unwrap();
    assert!(conn.rpc("server.nope", json!({})).is_err());
    assert!(
        pair(&base, &secret).is_err(),
        "a pairing secret must not work twice"
    );
}

#[test]
fn rpc_requires_a_valid_credential() {
    let (base, _) = target();
    let conn = Connection::new(&base, None, Credential::bearer("forged-token").unwrap()).unwrap();
    let error = conn.rpc("server.nope", json!({})).unwrap_err();
    assert!(
        error.to_string().contains("rpc_connection_failed")
            || error.to_string().contains("unauthorized"),
        "{error}"
    );
}

#[test]
fn shell_subscription_opens_with_a_snapshot() {
    let (base, secret) = target();
    let conn = pair(&base, &secret).unwrap();
    let done = std::cell::Cell::new(false);
    let mut first = None;
    conn.subscribe(
        "orchestration.subscribeShell",
        json!({}),
        || done.get(),
        |item| {
            if let Some(item) = item {
                first.get_or_insert(item);
                done.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    let first = first.expect("snapshot");
    assert_eq!(first["kind"], "snapshot");
    assert!(first["snapshot"]["projects"].is_array());
}

fn connected(fake: bool) -> (String, Connection) {
    let (base, secret) = spawn_with_provider(fake);
    let conn = pair(&base, &secret).unwrap();
    (base, conn)
}
fn create(conn: &Connection, id: &str) {
    conn.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("create-{id}"),"threadId":id,"projectId":"project-1","title":"Test","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
}
fn snap(conn: &Connection, id: &str) -> serde_json::Value {
    conn.request_json(
        "GET",
        &format!("/api/orchestration/threads/{id}/bounded"),
        None,
        true,
    )
    .unwrap()
}
fn stream_first(conn: &Connection, tag: &str, p: serde_json::Value) -> serde_json::Value {
    let done = std::cell::Cell::new(false);
    let mut first = None;
    conn.subscribe(
        tag,
        p,
        || done.get(),
        |v| {
            if let Some(v) = v {
                first = Some(v);
                done.set(true)
            }
            Ok(())
        },
    )
    .unwrap();
    first.unwrap()
}
#[test]
fn config_and_settings_round_trip_unknown_fields_and_secret_sentinel() {
    let (_, c) = connected(false);
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert!(config["providers"].is_array());
    assert_eq!(config["environment"]["orchestrationProtocolVersion"], 2);
    assert!(config["keybindings"].is_array());
    let changed=c.rpc("server.updateSettings",json!({"patch":{"futureField":{"x":1},"providerInstances":{"scripted":{"driver":"claudeAgent","config":{"apiKey":"private-key"},"environment":[{"name":"SECRET","sensitive":true,"value":"secret"}]}}}})).unwrap();
    assert_eq!(
        changed["providerInstances"]["scripted"]["config"]["apiKey"],
        "••••••"
    );
    assert_eq!(
        changed["providerInstances"]["scripted"]["environment"][0]["value"],
        ""
    );
    let changed=c.rpc("server.updateSettings",json!({"patch":{"providerInstances":changed["providerInstances"],"defaultTheme":"dark"}})).unwrap();
    assert_eq!(changed["futureField"]["x"], 1);
    let read = c.rpc("server.getSettings", json!({})).unwrap();
    assert_eq!(read["defaultTheme"], "dark");
    assert_eq!(read["futureField"]["x"], 1);
    let first = stream_first(&c, "server.subscribeServerConfig", json!({}));
    assert_eq!(first["type"], "snapshot");
    assert_eq!(
        stream_first(&c, "subscribeServerConfig", json!({}))["type"],
        "snapshot"
    );
    assert_eq!(first["config"]["settings"]["defaultTheme"], "dark");
}
#[test]
fn thread_commands_are_durable_and_idempotent() {
    let (_, c) = connected(false);
    create(&c, "t1");
    let before = snap(&c, "t1");
    create(&c, "t1");
    assert_eq!(
        snap(&c, "t1")["snapshotSequence"],
        before["snapshotSequence"]
    );
    for (command, field, set) in [
        ("thread.archive", "archivedAt", true),
        ("thread.unarchive", "archivedAt", false),
        ("thread.pin", "pinnedAt", true),
        ("thread.unpin", "pinnedAt", false),
        ("thread.settle", "settledAt", true),
        ("thread.unsettle", "settledAt", false),
        ("thread.visit", "lastVisitedAt", true),
    ] {
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":command,"commandId":command,"threadId":"t1","reason":"user"}),
        )
        .unwrap();
        assert_eq!(
            !snap(&c, "t1")["projection"]["thread"][field].is_null(),
            set
        );
    }
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.metadata.update","commandId":"rename","threadId":"t1","title":"Renamed"})).unwrap();
    assert_eq!(snap(&c, "t1")["projection"]["thread"]["title"], "Renamed");
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"delete","threadId":"t1"}),
    )
    .unwrap();
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert!(shell["threads"].as_array().unwrap().is_empty());
}
#[test]
fn shell_and_thread_resume_replay_and_completion_markers() {
    let (_, c) = connected(false);
    create(&c, "t1");
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(shell["threads"][0]["id"], "t1");
    let seq = shell["snapshotSequence"].clone();
    let initial = stream_first(
        &c,
        "orchestration.subscribeThread",
        json!({"threadId":"t1"}),
    );
    assert_eq!(initial["kind"], "snapshot");
    assert_eq!(initial["projection"]["thread"]["id"], "t1");
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.pin","commandId":"pin","threadId":"t1"}),
    )
    .unwrap();
    let event = stream_first(
        &c,
        "orchestration.subscribeShell",
        json!({"afterSequence":seq}),
    );
    assert_eq!(event["kind"], "thread.updated");
    assert!(!event["thread"]["pinnedAt"].is_null());
    let event = stream_first(
        &c,
        "orchestration.subscribeThread",
        json!({"threadId":"t1","afterSequence":seq}),
    );
    assert_eq!(event["event"]["type"], "thread.pinned");
    let seq = snap(&c, "t1")["snapshotSequence"].clone();
    let marker = stream_first(
        &c,
        "orchestration.subscribeShell",
        json!({"afterSequence":seq,"requestCompletionMarker":true}),
    );
    assert_eq!(marker["kind"], "synchronized");
    let marker = stream_first(
        &c,
        "orchestration.subscribeThread",
        json!({"threadId":"t1","afterSequence":seq,"requestCompletionMarker":true}),
    );
    assert_eq!(marker["kind"], "synchronized");
}
#[test]
fn bounded_history_pages_do_not_lose_messages() {
    let (_, c) = connected(false);
    create(&c, "t1");
    for n in 0..125 {
        let command = json!({"type":"message.dispatch","commandId":format!("m{n}"),"messageId":format!("m{n}"),"threadId":"t1","text":format!("message {n}"),"attachments":[],"dispatchMode":{"type":"defer_start"}});
        let a = c
            .rpc("orchestration.dispatchCommand", command.clone())
            .unwrap();
        assert_eq!(a, c.rpc("orchestration.dispatchCommand", command).unwrap());
    }
    let snapshot = snap(&c, "t1");
    assert_eq!(
        snapshot["projection"]["visibleTurnItems"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    assert_eq!(snapshot["hasMoreHistory"], true);
    let cursor = snapshot["historyCursor"].as_str().unwrap();
    let history = c
        .request_json(
            "GET",
            &format!("/api/orchestration/threads/t1/history?cursor={cursor}"),
            None,
            true,
        )
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 25);
    assert_eq!(history["hasMoreHistory"], false);
    assert_eq!(history["items"][0]["item"]["text"], "message 0");
}
#[test]
fn launch_empty_thread_is_idempotent() {
    let (_, c) = connected(false);
    let p = json!({"commandId":"launch","projectId":"p1","title":"Launch","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"}});
    let a = c.rpc("orchestration.launchThread", p.clone()).unwrap();
    assert_eq!(a, c.rpc("orchestration.launchThread", p).unwrap());
    assert_eq!(a["projection"]["thread"]["title"], "Launch");
}
fn wait_status(c: &Connection, status: &str) -> serde_json::Value {
    let start = std::time::Instant::now();
    loop {
        let s = snap(c, "t1");
        if s["projection"]["runs"][0]["status"] == status {
            return s;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "provider did not reach {status}: {s}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn fake_provider_emits_text_approval_and_completion() {
    let (_, c) = connected(true);
    create(&c, "t1");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m1","threadId":"t1","text":"hello","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let s = wait_status(&c, "waiting");
    assert_eq!(s["projection"]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(s["projection"]["runtimeRequests"][0]["status"], "pending");
    assert_eq!(s["projection"]["runs"][0]["status"], "waiting");
    c.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"approve","threadId":"t1","requestId":"approval-1","decision":"accept"})).unwrap();
    let s = wait_status(&c, "completed");
    assert_eq!(s["projection"]["runs"][0]["status"], "completed");
    assert_eq!(s["projection"]["messages"][1]["streaming"], false);
}
#[test]
fn run_interrupt_and_input_dismiss() {
    let (_, c) = connected(true);
    create(&c, "t1");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m1","threadId":"t1","text":"hello","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let s = wait_status(&c, "waiting");
    let rid = s["projection"]["runs"][0]["id"].clone();
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"run.interrupt","commandId":"stop","threadId":"t1","runId":rid}),
    )
    .unwrap();
    assert_eq!(
        snap(&c, "t1")["projection"]["runs"][0]["status"],
        "interrupted"
    );
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.user-input.dismiss","commandId":"dismiss","threadId":"t1","requestId":"approval-1"})).unwrap();
    assert_eq!(
        snap(&c, "t1")["projection"]["runtimeRequests"][0]["status"],
        "cancelled"
    );
}
#[test]
fn auth_management_and_scopes() {
    let (base, c) = connected(false);
    let grant = c
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"label":"read only","scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let read = pair(&base, grant["credential"].as_str().unwrap()).unwrap();
    assert!(read.rpc("server.getConfig", json!({})).is_ok());
    let error = read
        .rpc("server.updateSettings", json!({"patch":{}}))
        .unwrap_err();
    assert!(error.to_string().contains("settings:write"), "{error}");
    let session = read
        .request_json("GET", "/api/auth/session", None, false)
        .unwrap();
    assert_eq!(session["scopes"], json!(["orchestration:read"]));
    let clients = c
        .request_json("GET", "/api/auth/clients", None, false)
        .unwrap();
    let sid = clients
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["current"] == false)
        .unwrap()["sessionId"]
        .clone();
    c.request_json(
        "POST",
        "/api/auth/clients/revoke",
        Some(&json!({"sessionId":sid})),
        false,
    )
    .unwrap();
    assert!(read.rpc("server.getConfig", json!({})).is_err());
    let grant = c
        .request_json("POST", "/api/auth/pairing-token", Some(&json!({})), false)
        .unwrap();
    let links = c
        .request_json("GET", "/api/auth/pairing-links", None, false)
        .unwrap();
    assert!(links
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == grant["id"]));
    c.request_json(
        "POST",
        "/api/auth/pairing-links/revoke",
        Some(&json!({"id":grant["id"]})),
        false,
    )
    .unwrap();
    assert!(pair(&base, grant["credential"].as_str().unwrap()).is_err());
    c.request_json(
        "POST",
        "/api/auth/clients/revoke-others",
        Some(&json!({})),
        false,
    )
    .unwrap();
}

#[test]
fn multiplex_batches_ack_interrupt_eof_and_revocation() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let (base, secret) = spawn_server();
    let paired = pair_bearer(
        &base,
        &secret,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )
    .unwrap();
    let token = paired.access_token.to_string();
    let c = paired.connection(&ConnectionOptions::default()).unwrap();
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).max_blocking_threads(2).thread_stack_size(512*1024).enable_all().build().unwrap().block_on(async {
  let addr=base.trim_start_matches("http://");let tcp=tokio::net::TcpStream::connect(addr).await.unwrap();let mut request=format!("ws://{addr}/ws?orchestrationProtocol=2").into_client_request().unwrap();request.headers_mut().insert("Authorization",format!("Bearer {token}").parse().unwrap());let (mut ws,_)=tokio_tungstenite::client_async(request,tcp).await.unwrap();
  ws.send(Message::Text(json!([{"_tag":"Request","id":"stream","tag":"orchestration.subscribeShell","payload":{"requestCompletionMarker":true}},{"_tag":"Request","id":"unary","tag":"server.getConfig","payload":{}},{"_tag":"Ping"}]).to_string().into())).await.unwrap();
  let mut frames=Vec::new();for _ in 0..3 {let v:serde_json::Value=serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();frames.push(v);}
  assert!(frames.iter().any(|v|v["_tag"]=="Pong"));assert!(frames.iter().any(|v|v["requestId"]=="unary"&&v["_tag"]=="Exit"));assert!(frames.iter().any(|v|v["requestId"]=="stream"&&v["_tag"]=="Chunk"));
  // No second chunk until Ack.
  assert!(tokio::time::timeout(std::time::Duration::from_millis(50),ws.next()).await.is_err());
  ws.send(Message::Text(json!({"_tag":"Ack","requestId":"stream"}).to_string().into())).await.unwrap();let v:serde_json::Value=serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();assert_eq!(v["values"][0]["kind"],"synchronized");
  ws.send(Message::Text(json!({"_tag":"Interrupt","requestId":"stream"}).to_string().into())).await.unwrap();let v:serde_json::Value=serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();assert_eq!(v["_tag"],"Exit");assert_eq!(v["requestId"],"stream");
  std::thread::spawn(move || {let clients=c.request_json("GET","/api/auth/clients",None,false).unwrap();let sid=clients[0]["sessionId"].as_str().unwrap();let admin_grant=c.request_json("POST","/api/auth/pairing-token",Some(&json!({})),false).unwrap();let admin=pair(&base,admin_grant["credential"].as_str().unwrap()).unwrap();admin.request_json("POST","/api/auth/clients/revoke",Some(&json!({"sessionId":sid})),false).unwrap();}).join().unwrap();
  assert!(matches!(tokio::time::timeout(std::time::Duration::from_secs(3),ws.next()).await.unwrap(),Some(Ok(Message::Close(_)))|None));
 });
}
#[test]
fn protocol_rejected_before_auth() {
    use std::io::{Read, Write};
    let (base, _) = spawn_server();
    for path in ["/ws?orchestrationProtocol=1", "/api/orchestration/shell"] {
        let mut s = std::net::TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())
            .unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert!(reply.lines().next().unwrap().contains("426"), "{reply}");
        assert!(reply.contains("orchestration_protocol_incompatible"));
    }
}

#[test]
fn projects_share_the_thread_sequence_and_replay_log() {
    let (_, c) = connected(false);
    let p = json!({"type":"project.create","commandId":"project-create","projectId":"p1","title":"Project","workspaceRoot":"/tmp","scripts":[]});
    let result = c.rpc("projects.mutate", p.clone()).unwrap();
    assert_eq!(result, c.rpc("projects.mutate", p).unwrap());
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(shell["projects"][0]["id"], "p1");
    let seq = shell["snapshotSequence"].as_i64().unwrap();
    create(&c, "t1");
    assert!(snap(&c, "t1")["snapshotSequence"].as_i64().unwrap() > seq);
    let event = stream_first(
        &c,
        "orchestration.subscribeShell",
        json!({"afterSequence":0}),
    );
    assert_eq!(event["kind"], "project.updated");
}

#[test]
fn queued_dispatch_waits_for_completion_and_steering_reuses_active_run() {
    let (_, c) = connected(true);
    create(&c, "t1");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"first","threadId":"t1","text":"first","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let first = wait_status(&c, "waiting")["projection"]["runs"][0]["id"].clone();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"queue","messageId":"second","threadId":"t1","text":"second","attachments":[],"dispatchMode":{"type":"queue_after_active"}})).unwrap();
    let queued = snap(&c, "t1");
    assert_eq!(queued["projection"]["runs"][1]["status"], "queued");
    let steer = json!({"type":"message.dispatch","commandId":"steer","messageId":"steer","threadId":"t1","text":"STEERING","attachments":[],"dispatchMode":{"type":"steer_active","targetRunId":first}});
    let receipt = c
        .rpc("orchestration.dispatchCommand", steer.clone())
        .unwrap();
    assert_eq!(
        c.rpc("orchestration.dispatchCommand", steer).unwrap(),
        receipt
    );
    assert_eq!(
        snap(&c, "t1")["projection"]["runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    c.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"approve-first","threadId":"t1","requestId":"approval-1","decision":"accept"})).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let s = snap(&c, "t1");
        if s["projection"]["runs"][1]["status"] == "waiting" {
            assert_eq!(s["projection"]["runs"][0]["status"], "completed");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{s}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    c.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"approve-second","threadId":"t1","requestId":"approval-1","decision":"accept"})).unwrap();
}

#[test]
fn idle_model_and_permission_mode_updates_are_durable() {
    let (_, c) = connected(false);
    create(&c, "t1");
    for command in [
        json!({"type":"thread.model-selection.set","modelSelection":{"instanceId":"claude","model":"haiku"}}),
        json!({"type":"thread.runtime-mode.set","runtimeMode":"approval-required"}),
        json!({"type":"thread.interaction-mode.set","interactionMode":"plan"}),
    ] {
        let mut command = command;
        command["commandId"] = command["type"].clone();
        command["threadId"] = json!("t1");
        c.rpc("orchestration.dispatchCommand", command).unwrap();
    }
    let s = snap(&c, "t1");
    let t = &s["projection"]["thread"];
    assert_eq!(t["modelSelection"]["model"], "haiku");
    assert_eq!(t["runtimeMode"], "approval-required");
    assert_eq!(t["interactionMode"], "plan");
}
