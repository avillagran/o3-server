mod common;
mod support;
use common::http;
use serde_json::{json, Value};

fn request(
    s: &support::Server,
    token: &str,
    transport: Option<&str>,
    v: Value,
) -> (u16, String, Value) {
    let auth = format!("Bearer {token}");
    let mut h = vec![
        ("authorization", auth.as_str()),
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
        ("mcp-protocol-version", "2025-06-18"),
    ];
    if let Some(id) = transport {
        h.push(("mcp-session-id", id));
    }
    let (code, headers, body) = http(&s.base, "POST", "/mcp", &h, v.to_string().as_bytes());
    (
        code,
        headers,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}
fn init(s: &support::Server, token: &str) -> String {
    let (code, h, v) = request(
        s,
        token,
        None,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"o3-test","version":"1"}}}),
    );
    assert_eq!(code, 200, "{h} {v}");
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    h.lines()
        .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
        .unwrap()
        .to_owned()
}
#[test]
fn mcp_transport_credentials_lifecycle_and_readers() {
    let s = support::start(|_| {});
    let token = s
        .state
        .issue_session(
            "mcp-client",
            &["orchestration:read".into()],
            60_000,
            "MCP reader",
        )
        .unwrap();
    let id = init(&s, &token);
    let (_, _, v) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    assert!(
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "t3_project_list"),
        "{v}"
    );
    let (_, _, v) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"t3_project_list","arguments":{}}}),
    );
    assert_eq!(v["id"], "call");
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert_eq!(v["result"]["structuredContent"]["projects"], json!([]));
    let (code, _, _) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    assert_eq!(code, 202);
    let auth = format!("Bearer {token}");
    assert_eq!(
        http(
            &s.base,
            "DELETE",
            "/mcp",
            &[("authorization", &auth), ("mcp-session-id", &id)],
            b""
        )
        .0,
        204
    );
    assert_eq!(
        request(
            &s,
            &token,
            Some(&id),
            json!({"jsonrpc":"2.0","id":3,"method":"ping"})
        )
        .0,
        404
    );
}
#[test]
fn mcp_credentials_cannot_escape_to_environment_http_or_websocket() {
    let s = support::start(|_| {});
    let token = s
        .state
        .issue_session(
            "mcp-client",
            &["orchestration:read".into(), "orchestration:operate".into()],
            60_000,
            "MCP limited",
        )
        .unwrap();
    let auth = format!("Bearer {token}");
    assert_eq!(
        http(
            &s.base,
            "GET",
            "/api/projects",
            &[("authorization", &auth)],
            b""
        )
        .0,
        401
    );
    assert_eq!(
        http(
            &s.base,
            "POST",
            "/api/auth/websocket-ticket",
            &[("authorization", &auth)],
            b""
        )
        .0,
        401
    );
    assert_eq!(
        http(
            &s.base,
            "GET",
            "/ws?orchestrationProtocol=2",
            &[("authorization", &auth)],
            b""
        )
        .0,
        401
    );
    let owner = s
        .state
        .issue_session("owner", &["orchestration:read".into()], 60_000, "owner")
        .unwrap();
    let (code, h, _) = request(
        &s,
        &owner,
        None,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
    );
    assert_eq!(code, 401);
    assert!(h.contains("www-authenticate: bearer"));
}

