mod common;
mod support;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

struct Mcp<'a> {
    server: &'a support::Server,
    token: String,
    id: String,
}
impl<'a> Mcp<'a> {
    fn with_token(server: &'a support::Server, token: String) -> Self {
        let auth = format!("Bearer {token}");
        let (code,h,body)=common::http(&server.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json")],json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"bot-inbox-test","version":"1"}}}).to_string().as_bytes());
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
        let id = h
            .lines()
            .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
            .unwrap()
            .into();
        Self { server, token, id }
    }
    fn operator(server: &'a support::Server, access: &str) -> Self {
        let token = server
            .state
            .issue_mcp_session(access, 60_000, "operator")
            .unwrap();
        Self::with_token(server, token)
    }
    fn bot(server: &'a support::Server, bot: &str) -> Self {
        let token = server
            .state
            .issue_mcp_session_for("full-access", 60_000, "bot", Some(bot))
            .unwrap();
        Self::with_token(server, token)
    }
    fn reply(&self, name: &str, args: Value) -> Value {
        let auth = format!("Bearer {}", self.token);
        let (code,_,body)=common::http(&self.server.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json"),("mcp-session-id",&self.id)],json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string().as_bytes());
        assert_eq!(code, 200);
        serde_json::from_slice(&body).unwrap()
    }
    fn ok(&self, name: &str, args: Value) -> Value {
        let v = self.reply(name, args);
        assert_eq!(v["result"]["isError"], false, "{v}");
        v["result"]["structuredContent"].clone()
    }
    fn denied(&self, name: &str, args: Value, code: &str) {
        let v = self.reply(name, args);
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["structuredContent"]["code"], code, "{v}");
    }
    fn invalid(&self, name: &str, args: Value) {
        let v = self.reply(name, args);
        assert_eq!(v["error"]["code"], -32602, "{v}");
    }
}

fn fixture() -> (support::Server, t3_protocol::Connection) {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(std::sync::Arc::new(o3_server::provider::ScriptedProvider))
    });
    let c = s.connect();
    let root = s.dir.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Inbox","workspaceRoot":root,"scripts":[]})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create-t","threadId":"t","projectId":"p","title":"Requester","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    (s, c)
}

fn user_messages(m: &Mcp, thread: &str) -> Vec<String> {
    let read = m.ok("t3_thread_read", json!({"threadId":thread}));
    read["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["type"] == "user_message")
        .filter_map(|v| v["text"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn post_wakes_a_waiting_bot_and_its_reply_reaches_the_thread_exactly_once() {
    let (s, _c) = fixture();
    let operator = Mcp::operator(&s, "full-access");
    let bot = Mcp::bot(&s, "dot");

    let started = Instant::now();
    let empty = bot.ok("t3_bot_inbox_wait", json!({"timeoutMs":300}));
    assert_eq!(empty["timedOut"], true);
    assert_eq!(empty["messages"], json!([]));
    assert!(started.elapsed() >= Duration::from_millis(250));

    // Posting never starts a turn in the posting thread.
    let (woken, posted) = std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            let t = Instant::now();
            (bot.ok("t3_bot_inbox_wait", json!({"timeoutMs":20000})), t)
        });
        std::thread::sleep(Duration::from_millis(400));
        let posted = operator.ok(
            "t3_bot_inbox_post",
            json!({"botId":"dot","threadId":"t","body":"What is the invoice number?","correlationId":"sodexo-1"}),
        );
        let (woken, t) = waiter.join().unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the long poll woke on the post"
        );
        (woken, posted)
    });
    assert_eq!(woken["timedOut"], false);
    let message = &woken["messages"][0];
    assert_eq!(message["id"], posted["messageId"]);
    assert_eq!(message["direction"], "to_bot");
    assert_eq!(message["threadId"], "t");
    assert_eq!(message["correlationId"], "sodexo-1");
    assert_eq!(message["body"], "What is the invoice number?");
    assert!(
        user_messages(&operator, "t").is_empty(),
        "posting starts no turn"
    );

    // Delivery is at-least-once until acknowledged; afterCursor skips what was seen.
    let cursor = woken["nextCursor"].clone();
    let again = bot.ok("t3_bot_inbox_wait", json!({"timeoutMs":0}));
    assert_eq!(again["messages"].as_array().unwrap().len(), 1);
    let seen = bot.ok(
        "t3_bot_inbox_wait",
        json!({"timeoutMs":0,"afterCursor":cursor}),
    );
    assert_eq!(seen["messages"], json!([]));

    let answer =
        json!({"messageId":posted["messageId"],"body":"INV-4471","clientRequestId":"try-1"});
    let first = bot.ok("t3_bot_inbox_reply", answer.clone());
    let second = bot.ok("t3_bot_inbox_reply", answer);
    assert_eq!(first["messageId"], second["messageId"]);
    assert_eq!(first["correlationId"], "sodexo-1");
    let delivered = user_messages(&operator, "t");
    assert_eq!(
        delivered.len(),
        1,
        "a retried reply is delivered once: {delivered:?}"
    );
    assert!(delivered[0].contains("INV-4471") && delivered[0].contains("sodexo-1"));
    assert_eq!(
        bot.ok("t3_bot_inbox_wait", json!({"timeoutMs":0}))["messages"],
        json!([]),
        "a reply acknowledges the request"
    );

    let log = operator.ok("t3_bot_inbox_read", json!({"botId":"dot"}));
    let directions: Vec<_> = log["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["direction"].clone())
        .collect();
    assert_eq!(directions, [json!("to_bot"), json!("from_bot")]);
    assert_eq!(log["messages"][1]["replyTo"], posted["messageId"]);
    assert!(log["messages"][0]["ackedAt"].is_string());

    // Explicit acknowledgement for requests that need no answer.
    let quiet = operator.ok(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"fyi"}),
    );
    assert_eq!(
        bot.ok(
            "t3_bot_inbox_ack",
            json!({"messageIds":[quiet["messageId"]]})
        )["acked"],
        1
    );
    assert_eq!(
        bot.ok(
            "t3_bot_inbox_ack",
            json!({"messageIds":[quiet["messageId"]]})
        )["acked"],
        0
    );
}

