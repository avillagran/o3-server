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
        json!({"threadId":"a","limit":1,"maxCharsPerItem":3}),
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