fn call(s: &support::Server, token: &str, id: &str, name: &str, args: Value) -> Value {
    let (code, _, v) = request(
        s,
        token,
        Some(id),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":name,"arguments":args}}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["result"]["isError"], false, "{v}");
    let structured = v["result"]["structuredContent"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap(),
        structured
    );
    structured
}
#[test]
fn mcp_reads_real_rpc_projects_threads_and_paginated_unicode_transcript() {
    let s = support::start(|_| {});
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Project","workspaceRoot":s.dir,"scripts":[]})).unwrap();
    for tid in ["a", "b"] {
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("c-{tid}"),"threadId":tid,"projectId":"p","title":format!("Thread {tid}"),"modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    }
    for (mid, text) in [("m1", "a😀b transcript needle"), ("m2", "next message")] {
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":format!("dispatch-{mid}"),"messageId":mid,"threadId":"a","text":text,"attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    }
    let at = o3_server::store::iso(o3_server::store::now_ms());
    let visible = json!({"position":999,"visibility":"local","sourceThreadId":"a","sourceItemId":"reason","item":{"id":"reason","threadId":"a","type":"reasoning","status":"completed","title":null,"runId":null,"text":"thinking text","updatedAt":at}});
    s.state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO records(kind,id,thread_id,body) VALUES('visibleTurnItems','reason','a',?)",
            [visible.to_string()],
        )
        .unwrap();
    let token = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let id = init(&s, &token);
    let projects = call(&s, &token, &id, "t3_project_list", json!({"limit":1}));
    assert_eq!(projects["projects"][0]["id"], "p");
    assert_eq!(
        call(&s, &token, &id, "t3_project_read", json!({"projectId":"p"}))["title"],
        "Project"
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_configuration",
            json!({"threadId":"a"})
        )["modelSelection"]["model"],
        "test"
    );
    let list = call(
        &s,
        &token,
        &id,
        "t3_thread_list",
        json!({"projectId":"p","limit":1}),
    );
    assert_eq!(list["total"], 2);
    assert_eq!(list["nextCursor"], 1);
    let next = call(
        &s,
        &token,
        &id,
        "t3_thread_list",
        json!({"projectId":"p","cursor":1,"limit":1}),
    );
    assert_ne!(
        list["threads"][0]["threadId"],
        next["threads"][0]["threadId"]
    );
    let r = call(
        &s,
        &token,
        &id,
        "t3_thread_read",
        json!({"threadId":"a","limit":1,"maxCharsPerItem":2}),
    );
    assert_eq!(r["thread"]["itemCount"], 5);
    let activity = call(
        &s,
        &token,
        &id,
        "t3_thread_read",
        json!({"threadId":"a","view":"activity","afterPosition":998}),
    );
    assert_eq!(activity["items"][0]["text"], "thinking text");
    let counted = call(
        &s,
        &token,
        &id,
        "t3_thread_list",
        json!({"projectId":"p","titleContains":"Thread a"}),
    );
    assert_eq!(counted["threads"][0]["itemCount"], 5);
    assert_eq!(r["thread"]["threadId"], "a");
    assert_eq!(r["hasMore"], true);
    assert_eq!(r["items"][0]["text"], "a😀");
    assert_eq!(r["items"][0]["nextTextOffset"], 3);
    let item = r["items"][0]["itemId"].clone();
    let tail = call(
        &s,
        &token,
        &id,
        "t3_thread_read",
        json!({"threadId":"a","itemId":item,"textOffset":3}),
    );
    assert_eq!(tail["items"][0]["text"], "b transcript needle");
    let next = call(
        &s,
        &token,
        &id,
        "t3_thread_read",
        json!({"threadId":"a","afterPosition":r["nextPosition"]}),
    );
    assert_eq!(next["items"][0]["text"], "next message");
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_search",
            json!({"query":"needle"})
        )["matches"][0]["threadId"],
        "a"
    );
    let env = call(&s, &token, &id, "t3_environment_read", json!({}));
    assert_eq!(env["environmentId"], s.state.environment_id);
    assert_eq!(
        env["preferences"]["sourceControlWritingStyle"]["truncated"],
        false
    );
    assert!(env.get("capabilities").is_none());
}
#[test]
fn mcp_checks_session_ownership_revocation_and_invalid_arguments() {
    let s = support::start(|_| {});
    let token = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let id = init(&s, &token);
    let other = s
        .state
        .issue_mcp_session("read-only", 60_000, "other")
        .unwrap();
    assert_eq!(
        request(
            &s,
            &other,
            Some(&id),
            json!({"jsonrpc":"2.0","id":9,"method":"ping"})
        )
        .0,
        404
    );
    let (_, _, bad) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"t3_thread_read","arguments":{"threadId":"a","limit":101}}}),
    );
    assert_eq!(bad["error"]["code"], -32602);
    let (_, _, missing) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"t3_thread_read","arguments":{"threadId":"absent"}}}),
    );
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(
        missing["result"]["structuredContent"]["code"],
        "thread_not_found"
    );
    let (_, _, unknown) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"t3_project_delete","arguments":{"projectId":"p"}}}),
    );
    assert_eq!(unknown["error"]["code"], -32602);
    let claims = s.state.mcp_session(&token).unwrap();
    s.state
        .revoke_session(claims["sid"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        request(
            &s,
            &token,
            Some(&id),
            json!({"jsonrpc":"2.0","id":9,"method":"ping"})
        )
        .0,
        401
    );
}