#[test]
fn mailboxes_are_isolated_and_scopes_are_enforced() {
    let (s, _c) = fixture();
    let operator = Mcp::operator(&s, "full-access");
    let dot = Mcp::bot(&s, "dot");
    let other = Mcp::bot(&s, "other");
    let reader = Mcp::operator(&s, "read-only");
    let posted = operator.ok(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"secret"}),
    );

    // A bound credential cannot name, read, acknowledge or answer another mailbox.
    for tool in ["t3_bot_inbox_wait", "t3_bot_inbox_read"] {
        other.denied(
            tool,
            json!({"botId":"dot","timeoutMs":0}),
            "capability_denied",
        );
        assert_eq!(
            other.ok(tool, json!({"timeoutMs":0}))["messages"],
            json!([])
        );
    }
    other.denied(
        "t3_bot_inbox_ack",
        json!({"botId":"dot","messageIds":[posted["messageId"]]}),
        "capability_denied",
    );
    other.denied(
        "t3_bot_inbox_reply",
        json!({"messageId":posted["messageId"],"body":"no"}),
        "invalid_request",
    );
    assert_eq!(
        dot.ok("t3_bot_inbox_read", json!({}))["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A bot answers; it does not address mailboxes.
    dot.denied(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"loop"}),
        "capability_denied",
    );

    // Read-only credentials, bound or not, cannot use the inbox as operators.
    reader.denied(
        "t3_bot_inbox_read",
        json!({"botId":"dot"}),
        "capability_denied",
    );
    reader.denied(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"x"}),
        "capability_denied",
    );

    // Operators address mailboxes explicitly and only existing threads.
    operator.denied("t3_bot_inbox_read", json!({}), "target_required");
    operator.denied(
        "t3_bot_inbox_post",
        json!({"botId":"dot","body":"x"}),
        "target_required",
    );
    operator.denied(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"missing","body":"x"}),
        "thread_not_found",
    );

    // Bounds are enforced before any work.
    operator.invalid(
        "t3_bot_inbox_post",
        json!({"botId":"Dot","threadId":"t","body":"x"}),
    );
    operator.invalid(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"x".repeat(20_001)}),
    );
    operator.invalid(
        "t3_bot_inbox_post",
        json!({"botId":"dot","threadId":"t","body":"   "}),
    );
    dot.invalid("t3_bot_inbox_ack", json!({"messageIds":[]}));

    // Revoking the credential ends access immediately.
    let claims = s.state.mcp_session(&dot.token).unwrap();
    s.state
        .revoke_session(claims["sid"].as_str().unwrap())
        .unwrap();
    let auth = format!("Bearer {}", dot.token);
    let (code, ..) = common::http(
        &s.base,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-session-id", &dot.id),
        ],
        json!({"jsonrpc":"2.0","id":3,"method":"ping"})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(code, 401);
}

#[test]
fn revoking_a_credential_ends_its_open_long_poll() {
    let (s, _c) = fixture();
    let dot = Mcp::bot(&s, "dot");
    let sid = s.state.mcp_session(&dot.token).unwrap()["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        let waiter = scope.spawn(|| dot.reply("t3_bot_inbox_wait", json!({"timeoutMs":30000})));
        std::thread::sleep(Duration::from_millis(400));
        s.state.revoke_session(&sid).unwrap();
        // Any wake-up re-checks the credential.
        s.state.notify();
        waiter.join().unwrap()
    });
    assert_eq!(result["result"]["isError"], true, "{result}");
    assert_eq!(
        result["result"]["structuredContent"]["code"],
        "capability_denied"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn cli_binds_a_credential_to_one_bot_and_rejects_bad_names() {
    let s = support::start(|_| {});
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_o3-server"))
            .env("T3CODE_HOME", &s.dir)
            .args(args)
            .output()
            .unwrap()
    };
    let bad = run(&[
        "auth", "mcp", "issue", "--access", "auto", "--bot-id", "Bad Name",
    ]);
    assert!(!bad.status.success());
    let issued = run(&[
        "auth",
        "mcp",
        "issue",
        "--access",
        "auto",
        "--bot-id",
        "dot",
        "--token-only",
    ]);
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let token = String::from_utf8(issued.stdout).unwrap();
    let cli_state = o3_server::store::State::open(&s.dir.join("userdata")).unwrap();
    assert!(cli_state.mcp_session(token.trim()).is_some());
}

#[test]
fn the_bot_inbox_tools_are_advertised_with_their_limits() {
    let s = support::start(|_| {});
    let m = Mcp::operator(&s, "read-only");
    let auth = format!("Bearer {}", m.token);
    let (_, _, body) = common::http(
        &s.base,
        "POST",
        "/mcp",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-session-id", &m.id),
        ],
        json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})
            .to_string()
            .as_bytes(),
    );
    let v: Value = serde_json::from_slice(&body).unwrap();
    let tools = v["result"]["tools"].as_array().unwrap();
    for name in [
        "t3_bot_inbox_post",
        "t3_bot_inbox_wait",
        "t3_bot_inbox_read",
        "t3_bot_inbox_ack",
        "t3_bot_inbox_reply",
    ] {
        assert!(tools.iter().any(|t| t["name"] == name), "{name}");
    }
}