#[test]
fn mcp_rejects_invalid_envelopes_and_bounds_transport_sessions() {
    let s = support::start(|_| {});
    let token = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let id = init(&s, &token);
    let auth = format!("Bearer {token}");
    let headers = [
        ("authorization", auth.as_str()),
        ("mcp-session-id", id.as_str()),
        ("content-type", "application/json"),
    ];
    let (status, _, body) = http(&s.base, "POST", "/mcp", &headers, b"{");
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
        -32700
    );
    let (status, _, body) = http(&s.base, "POST", "/mcp", &headers, b"[]");
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
        -32600
    );
    assert_eq!(
        http(
            &s.base,
            "POST",
            "/mcp",
            &headers,
            &vec![b'x'; 1024 * 1024 + 1]
        )
        .0,
        413
    );
    assert_eq!(
        http(
            &s.base,
            "POST",
            "/mcp",
            &[("authorization", &auth), ("content-type", "text/plain")],
            b"{}"
        )
        .0,
        415
    );
    assert_eq!(
        http(
            &s.base,
            "POST",
            "/mcp",
            &[
                ("authorization", &auth),
                ("origin", "https://untrusted.invalid")
            ],
            b"{}"
        )
        .0,
        403
    );
    assert_eq!(
        request(
            &s,
            &token,
            None,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/list"})
        )
        .0,
        400
    );
    let (_, _, v) = request(
        &s,
        &token,
        Some(&id),
        json!({"jsonrpc":"2.0","id":9,"method":"nonexistent"}),
    );
    assert_eq!(v["error"]["code"], -32601);
    for _ in 0..7 {
        init(&s, &token);
    }
    assert_eq!(request(&s,&token,None,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"o3-test","version":"1"}}})).0,429);
}
#[test]
fn mcp_cli_issues_only_dedicated_credentials_and_help_has_no_effects() {
    let s = support::start(|_| {});
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
            .env("T3CODE_HOME", &s.dir)
            .args(args)
            .output()
            .unwrap()
    };
    let help = run(&["auth", "mcp", "issue", "--help"]);
    assert!(help.status.success());
    assert!(!s.dir.join("userdata").exists());
    let bad = run(&["auth", "mcp", "issue", "--access", "invalid"]);
    assert!(!bad.status.success());
    let issued = run(&[
        "auth",
        "mcp",
        "issue",
        "--access",
        "read-only",
        "--token-only",
    ]);
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let cli_state = o3_server::store::State::open(&s.dir.join("userdata")).unwrap();
    let token = String::from_utf8(issued.stdout).unwrap();
    let claims = cli_state
        .mcp_session(token.trim())
        .expect("dedicated MCP session");
    assert_eq!(claims["sub"], "mcp-client");
    assert_eq!(claims["scopes"], json!(["orchestration:read"]));
    assert!(cli_state.session(token.trim()).is_none());
    let issued = run(&[
        "auth",
        "mcp",
        "issue",
        "--access",
        "auto-accept-edits",
        "--token-only",
    ]);
    assert!(issued.status.success());
    let token = String::from_utf8(issued.stdout).unwrap();
    let claims = cli_state.mcp_session(token.trim()).unwrap();
    assert_eq!(claims["runtimeModeCeiling"], "auto-accept-edits");
    assert_eq!(
        cli_state.list_sessions().unwrap().as_array().unwrap().len(),
        2
    );
}

#[test]
fn mcp_rejects_expired_tampered_and_proof_bound_credentials() {
    let s = support::start(|_| {});
    let expired = s
        .state
        .issue_mcp_session("read-only", -1, "expired")
        .unwrap();
    assert_eq!(request(&s, &expired, None, json!({})).0, 401);
    let token = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let claims = o3_server::auth::verify(
        &token,
        "session",
        &s.state.signing_key,
        o3_server::store::now_ms(),
    )
    .unwrap();
    let mut bound = claims.clone();
    bound["jkt"] = json!("proof-key-thumbprint");
    let bound = o3_server::auth::sign(&bound, &s.state.signing_key);
    assert_eq!(request(&s, &bound, None, json!({})).0, 401);
    let tampered = format!("{token}bad");
    assert_eq!(request(&s, &tampered, None, json!({})).0, 401);
    let ticket = o3_server::auth::websocket_ticket(
        &claims,
        &s.state.signing_key,
        o3_server::store::now_ms(),
        Some("rpc"),
    );
    let path = format!("/ws?orchestrationProtocol=2&wsTicket={ticket}");
    assert_eq!(http(&s.base, "GET", &path, &[], b"").0, 401);
    let (code, h, _) = http(&s.base, "POST", "/mcp", &[], b"{");
    assert_eq!(code, 401);
    assert!(h.contains("www-authenticate: bearer"));
}

#[test]
fn mcp_payload_budget_pages_dense_activity_without_losing_items() {
    let s = support::start(|_| {});
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Project","workspaceRoot":s.dir,"scripts":[]})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"tc","threadId":"t","projectId":"p","title":"Dense activity","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let at = o3_server::store::iso(o3_server::store::now_ms());
    for n in 0..50 {
        let id = format!("item-{n}");
        let row = json!({"position":n,"visibility":"local","sourceThreadId":"t","sourceItemId":id,"item":{"id":id,"threadId":"t","type":"reasoning","status":"completed","title":null,"runId":null,"text":"x".repeat(50_000),"updatedAt":at}});
        s.state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('visibleTurnItems',?,'t',?)",
                [&id, &row.to_string()],
            )
            .unwrap();
    }
    let token = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let id = init(&s, &token);
    let mut args = json!({"threadId":"t","view":"activity","limit":100,"maxCharsPerItem":50_000});
    let mut seen = std::collections::BTreeSet::new();
    for page in 0..3 {
        let result = call(&s, &token, &id, "t3_thread_read", args.clone());
        let items = result["items"].as_array().unwrap();
        if page == 0 {
            assert!(items.len() < 50);
            assert_eq!(result["hasMore"], true);
        }
        for item in items {
            assert_eq!(item["text"].as_str().unwrap().len(), 50_000);
            assert!(seen.insert(item["itemId"].as_str().unwrap().to_owned()));
        }
        if result["hasMore"] == false {
            break;
        }
        args["afterPosition"] = result["nextPosition"].clone();
    }
    assert_eq!(seen.len(), 50);
}

fn control_fixture() -> (support::Server, t3_protocol::Connection) {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(std::sync::Arc::new(o3_server::provider::ScriptedProvider))
    });
    let c = s.connect();
    let root = s.dir.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Control","workspaceRoot":root,"scripts":[]})).unwrap();
    for (tid, mode) in [("low", "approval-required"), ("high", "full-access")] {
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("create-{tid}"),"threadId":tid,"projectId":"p","title":tid,"modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":mode,"interactionMode":"default"})).unwrap();
    }
    (s, c)
}
fn tool_error(s: &support::Server, token: &str, id: &str, name: &str, args: Value) -> Value {
    let (_, _, v) = request(
        s,
        token,
        Some(id),
        json!({"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":name,"arguments":args}}),
    );
    assert_eq!(v["result"]["isError"], true, "{v}");
    v["result"]["structuredContent"].clone()
}
#[test]
fn mcp_writes_enforce_current_credential_and_target_runtime_ceiling() {
    let (s, c) = control_fixture();
    let readonly = s
        .state
        .issue_mcp_session("read-only", 60_000, "reader")
        .unwrap();
    let reader_id = init(&s, &readonly);
    let limited = s
        .state
        .issue_mcp_session("approval-required", 60_000, "limited")
        .unwrap();
    let id = init(&s, &limited);
    assert_eq!(
        tool_error(
            &s,
            &readonly,
            &reader_id,
            "t3_thread_organize",
            json!({"threadId":"low","action":"pin"})
        )["code"],
        "capability_denied"
    );
    assert_eq!(
        tool_error(
            &s,
            &limited,
            &id,
            "t3_thread_organize",
            json!({"threadId":"high","action":"pin"})
        )["code"],
        "runtime_mode_escalation_denied"
    );
    call(
        &s,
        &limited,
        &id,
        "t3_thread_organize",
        json!({"threadId":"low","action":"pin"}),
    );
    assert!(call(
        &s,
        &limited,
        &id,
        "t3_thread_list",
        json!({"projectId":"p"})
    )["threads"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["threadId"] == "low"));
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.runtime-mode.set","commandId":"raise-mode","threadId":"low","runtimeMode":"full-access"})).unwrap();
    assert_eq!(
        tool_error(
            &s,
            &limited,
            &id,
            "t3_thread_configure",
            json!({"threadId":"low","modelSelection":{"instanceId":"scripted","model":"changed"}})
        )["code"],
        "runtime_mode_escalation_denied"
    );
    let full = s
        .state
        .issue_mcp_session("full-access", 60_000, "full")
        .unwrap();
    let full_id = init(&s, &full);
    let claims = s.state.mcp_session(&full).unwrap();
    s.state.db.lock().unwrap().execute("UPDATE auth_sessions SET client=json_set(client,'$.runtimeModeCeiling','approval-required') WHERE session_id=?",[claims["sid"].as_str().unwrap()]).unwrap();
    assert_eq!(
        tool_error(
            &s,
            &full,
            &full_id,
            "t3_thread_organize",
            json!({"threadId":"high","action":"archive"})
        )["code"],
        "runtime_mode_escalation_denied"
    );
    assert_eq!(
        call(
            &s,
            &full,
            &full_id,
            "t3_thread_read",
            json!({"threadId":"high"})
        )["thread"]["archived"],
        false
    );
}
#[test]
fn mcp_send_is_idempotent_has_agent_provenance_and_cannot_approve_permissions() {
    let (s, _) = control_fixture();
    let token = s
        .state
        .issue_mcp_session("full-access", 60_000, "control")
        .unwrap();
    let id = init(&s, &token);
    let args = json!({"threadId":"high","message":"test MCP input","clientRequestId":"once"});
    let first = call(&s, &token, &id, "t3_thread_send", args.clone());
    assert_eq!(first["delivery"], "started");
    let replay = call(&s, &token, &id, "t3_thread_send", args);
    assert_eq!(replay["runId"], first["runId"]);
    assert_eq!(replay["messageId"], first["messageId"]);
    assert!(support::wait_for(std::time::Duration::from_secs(5), || {
        s.state.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='runtimeRequests' AND json_extract(body,'$.status')='pending'",[],|r|r.get::<_,i64>(0)).unwrap()==1
    }));
    let user_count:i64=s.state.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='messages' AND thread_id='high' AND json_extract(body,'$.role')='user'",[],|r|r.get(0)).unwrap();
    assert_eq!(user_count, 1);
    let provenance:String=s.state.db.lock().unwrap().query_row("SELECT json_extract(body,'$.createdBy')||'/'||json_extract(body,'$.creationSource') FROM records WHERE kind='messages' AND id=?",[first["messageId"].as_str().unwrap()],|r|r.get(0)).unwrap();
    assert_eq!(provenance, "agent/mcp");
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_pending_request_list",
            json!({"threadId":"high"})
        )["requestIds"],
        json!([])
    );
    assert_eq!(
        tool_error(
            &s,
            &token,
            &id,
            "t3_pending_request_respond",
            json!({"threadId":"high","requestId":"approval-1","answers":{"decision":"allow"}})
        )["code"],
        "capability_denied"
    );
    let waiting = call(
        &s,
        &token,
        &id,
        "t3_thread_wait",
        json!({"threadId":"high","runId":first["runId"],"timeoutMs":5}),
    );
    assert_eq!(waiting["timedOut"], true);
    call(
        &s,
        &token,
        &id,
        "t3_thread_interrupt",
        json!({"threadId":"high","runId":first["runId"],"clientRequestId":"stop-once"}),
    );
    let done = call(
        &s,
        &token,
        &id,
        "t3_thread_wait",
        json!({"threadId":"high","runId":first["runId"],"timeoutMs":5000}),
    );
    assert_eq!(done["status"], "interrupted");
    let replay = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"test MCP input","clientRequestId":"once"}),
    );
    assert_eq!(replay["runId"], first["runId"]);
    assert_eq!(replay["status"], "interrupted");
}
#[test]
fn mcp_queue_operations_keep_order_and_reject_started_runs() {
    let (s, _) = control_fixture();
    let token = s
        .state
        .issue_mcp_session("full-access", 60_000, "control")
        .unwrap();
    let id = init(&s, &token);
    let first = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"running","clientRequestId":"running"}),
    );
    let a = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"queued a","mode":"queue"}),
    );
    let b = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"queued b","mode":"queue"}),
    );
    assert_eq!(a["delivery"], "queued");
    assert_eq!(b["delivery"], "queued");
    call(
        &s,
        &token,
        &id,
        "t3_queue_edit",
        json!({"threadId":"high","queuedRunId":a["runId"],"text":"edited a"}),
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_queue_read",
            json!({"threadId":"high","queuedRunId":a["runId"]})
        )["text"],
        "edited a"
    );
    call(
        &s,
        &token,
        &id,
        "t3_queue_reorder",
        json!({"threadId":"high","queuedRunId":b["runId"],"beforeRunId":a["runId"]}),
    );
    let list = call(&s, &token, &id, "t3_queue_list", json!({"threadId":"high"}));
    assert_eq!(list["items"][0]["queuedRunId"], b["runId"]);
    assert!(tool_error(
        &s,
        &token,
        &id,
        "t3_queue_edit",
        json!({"threadId":"high","queuedRunId":first["runId"],"text":"bad"})
    )["code"]
        .is_string());
    for run in [&a, &b] {
        call(
            &s,
            &token,
            &id,
            "t3_queue_cancel",
            json!({"threadId":"high","queuedRunId":run["runId"]}),
        );
    }
    assert_eq!(
        call(&s, &token, &id, "t3_queue_list", json!({"threadId":"high"}))["items"],
        json!([])
    );
    call(
        &s,
        &token,
        &id,
        "t3_thread_interrupt",
        json!({"threadId":"high"}),
    );
}
#[test]
fn mcp_organize_configure_and_no_active_interrupt_follow_existing_commands() {
    let (s, _) = control_fixture();
    let token = s
        .state
        .issue_mcp_session("full-access", 60_000, "control")
        .unwrap();
    let id = init(&s, &token);
    call(
        &s,
        &token,
        &id,
        "t3_thread_configure",
        json!({"threadId":"high","modelSelection":{"provider":"scripted","model":"other","options":[{"id":" effort ","value":" high "}]}}),
    );
    let config = call(
        &s,
        &token,
        &id,
        "t3_thread_configuration",
        json!({"threadId":"high"}),
    );
    assert_eq!(config["modelSelection"]["instanceId"], "scripted");
    assert_eq!(config["modelSelection"]["model"], "other");
    assert_eq!(
        config["modelSelection"]["options"],
        json!([{"id":"effort","value":"high"}])
    );
    assert_eq!(config["runtimeMode"], "full-access");
    call(
        &s,
        &token,
        &id,
        "t3_thread_organize",
        json!({"threadId":"high","action":"settle"}),
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_read",
            json!({"threadId":"high"})
        )["thread"]["settled"],
        true
    );
    call(
        &s,
        &token,
        &id,
        "t3_thread_organize",
        json!({"threadId":"high","action":"unsettle"}),
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_interrupt",
            json!({"threadId":"high"})
        )["status"],
        "no_active_run"
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_wait",
            json!({"threadId":"high"})
        )["status"],
        "idle"
    );
    call(
        &s,
        &token,
        &id,
        "t3_thread_organize",
        json!({"threadId":"high","action":"archive"}),
    );
    assert_eq!(
        tool_error(
            &s,
            &token,
            &id,
            "t3_thread_send",
            json!({"threadId":"high","message":"refused"})
        )["code"],
        "invalid_request"
    );
    call(
        &s,
        &token,
        &id,
        "t3_thread_organize",
        json!({"threadId":"high","action":"unarchive"}),
    );
}

#[test]
fn mcp_user_questions_round_trip_through_native_projection_without_permission_approval() {
    use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
    };
    struct QuestionPeer(Arc<Mutex<Vec<Value>>>);
    impl Provider for QuestionPeer {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            output: tokio::sync::mpsc::Sender<ProviderEvent>,
            mut controls: tokio::sync::mpsc::Receiver<ProviderControl>,
            mut cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async move {
                output
                    .send(ProviderEvent::Started)
                    .await
                    .map_err(|e| e.to_string())?;
                output.send(ProviderEvent::Native(json!({"type":"permission","request_id":format!("{}:question",input.run_id),"native_id":"question","request":{"tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which color?","header":"Color","options":[{"label":"Blue","description":"Blue"},{"label":"Green","description":"Green"}]}]}}}))).await.map_err(|e|e.to_string())?;
                tokio::select! {
                    c=controls.recv()=>match c {
                        Some(ProviderControl::Respond{answers,..})=>{self.0.lock().unwrap().push(answers.unwrap_or(Value::Null));output.send(ProviderEvent::Completed).await.map_err(|e|e.to_string())?;},
                        _=>{output.send(ProviderEvent::Interrupted).await.map_err(|e|e.to_string())?;},
                    },
                    _=cancel.changed()=>{},
                }
                Ok(())
            })
        }
    }
    let (s, _) = control_fixture();
    let answers = Arc::new(Mutex::new(Vec::new()));
    *s.state.provider.lock().unwrap() = Some(Arc::new(QuestionPeer(answers.clone())));
    let token = s
        .state
        .issue_mcp_session("full-access", 60_000, "questions")
        .unwrap();
    let id = init(&s, &token);
    let sent = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"Ask a question"}),
    );
    assert!(support::wait_for(std::time::Duration::from_secs(5), || {
        s.state.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='runtimeRequests' AND json_extract(body,'$.kind')='user_input'",[],|r|r.get::<_,i64>(0)).unwrap()==1
    }));
    let list = call(
        &s,
        &token,
        &id,
        "t3_pending_request_list",
        json!({"threadId":"high"}),
    );
    let question = list["requestIds"][0].clone();
    assert!(question.is_string(), "{list}");
    let read = call(
        &s,
        &token,
        &id,
        "t3_pending_request_read",
        json!({"threadId":"high","requestId":question}),
    );
    assert_eq!(read["questions"][0]["question"], "Which color?");
    call(
        &s,
        &token,
        &id,
        "t3_pending_request_respond",
        json!({"threadId":"high","requestId":question,"answers":{"Which color?":"Blue"}}),
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_wait",
            json!({"threadId":"high","runId":sent["runId"],"timeoutMs":5000})
        )["status"],
        "completed"
    );
    assert_eq!(
        *answers.lock().unwrap(),
        vec![json!({"Which color?":"Blue"})]
    );
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_pending_request_list",
            json!({"threadId":"high"})
        )["requestIds"],
        json!([])
    );
}

#[test]
fn mcp_steers_native_turn_and_promotes_queued_message_without_replay() {
    use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
    };
    struct Peer(Arc<Mutex<Vec<String>>>);
    impl Provider for Peer {
        fn execute<'a>(
            &'a self,
            input: RunInput,
            output: tokio::sync::mpsc::Sender<ProviderEvent>,
            mut controls: tokio::sync::mpsc::Receiver<ProviderControl>,
            mut cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async move {
                output
                    .send(ProviderEvent::Native(
                        json!({"type":"session","session_id":format!("native-{}",input.run_id)}),
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
                output
                    .send(ProviderEvent::Started)
                    .await
                    .map_err(|e| e.to_string())?;
                loop {
                    tokio::select! {
                        c=controls.recv()=>match c {
                            Some(ProviderControl::SteerContent(content))=>self.0.lock().unwrap().push(content[0]["text"].as_str().unwrap().to_owned()),
                            Some(ProviderControl::Interrupt)|None=>{output.send(ProviderEvent::Interrupted).await.map_err(|e|e.to_string())?;break;},
                            _=>{},
                        },
                        _=cancel.changed()=>break,
                    }
                }
                Ok(())
            })
        }
    }
    let (s, _) = control_fixture();
    let received = Arc::new(Mutex::new(Vec::new()));
    *s.state.provider.lock().unwrap() = Some(Arc::new(Peer(received.clone())));
    let token = s
        .state
        .issue_mcp_session("full-access", 60_000, "steer")
        .unwrap();
    let id = init(&s, &token);
    assert_eq!(
        tool_error(
            &s,
            &token,
            &id,
            "t3_thread_send",
            json!({"threadId":"high","message":"not queued","mode":"steer"})
        )["code"],
        "invalid_request"
    );
    let sent = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"start"}),
    );
    assert!(support::wait_for(std::time::Duration::from_secs(5), || {
        s.state.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='providerTurns' AND json_extract(body,'$.status')='running'",[],|r|r.get::<_,i64>(0)).unwrap()==1
    }));
    let args = json!({"threadId":"high","message":"auto steering","clientRequestId":"steer-once"});
    let steered = call(&s, &token, &id, "t3_thread_send", args.clone());
    assert_eq!(steered["delivery"], "steered");
    assert_eq!(steered["runId"], sent["runId"]);
    assert_eq!(steered, call(&s, &token, &id, "t3_thread_send", args));
    let queued = call(
        &s,
        &token,
        &id,
        "t3_thread_send",
        json!({"threadId":"high","message":"queued steering","mode":"queue"}),
    );
    call(
        &s,
        &token,
        &id,
        "t3_queue_promote_to_steer",
        json!({"threadId":"high","queuedRunId":queued["runId"],"targetRunId":sent["runId"]}),
    );
    assert!(support::wait_for(
        std::time::Duration::from_secs(5),
        || received.lock().unwrap().len() == 2
    ));
    assert_eq!(
        *received.lock().unwrap(),
        vec!["auto steering".to_owned(), "queued steering".to_owned()]
    );
    let stop = json!({"threadId":"high","runId":sent["runId"],"clientRequestId":"interrupt-once"});
    let requested = call(&s, &token, &id, "t3_thread_interrupt", stop.clone());
    assert_eq!(requested["status"], "interrupt_requested");
    assert_eq!(
        call(
            &s,
            &token,
            &id,
            "t3_thread_wait",
            json!({"threadId":"high","runId":sent["runId"],"timeoutMs":5000})
        )["status"],
        "interrupted"
    );
    let replay = call(&s, &token, &id, "t3_thread_interrupt", stop);
    assert_eq!(replay["runId"], requested["runId"]);
    assert_eq!(replay["status"], "interrupted");
}
