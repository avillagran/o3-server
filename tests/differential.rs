//! Opt-in upstream comparison. Run tools/conformance-diff; never uses an existing profile.
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use t3_protocol::{Connection, Credential};

struct Server {
    child: Child,
    base: String,
    token: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        // Only the process group created by this harness, including provider children.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn command(bin: &Path, home: &Path) -> Command {
    let mut c = Command::new("nice");
    // No inherited provider credentials, profile overrides, or service integration.
    let node_output = Command::new("node")
        .args(["-p", "process.execPath"])
        .output()
        .expect("node runtime required");
    assert!(node_output.status.success());
    let node = PathBuf::from(String::from_utf8(node_output.stdout).unwrap().trim());
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        home.join("bin").display(),
        node.parent().unwrap().display()
    );
    c.env_clear()
        .env("PATH", path)
        .env("HOME", home.join("home"))
        .env("T3CODE_HOME", home)
        .env("TMPDIR", "/var/tmp/o3-build/tmp")
        .env("O3_USAGE_RATES_URL", "http://127.0.0.1:0/rates")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg(bin)
        .current_dir(home)
        .stdin(Stdio::null());
    c
}
fn cli_output(c: &mut Command) -> std::process::Output {
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}
fn start(bin: &Path, home: &Path) -> Server {
    for dir in [home.to_path_buf(), home.join("home"), home.join("run")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let output = cli_output(command(bin, home).args([
        "auth",
        "session",
        "issue",
        "--subject",
        "conformance",
        "--ttl",
        "1h",
        "--token-only",
    ]));
    assert!(
        output.status.success(),
        "issue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let token = String::from_utf8(output.stdout).unwrap().trim().to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let log = std::fs::File::create(home.join("server.log")).unwrap();
    let child = command(bin, home)
        .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .process_group(0)
        .spawn()
        .unwrap();
    let mut s = Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
        token,
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if t3_protocol::fetch_descriptor(&s.base).is_ok() {
            break;
        }
        assert!(
            s.child.try_wait().unwrap().is_none(),
            "server exited; see {}",
            home.join("server.log").display()
        );
        assert!(
            Instant::now() < deadline,
            "startup timed out: {}",
            home.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    s
}
fn outcome(r: t3_protocol::Result<Value>) -> Value {
    match r {
        Ok(v) => json!({"ok":v}),
        Err(e) => {
            json!({"error":e.to_string(),"errorCode":t3_protocol::error_code(&e.to_string())})
        }
    }
}
// Preserve literal discriminators and status values. Other scalar values become types;
// absent fields, nulls, array length and stream order remain observable.
fn shape(v: &Value, key: &str) -> Value {
    match v {
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), shape(v, k))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| shape(v, key)).collect()),
        Value::Null => json!({"$type":"null"}),
        Value::Bool(v) => json!({"$type":"boolean","value":v}),
        Value::Number(n)
            if [
                "statusCode",
                "messageCount",
                "queueCount",
                "activityCount",
                "latestLocalTurnOrdinal",
                "ordinal",
                "totalMatches",
                "activeIndex",
                "occurrence",
                "startIndex",
                "count",
            ]
            .contains(&key) =>
        {
            json!({"$type":"number","value":n})
        }
        Value::Number(_) => json!({"$type":"number"}),
        Value::String(s)
            if [
                "_tag",
                "inputIntent",
                "role",
                "instanceId",
                "driver",
                "kind",
                "type",
                "status",
                "errorTag",
                "errorCode",
                "statusCode",
            ]
            .contains(&key) =>
        {
            json!({"$type":"string","value":s})
        }
        Value::String(_) => json!({"$type":"string"}),
    }
}
fn stream(c: &Connection, tag: &str, p: Value, n: usize) -> Value {
    let done = std::cell::Cell::new(false);
    let mut items = Vec::new();
    let mut reader_views = Vec::new();
    let mut mirror = json!({});
    let start = Instant::now();
    let result = c.subscribe(
        tag,
        p,
        || done.get() || start.elapsed() > Duration::from_secs(5),
        |v| {
            if let Some(v) = v {
                if tag == "orchestration.subscribeThread" {
                    if v["kind"] == "snapshot" || v["type"] == "snapshot" {
                        mirror = v["projection"].clone();
                    } else if v["event"].is_object() {
                        t3_protocol::projection::apply(&mut mirror, &v["event"]);
                    }
                    if mirror["thread"].is_object() {
                        let view = t3_protocol::timeline::view(
                            &mirror,
                            &json!({}),
                            &t3_protocol::timeline::Render::neutral(false),
                        )
                        .expect("real server projection must render");
                        let queue = t3_protocol::queue::derive(&mirror);
                        let checks = json!({"working":view["threadState"]["working"],"messageCount":view["messages"].as_array().map(Vec::len),"queueCount":queue["items"].as_array().map(Vec::len),"activityCount":view["activity"].as_array().map(Vec::len),"fixtureTextFound":view["messages"].as_array().is_some_and(|m| m.iter().any(|m|m["text"] == "Conformance fixture Conformance"))});
                        reader_views.push(json!({"timeline":view,"queue":queue,"checks":checks}));
                    }
                }
                items.push(v);
                if items.len() >= n {
                    done.set(true);
                }
            }
            Ok(())
        },
    );
    json!({"items":items,"readerViews":reader_views,"error":result.err().map(|e|e.to_string())})
}
// Supplement client-visible failures with their original Effect error tags.
fn raw_rpc(s: &Server, tag: &str, payload: Value) -> Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let addr = s.base.trim_start_matches("http://");
            let mut request = format!("ws://{addr}/ws?orchestrationProtocol=2")
                .into_client_request()
                .unwrap();
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {}", s.token).parse().unwrap(),
            );
            let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (mut ws, _) = tokio_tungstenite::client_async(request, tcp).await.unwrap();
            ws.send(Message::Text(
                json!({"_tag":"Request","id":"1","tag":tag,"payload":payload,"headers":[]})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(Ok(message)) = ws.next().await {
                    if let Ok(text) = message.to_text() {
                        if let Ok(value) = serde_json::from_str::<Value>(text) {
                            return value;
                        }
                    }
                }
                json!({"transport":"closed"})
            })
            .await
            .unwrap_or(json!({"transport":"timeout"}));
            let _ = ws.close(None).await;
            reply
        })
}
fn http_status(s: &Server, path: &str, protocol: bool, auth: bool) -> Value {
    use std::io::{Read, Write};
    let mut tcp = std::net::TcpStream::connect(s.base.trim_start_matches("http://")).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if protocol {
        request.push_str("x-t3-orchestration-protocol: 2\r\n");
    }
    if auth {
        request.push_str(&format!("Authorization: Bearer {}\r\n", s.token));
    }
    request.push_str("\r\n");
    tcp.write_all(request.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    let _ = tcp.take(1024 * 1024).read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    json!({"statusCode":text.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("")})
}
fn script(s: &Server, workspace: &Path) -> BTreeMap<String, Value> {
    let c = Connection::new(&s.base, None, Credential::bearer(&s.token).unwrap()).unwrap();
    let mut rows = BTreeMap::new();
    if let Ok(config) = c.rpc("server.getConfig", json!({})) {
        rows.insert("provider identities and model slugs".into(),json!(config["providers"].as_array().unwrap().iter().map(|p|json!({"instanceId":p["instanceId"],"driver":p["driver"],"models":p["models"].as_array().unwrap_or(&vec![]).iter().map(|m|m["slug"].clone()).collect::<Vec<_>>()})).collect::<Vec<_>>()));
        eprintln!(
            "provider identities: {}",
            config["providers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| format!("{}:{}", p["instanceId"], p["driver"]))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    for (path, protocol, auth) in [
        ("/.well-known/t3/environment", false, false),
        ("/api/auth/session", false, false),
        ("/api/orchestration/shell", false, true),
        ("/api/orchestration/shell", true, true),
        ("/ws?orchestrationProtocol=1", false, true),
        ("/api/assets/forged/file.txt", false, false),
    ] {
        rows.insert(
            format!("status GET {path} protocol={protocol} auth={auth}"),
            http_status(s, path, protocol, auth),
        );
    }
    for path in [
        "/.well-known/t3/environment",
        "/api/auth/session",
        "/api/auth/clients",
        "/api/auth/pairing-links",
        "/api/projects",
        "/api/orchestration/shell",
    ] {
        rows.insert(
            format!("GET {path}"),
            outcome(c.request_json("GET", path, None, path.contains("orchestration"))),
        );
    }
    let grant = c.request_json(
        "POST",
        "/api/auth/pairing-token",
        Some(&json!({"label":"differential"})),
        false,
    );
    if let Ok(g) = &grant {
        if let Some(secret) = g["credential"].as_str() {
            let paired = t3_protocol::pair_bearer(
                &s.base,
                secret,
                None,
                &t3_protocol::PairingClient::default(),
                &t3_protocol::ConnectionOptions::default(),
            );
            rows.insert(
                "POST /oauth/token real pairing".into(),
                json!({"authenticated":paired.is_ok()}),
            );
        }
    }
    rows.insert("POST /api/auth/pairing-token".into(), outcome(grant));
    let upload = c.upload_attachment(
        "fixture.txt",
        "text/plain",
        8,
        std::io::Cursor::new(b"fixture\n"),
    );
    if let Ok(value) = &upload {
        if let Some(id) = value["id"].as_str() {
            let asset=c.rpc("assets.createUrl",json!({"resource":{"_tag":"attachment","attachmentId":id,"fileName":"fixture.txt","mimeType":"text/plain"}}));
            if let Ok(g) = &asset {
                if let Some(url) = g["relativeUrl"].as_str() {
                    rows.insert(
                        "status GET signed attachment".into(),
                        http_status(s, url, false, false),
                    );
                }
            }
            rows.insert("assets.createUrl attachment".into(), outcome(asset));
        }
    }
    rows.insert(
        "POST /api/attachments/upload real client".into(),
        outcome(upload),
    );
    if let Some(binary) = std::env::var_os("O3_DIFF_CLIENT_TEST_BIN") {
        for test in [
            "pairing_is_one_use_and_session_authenticates_rpc",
            "rpc_requires_a_valid_credential",
            "shell_subscription_opens_with_a_snapshot",
        ] {
            let grant = c
                .request_json(
                    "POST",
                    "/api/auth/pairing-token",
                    Some(&json!({"label":"conformance-base"})),
                    false,
                )
                .unwrap();
            let output = cli_output(
                command(Path::new(&binary), workspace)
                    .env("O3_CONFORMANCE_BASE", &s.base)
                    .env(
                        "O3_CONFORMANCE_PAIRING",
                        grant["credential"].as_str().unwrap(),
                    )
                    .args([test, "--exact", "--test-threads=1"]),
            );
            rows.insert(
                format!("O3_CONFORMANCE_BASE {test}"),
                json!({"statusCode":output.status.code()}),
            );
            assert!(
                output.status.success(),
                "conformance-base {test}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
    let mut calls = vec![
        ("server.getConfig", json!({})),
        ("server.getSettings", json!({})),
        (
            "server.updateSettings",
            json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true}}}}),
        ),
        ("server.refreshProviders", json!({})),
        ("server.discoverSourceControl", json!({})),
        (
            "server.reportClientActivity",
            json!({"clientId":"diff","clientKind":"desktop-renderer","visible":true,"focused":true,"recentlyInteracted":true,"appState":"active","observedAt":"2026-10-31T00:00:00.000Z","scopes":[]}),
        ),
        (
            "projects.mutate",
            json!({"type":"project.create","commandId":"project","projectId":"p1","title":"Conformance","workspaceRoot":workspace,"scripts":[]}),
        ),
        (
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","createdBy":"user","creationSource":"web","commandId":"create","threadId":"t1","projectId":"p1","title":"Conformance","modelSelection":{"instanceId":"codex","model":"gpt-5"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null}),
        ),
        (
            "orchestration.dispatchCommand:message.dispatch",
            json!({"type":"message.dispatch","commandId":"message","threadId":"t1","messageId":"m1","text":"Conformance fixture Conformance","createdBy":"user","creationSource":"web","attachments":[],"dispatchMode":{"type":"defer_start"}}),
        ),
        (
            "orchestration.searchThreads",
            json!({"query":"Conformance","titlesOnly":true}),
        ),
        (
            "orchestration.searchThread",
            json!({"threadId":"t1","query":"Conformance"}),
        ),
        (
            "orchestration.getTurnItem",
            json!({"threadId":"t1","itemId":"missing"}),
        ),
        (
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":0,"toTurnCount":0}),
        ),
        (
            "orchestration.getFullThreadDiff",
            json!({"threadId":"t1","toTurnCount":0}),
        ),
        (
            "projects.readFile",
            json!({"cwd":workspace,"relativePath":"fixture.txt"}),
        ),
        (
            "projects.searchEntries",
            json!({"cwd":workspace,"query":"fixture","limit":20}),
        ),
        ("filesystem.browse", json!({"partialPath":workspace})),
        ("vcs.refreshStatus", json!({"cwd":workspace})),
        ("vcs.listRefs", json!({"cwd":workspace})),
        (
            "vcs.createRef",
            json!({"cwd":workspace,"refName":"conformance"}),
        ),
        (
            "vcs.switchRef",
            json!({"cwd":workspace,"refName":"conformance"}),
        ),
        ("vcs.pull", json!({"cwd":workspace})),
        ("review.getDiffPreview", json!({"cwd":workspace})),
        (
            "review.getDiffFileContents",
            json!({"cwd":workspace,"sourceKind":"working-tree","changeType":"new","baseRef":null,"headRef":null,"oldPath":"fixture.txt","newPath":"fixture.txt"}),
        ),
        (
            "secrets.answerRequest",
            json!({"threadId":"t1","turnItemId":"missing","answer":{"type":"decline"}}),
        ),
        (
            "attachments.createUploadUrl",
            json!({"name":"fixture.txt","mimeType":"text/plain","sizeBytes":8,"type":"file"}),
        ),
        (
            "assets.createUrl",
            json!({"resource":{"_tag":"workspace-file","threadId":"t1","path":"fixture.html"}}),
        ),
        (
            "terminal.open",
            json!({"threadId":"t1","terminalId":"term1","cwd":workspace,"cols":80,"rows":24}),
        ),
        (
            "terminal.resize",
            json!({"threadId":"t1","terminalId":"term1","cols":100,"rows":30}),
        ),
        (
            "terminal.write",
            json!({"threadId":"t1","terminalId":"term1","data":"printf 'CONFORMANCE\\n'\n"}),
        ),
    ];
    for (tag, p) in calls.drain(..) {
        let rpc_tag = tag.split(':').next().unwrap();
        let result = c.rpc(rpc_tag, p.clone());
        if let Err(e) = &result {
            eprintln!("{tag}: {e}");
            rows.insert(format!("error-frame {tag}"), raw_rpc(s, rpc_tag, p));
        }
        rows.insert(tag.into(), outcome(result));
    }
    // Registry-driven negative coverage makes newly added P0/P1 commands visible.
    // These missing-run/request probes are rejection evidence, never native execution proof.
    let registry: Value = serde_json::from_str(include_str!("../docs/compat-status.json")).unwrap();
    for (kind, row) in registry["commands"].as_object().unwrap() {
        if !matches!(row["priority"].as_str(), Some("P0" | "P1"))
            || matches!(kind.as_str(), "thread.create" | "message.dispatch")
        {
            continue;
        }
        let mut p = json!({"type":kind,"commandId":format!("probe-{kind}"),"threadId":"absent-thread","runId":"absent-run","requestId":"absent-request","queuedRunId":"absent-run","targetRunId":"absent-run","sourceThreadId":"absent-thread","targetThreadId":"absent-fork","sourcePoint":{"type":"latest_stable"},"createdBy":"user","creationSource":"web","decision":"accept","holdQueue":true,"text":"edited","title":"Conformance","enabled":true,"orderKey":"a","beforeRunId":null,"snoozedUntil":"2099-01-01T00:00:00.000Z","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"codex","model":"gpt-5"}});
        if kind == "thread.metadata.update" {
            p["title"] = json!("Conformance");
        }
        let result = c.rpc("orchestration.dispatchCommand", p.clone());
        rows.insert(format!("command-negative {kind}"), outcome(result));
        rows.insert(
            format!("command-frame {kind}"),
            raw_rpc(s, "orchestration.dispatchCommand", p),
        );
    }
    for (tag, p, n) in [
        (
            "orchestration.subscribeShell",
            json!({"requestCompletionMarker":true}),
            2,
        ),
        (
            "orchestration.subscribeThread",
            json!({"threadId":"t1","acceptBoundedSnapshot":true,"requestCompletionMarker":true}),
            2,
        ),
        ("subscribeServerConfig", json!({}), 1),
        (
            "orchestration.subscribeCommandOutput",
            json!({"threadId":"t1","itemId":"missing"}),
            1,
        ),
        (
            "terminal.attach",
            json!({"threadId":"t1","terminalId":"term1"}),
            1,
        ),
        (
            "git.runStackedAction",
            json!({"cwd":workspace,"action":"commit","commitMessage":"conformance"}),
            1,
        ),
    ] {
        rows.insert(tag.into(), stream(&c, tag, p, n));
    }
    for suffix in ["bounded", "history"] {
        let path = format!("/api/orchestration/threads/t1/{suffix}");
        rows.insert(
            format!("GET {path}"),
            outcome(c.request_json("GET", &path, None, true)),
        );
    }
    c.rpc(
        "terminal.close",
        json!({"threadId":"t1","terminalId":"term1"}),
    )
    .ok();
    // Do not infer coverage of native inference from a rejected mutation.
    rows.insert("orchestration.launchThread".into(),outcome(c.rpc("orchestration.launchThread",json!({"commandId":"launch","projectId":"p1","title":"Empty launch","modelSelection":{"instanceId":"codex","model":"gpt-5"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"}}))));
    rows
}
fn reference_binary() -> PathBuf {
    std::env::var_os("T3_REFERENCE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".t3/runtime/versions/0.1.7-nightly.20261031.3991/t3")
        })
}
#[test]
#[ignore = "starts isolated real T3 and o3; tools/conformance-diff"]
fn upstream_differential() {
    let root = PathBuf::from("/var/tmp/o3-build/tmp/differential")
        .join(format!("run-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let t3 = reference_binary();

    let upstream = start(&t3, &root.join("t3"));
    let o3 = start(Path::new(env!("CARGO_BIN_EXE_o3-server")), &root.join("o3"));
    let children =
        std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", o3.child.id())).unwrap();
    assert!(
        children.trim().is_empty(),
        "o3 startup spawned child: {children}"
    );
    eprintln!("o3 startup: zero child processes");
    let mut results = Vec::new();
    for (name, s) in [("upstream", &upstream), ("o3", &o3)] {
        let workspace = root.join(name).join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("fixture.txt"), "fixture\n").unwrap();
        std::fs::write(workspace.join("fixture.html"), "<p>fixture</p>\n").unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["add", "fixture.txt"],
            vec![
                "-c",
                "user.name=Conformance",
                "-c",
                "user.email=conformance@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&workspace)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success());
        }
        let mut rows = script(s, &workspace);
        let (bin, home) = if name == "upstream" {
            (t3.as_path(), root.join("t3"))
        } else {
            (Path::new(env!("CARGO_BIN_EXE_o3-server")), root.join("o3"))
        };
        let runtime = home.join("userdata/server-runtime.json");
        let port = s.base.rsplit(':').next().unwrap();
        for (label, args) in [
            (
                "auth session list",
                vec!["auth", "session", "list", "--json"],
            ),
            (
                "__ssh-helper runtime-port",
                vec!["__ssh-helper", "runtime-port", runtime.to_str().unwrap()],
            ),
            (
                "__ssh-helper wait-ready",
                vec!["__ssh-helper", "wait-ready", port, "2000", "50"],
            ),
            (
                "auth pairing create",
                vec!["auth", "pairing", "create", "--json"],
            ),
        ] {
            let output = cli_output(command(bin, &home).args(args));
            let stdout = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(json!("text"));
            rows.insert(
                format!("CLI {label}"),
                json!({"statusCode":output.status.code(),"stdout":stdout}),
            );
        }
        // Shapes only: never persist bearer tokens or signed asset URLs.
        results.push(
            rows.iter()
                .map(|(k, v)| (k.clone(), shape(v, "")))
                .collect::<BTreeMap<_, _>>(),
        );
    }
    for result in &results {
        let views = result["orchestration.subscribeThread"]["readerViews"]
            .as_array()
            .expect("reader evidence required");
        assert_eq!(
            views.len(),
            2,
            "snapshot and completion-marker reader coverage required"
        );
    }
    assert_eq!(
        results[0]["orchestration.subscribeThread"]["readerViews"],
        results[1]["orchestration.subscribeThread"]["readerViews"],
        "upstream/o3 client-reader gap"
    );
    let keys: std::collections::BTreeSet<_> =
        results.iter().flat_map(|r| r.keys().cloned()).collect();
    let comparisons:Vec<_>=keys.iter().map(|k| json!({"surface":k,"equal":results[0].get(k)==results[1].get(k),"upstream":results[0].get(k),"o3":results[1].get(k)})).collect();
    let report = json!({"reference":t3.parent().and_then(|p|p.file_name()),"pinnedContract":"3f9448747 / nightly 3726","results":comparisons});
    let path = root.join("report.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("differential evidence: {}", path.display());
    drop(o3);
    drop(upstream);
    for name in ["t3", "o3", "upstream"] {
        let _ = std::fs::remove_dir_all(root.join(name));
    }

    if std::env::var_os("O3_DIFF_STRICT").is_some() {
        assert!(
            comparisons.iter().all(|r| r["equal"] == true),
            "differential mismatches; see {}",
            path.display()
        );
    }
}
#[test]
fn shape_preserves_null_optional_tags_and_order() {
    assert_ne!(shape(&json!({"x":null}), ""), shape(&json!({}), ""));
    assert_ne!(
        shape(&json!([{"kind":"snapshot"},{"kind":"event"}]), ""),
        shape(&json!([{"kind":"event"},{"kind":"snapshot"}]), "")
    );
    assert_eq!(
        shape(&json!({"id":"a","createdAt":"today"}), ""),
        shape(&json!({"id":"b","createdAt":"yesterday"}), "")
    );
}

fn connect(s: &Server) -> Connection {
    Connection::new(&s.base, None, Credential::bearer(&s.token).unwrap()).unwrap()
}
fn first_thread(c: &Connection) -> Value {
    let done = std::cell::Cell::new(false);
    let mut snapshot = Value::Null;
    c.subscribe(
        "orchestration.subscribeThread",
        json!({"threadId":"concurrent"}),
        || done.get(),
        |v| {
            if let Some(v) = v {
                snapshot = v;
                done.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(snapshot["kind"], "snapshot");
    snapshot
}
fn reader_outputs(p: &Value) -> Value {
    json!({"timeline":t3_protocol::timeline::view(p, &json!({}), &t3_protocol::timeline::Render::neutral(false)).unwrap(),"queue":t3_protocol::queue::derive(p),"approvals":t3_protocol::approvals::pending(p),"compaction":t3_protocol::compaction::candidate(p,&json!({}),4102444800000)})
}
struct Observer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    rx: std::sync::mpsc::Receiver<Value>,
    join: std::thread::JoinHandle<(Value, u64, Vec<Value>)>,
}
fn observe(s: &Server) -> Observer {
    let c = connect(s);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || {
        let mut p = Value::Null;
        let mut seq = 0;
        let mut boundary = 0;
        let mut tail = Vec::new();
        let mut records = std::collections::BTreeSet::new();
        let mut terminal_records = std::collections::BTreeSet::new();
        let deadline = Instant::now() + Duration::from_secs(90);
        c.subscribe(
            "orchestration.subscribeThread",
            json!({"threadId":"concurrent"}),
            || stopped.load(std::sync::atomic::Ordering::Relaxed) || Instant::now() > deadline,
            |v| {
                if let Some(v) = v {
                    if p.is_null() {
                        assert_eq!(v["kind"], "snapshot", "snapshot must precede tail");
                        p = v["projection"].clone();
                        seq = t3_protocol::stream::cursor_of("orchestration.subscribeThread", &v)
                            .unwrap();
                        boundary = seq;
                        for kind in [
                            "runs",
                            "messages",
                            "providerThreads",
                            "providerTurns",
                            "nodes",
                            "attempts",
                            "turnItems",
                        ] {
                            for r in p[kind].as_array().into_iter().flatten() {
                                records.insert((kind.to_owned(), r["id"].to_string()));
                            }
                        }
                    } else if v["event"].is_object() {
                        let n = t3_protocol::stream::cursor_of("orchestration.subscribeThread", &v)
                            .unwrap();
                        assert!(n > seq, "nonmonotonic or duplicated sequence");
                        seq = n;
                        tail.push(v.clone());
                        let e = &v["event"];
                        let typ = e["type"].as_str().unwrap();
                        if typ == "run.created" {
                            let key = ("runs".to_owned(), e["payload"]["id"].to_string());
                            assert!(
                                records.insert(key.clone()),
                                "duplicated record creation: {key:?}, {}",
                                shape(e, "")
                            );
                        }
                        if matches!(
                            typ,
                            "run.updated"
                                | "node.updated"
                                | "run-attempt.updated"
                                | "provider-turn.updated"
                        ) {
                            let key = (typ.to_owned(), e["payload"]["id"].to_string());
                            let terminal = matches!(
                                e["payload"]["status"].as_str(),
                                Some("completed" | "failed" | "interrupted" | "cancelled")
                            );
                            assert!(
                                !terminal_records.contains(&key) || terminal,
                                "terminal record became active: {e}"
                            );
                            if terminal {
                                terminal_records.insert(key);
                            }
                        }
                        assert!(
                            t3_protocol::projection::apply(&mut p, e),
                            "unapplied event: {typ}"
                        );
                    }
                    // Execute actual readers throughout the stream, including mid-flight queue state.
                    reader_outputs(&p);
                    tx.send(p.clone()).unwrap();
                }
                Ok(())
            },
        )
        .unwrap();
        (p, boundary, tail)
    });
    let observer = Observer { stop, rx, join };
    observer.rx.recv_timeout(Duration::from_secs(20)).unwrap();
    observer
}
fn wait_projection(c: &Connection, pred: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(40);
    loop {
        let p = c
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":"concurrent"}),
            )
            .unwrap();
        if pred(&p) {
            return p;
        }
        assert!(
            Instant::now() < end,
            "provider did not reach requested state: {}",
            shape(&p, "")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn dispatch_command(c: &Connection, mut v: Value) {
    if matches!(
        v["type"].as_str(),
        Some("thread.create" | "message.dispatch")
    ) {
        v["createdBy"] = json!("user");
        v["creationSource"] = json!("web");
    }
    let kind = v["type"].clone();
    c.rpc("orchestration.dispatchCommand", v)
        .unwrap_or_else(|e| panic!("{kind}: {e}"));
}
fn verify_observers(
    s: &Server,
    observers: [Observer; 2],
    view: &Value,
    reference: bool,
) -> (Vec<Value>, usize) {
    let mut replay_differences = Vec::new();
    let mut ignored_reference_replay = 0;
    for obs in observers {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let p = obs
                .rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap();
            if reader_outputs(&p) == *view {
                break;
            }
        }
        eprintln!("observer converged");
        obs.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let (reduced, boundary, tail) = obs.join.join().unwrap();
        eprintln!("observer joined: tail={}", tail.len());
        if !tail.is_empty() {
            let done = std::cell::Cell::new(false);
            let mut replay = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(15);
            connect(s)
                .subscribe(
                    "orchestration.subscribeThread",
                    json!({"threadId":"concurrent","afterSequence":boundary,"requestCompletionMarker":true}),
                    || done.get() || Instant::now() > deadline,
                    |v| {
                        if let Some(v) = v {
                            if v["kind"] == "synchronized" { done.set(true); return Ok(()); }
                            replay.push(v);
                            if replay.len() == tail.len() {
                                done.set(true);
                            }
                        }
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(replay.len(), tail.len(), "replay event count differs");
            for (replayed, observed) in replay.iter().zip(&tail) {
                assert_eq!(
                    replayed["sequence"], observed["sequence"],
                    "replay reordered sequence"
                );
                assert_eq!(
                    replayed["event"]["id"], observed["event"]["id"],
                    "replay changed identity"
                );
                ignored_reference_replay += replay_differences_checked(
                    replayed,
                    observed,
                    reference,
                    &mut replay_differences,
                );
            }
        }
        assert_eq!(
            reader_outputs(&reduced),
            *view,
            "subscriber differs from fresh snapshot"
        );
    }
    (replay_differences, ignored_reference_replay)
}
fn concurrent_scenario(s: &Server, home: &Path, reference: bool) -> Value {
    let c = connect(s);
    c.rpc("server.updateSettings",json!({"patch":{"textGenerationModelSelection":{"instanceId":"unavailable","model":"fixture"}}})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"fixture-project","projectId":"fixture-project","title":"Fixture","workspaceRoot":home,"scripts":[]})).unwrap();
    dispatch_command(
        &c,
        json!({"type":"thread.create","commandId":"create-concurrent","threadId":"concurrent","projectId":"fixture-project","title":"Concurrent fixture","modelSelection":{"instanceId":"codex","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null}),
    );
    let a = observe(s);
    dispatch_command(
        &c,
        json!({"type":"message.dispatch","commandId":"first","threadId":"concurrent","messageId":"first","text":"first","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
    );
    eprintln!("first dispatched");
    let end = Instant::now() + Duration::from_secs(30);
    while !home.join("started-1").exists() {
        assert!(
            Instant::now() < end,
            "fake provider did not start; inspect isolated server.log and peer-methods.log"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let p = wait_projection(&c, |p| {
        p["runs"]
            .as_array()
            .is_some_and(|rs| rs.iter().any(|r| r["status"] == "running"))
    });
    let run = p["runs"][0]["id"].clone();
    std::thread::scope(|scope| {
        let mut joins = vec![];
        for i in 0..3 {
            let c = connect(s);
            joins.push(scope.spawn(move || dispatch_command(&c,json!({"type":"message.dispatch","commandId":format!("queued-{i}"),"threadId":"concurrent","messageId":format!("queued-{i}"),"text":format!("queued-{i}"),"attachments":[],"dispatchMode":{"type":"queue_after_active"}}))));
        }
        let c = connect(s);
        joins.push(scope.spawn(move || {
            dispatch_command(
                &c,
                json!({"type":"thread.pin","commandId":"pin-concurrent","threadId":"concurrent"}),
            )
        }));
        for j in joins {
            j.join().unwrap();
        }
    });
    eprintln!("concurrent commands received");
    let b = observe(s);
    dispatch_command(
        &c,
        json!({"type":"run.interrupt","commandId":"interrupt-concurrent","threadId":"concurrent","runId":run,"holdQueue":true}),
    );
    let requested = wait_projection(&c, |p| {
        p["turnItems"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["type"] == "run_interrupt_request")
    });
    let mid = reader_outputs(&requested);
    assert_eq!(mid["timeline"]["threadState"]["working"], true);
    assert_eq!(mid["queue"]["canSteer"], true);
    assert_eq!(mid["queue"]["activeRunId"], run);
    assert!(!requested["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "run_interrupt_result"));
    std::fs::write(home.join("release-interrupt"), "").unwrap();
    wait_projection(&c, |p| {
        reader_outputs(p)["timeline"]["threadState"]["working"] == false
    });
    dispatch_command(
        &c,
        json!({"type":"queue.resume","commandId":"resume-concurrent","threadId":"concurrent"}),
    );
    for n in 1..=4 {
        std::fs::write(home.join(format!("release-{n}")), "").unwrap();
    }
    wait_projection(&c, |p| {
        p["runs"].as_array().is_some_and(|rs| {
            rs.len() == 4
                && rs
                    .iter()
                    .all(|r| matches!(r["status"].as_str(), Some("completed" | "interrupted")))
        })
    });
    // Keep each complete replay range below the reference's replay cap.
    let before = reader_outputs(&first_thread(&c)["projection"]);
    let (mut replay_differences, mut ignored_reference_replay) =
        verify_observers(s, [a, b], &before, reference);
    let a = observe(s);
    let b = observe(s);
    {
        let mode = "approval";
        std::fs::write(home.join(format!("{mode}-mode")), "").unwrap();
        dispatch_command(
            &c,
            json!({"type":"message.dispatch","commandId":format!("{mode}-message"),"messageId":format!("{mode}-message"),"threadId":"concurrent","text":mode,"attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        let pending = wait_projection(&c, |p| {
            if mode == "approval" {
                !t3_protocol::approvals::pending(p)
                    .as_array()
                    .unwrap()
                    .is_empty()
            } else {
                home.join("background-result").exists()
                    && p["providerThreads"].as_array().unwrap().iter().any(|r| {
                        r["pendingBackgroundTasks"]
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                    })
            }
        });
        if mode == "approval" {
            let rows = t3_protocol::approvals::pending(&pending);
            assert_eq!(rows[0]["responseCapability"], "live");
            for obs in [&a, &b] {
                let end = Instant::now() + Duration::from_secs(20);
                loop {
                    let reduced = obs
                        .rx
                        .recv_timeout(end.saturating_duration_since(Instant::now()))
                        .unwrap();
                    if t3_protocol::approvals::pending(&reduced) == rows {
                        assert_eq!(
                            reader_outputs(&reduced)["approvals"],
                            reader_outputs(&pending)["approvals"]
                        );
                        t3_protocol::approvals::command(&json!({"thread":"concurrent","runtimeRequestId":rows[0]["requestId"],"decision":"cancel"}), &reduced).unwrap();
                        break;
                    }
                }
            }
            t3_protocol::approvals::command(&json!({"thread":"concurrent","runtimeRequestId":rows[0]["requestId"],"decision":"cancel"}), &pending).unwrap();
        }
        dispatch_command(
            &c,
            json!({"type":"thread.stop","commandId":format!("{mode}-stop"),"threadId":"concurrent"}),
        );
        wait_projection(&c, |p| {
            !p["runs"].as_array().unwrap().iter().any(|r| {
                matches!(
                    r["status"].as_str(),
                    Some("running" | "waiting" | "starting" | "preparing")
                )
            })
        });
        let stopped = first_thread(&c)["projection"].clone();
        if mode == "approval" {
            assert_eq!(
                t3_protocol::approvals::pending(&stopped)[0]["responseCapability"],
                "live"
            );
        }
        std::fs::remove_file(home.join(format!("{mode}-mode"))).unwrap();
        dispatch_command(
            &c,
            json!({"type":"queue.resume","commandId":format!("{mode}-resume"),"threadId":"concurrent"}),
        );
    }
    dispatch_command(
        &c,
        json!({"type":"thread.unsettle","commandId":"unsettle-final","threadId":"concurrent","reason":"user"}),
    );
    dispatch_command(
        &c,
        json!({"type":"thread.stop","commandId":"stop-idle","threadId":"concurrent"}),
    );
    // Observe original Effect exits rather than the client's flattened errors.
    dispatch_command(
        &c,
        json!({"type":"thread.create","commandId":"create-other","threadId":"other","projectId":"fixture-project","title":"Other","modelSelection":{"instanceId":"codex","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null}),
    );
    let mut dispatch_frames = serde_json::Map::new();
    for (name, thread, rid) in [
        ("terminal", "concurrent", run.clone()),
        ("unknown", "concurrent", json!("unknown")),
        ("wrong-thread", "other", run.clone()),
    ] {
        let frame = raw_rpc(
            s,
            "orchestration.dispatchCommand",
            json!({"type":"run.interrupt","commandId":format!("reject-{name}"),"threadId":thread,"runId":rid}),
        );
        dispatch_frames.insert(name.into(), frame);
    }
    for kind in ["run.interrupt", "thread.stop"] {
        dispatch_frames.insert(format!("missing-thread-{kind}"), raw_rpc(s,"orchestration.dispatchCommand",json!({"type":kind,"commandId":format!("missing-{kind}"),"threadId":"missing","runId":"unknown"})));
    }

    dispatch_command(
        &c,
        json!({"type":"message.dispatch","commandId":"unowned-message","messageId":"unowned-message","threadId":"other","text":"deferred","attachments":[],"dispatchMode":{"type":"defer_start"}}),
    );
    let unowned = c
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"other"}),
        )
        .unwrap();
    let unowned_run = unowned["runs"][0]["id"].as_str().unwrap();
    let owner = unowned["runs"][0]["providerThreadId"].as_str().unwrap();
    // Fault injection is restricted to this test's isolated database. Remove
    // durable ownership from a deferred run; no live native turn is involved.
    let db = rusqlite::Connection::open(home.join(if reference {
        "userdata/statev2.sqlite"
    } else {
        "userdata/o3.sqlite"
    }))
    .unwrap();
    if reference {
        db.execute(
            "DELETE FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?",
            [owner],
        )
        .unwrap();
    } else {
        db.execute(
            "DELETE FROM records WHERE kind='providerThreads' AND id=?",
            [owner],
        )
        .unwrap();
    }
    drop(db);
    let missing_owner = raw_rpc(
        s,
        "orchestration.dispatchCommand",
        json!({"type":"run.interrupt","commandId":"missing-owner","threadId":"other","runId":unowned_run}),
    );
    dispatch_frames.insert(
        "missing-owner".into(),
        serde_json::from_str(
            &missing_owner
                .to_string()
                .replace(unowned_run, "unowned-run"),
        )
        .unwrap(),
    );
    let stop = raw_rpc(
        s,
        "orchestration.dispatchCommand",
        json!({"type":"thread.stop","commandId":"stop-unowned","threadId":"other"}),
    );
    assert_eq!(
        stop["exit"]["_tag"], "Success",
        "stop must fall back when ownership is missing: {stop}"
    );
    dispatch_command(
        &c,
        json!({"type":"thread.delete","commandId":"delete-other","threadId":"other"}),
    );
    dispatch_frames.insert("deleted-interrupt".into(), raw_rpc(s,"orchestration.dispatchCommand",json!({"type":"run.interrupt","commandId":"deleted-interrupt","threadId":"other","runId":"unknown"})));
    let deleted_stop = raw_rpc(
        s,
        "orchestration.dispatchCommand",
        json!({"type":"thread.stop","commandId":"deleted-stop","threadId":"other"}),
    );
    assert_eq!(
        deleted_stop["exit"]["_tag"], "Success",
        "deleted stop must be a no-op: {deleted_stop}"
    );
    std::fs::write(
        home.join("dispatch-frames.json"),
        serde_json::to_vec_pretty(&dispatch_frames).unwrap(),
    )
    .unwrap();
    eprintln!("quiescent");
    let fresh = first_thread(&c)["projection"].clone();
    let view = reader_outputs(&fresh);
    let messages = view["timeline"]["messages"].as_array().unwrap();
    for text in ["first", "queued-0", "queued-1", "queued-2"] {
        assert_eq!(
            messages.iter().filter(|m| m["text"] == text).count(),
            1,
            "missing or duplicated user message"
        );
    }
    let (extra_differences, extra_ignored) = verify_observers(s, [a, b], &view, reference);
    replay_differences.extend(extra_differences);
    ignored_reference_replay += extra_ignored;
    let done = std::cell::Cell::new(false);
    let mut sidebar = Value::Null;
    let shell_connection = connect(s);
    shell_connection
        .subscribe(
            "orchestration.subscribeShell",
            json!({}),
            || done.get(),
            |v| {
                if let Some(v) = v {
                    if v.get("resolvedRepositoryIdentityRoots").is_some() {
                        return Ok(());
                    }
                    sidebar = json!(
                        t3_protocol::sidebar::view::rows(&v["snapshot"], "fixture", 0).unwrap()
                    );
                    assert!(
                        !sidebar.as_array().unwrap().is_empty(),
                        "empty sidebar: {}",
                        shape(&v, "")
                    );
                    done.set(true);
                }
                Ok(())
            },
        )
        .unwrap();
    json!({"thread":view,"sidebar":sidebar,"replayDifferences":replay_differences,"ignoredReferenceReplay":ignored_reference_replay,"dispatchFrames":serde_json::from_str::<Value>(&serde_json::to_string(&dispatch_frames).unwrap().replace(run.as_str().unwrap(), "fixture-run")).unwrap(),"interruptPending":{"working":mid["timeline"]["threadState"]["working"],"canSteer":mid["queue"]["canSteer"],"activeRunId":mid["queue"]["activeRunId"]}})
}
#[test]
#[ignore = "real isolated reference and credential-free gated Codex; tools/conformance-diff concurrent"]
fn upstream_concurrent_differential() {
    use std::os::unix::fs::PermissionsExt;
    let root = PathBuf::from("/var/tmp/o3-build/tmp").join(format!(
        "concurrent-{}-{}",
        std::process::id(),
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let reference = reference_binary();
    let version = cli_output(command(&reference, &root).arg("--version"));
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("0.1.7-nightly.20261031.3991"),
        "reference must be pinned nightly 3991"
    );
    for name in ["t3", "o3"] {
        let home = root.join(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/concurrent-codex.py"),
            home.join("bin/codex"),
        )
        .unwrap();
    }
    let upstream = start(&reference, &root.join("t3"));
    let o3 = start(Path::new(env!("CARGO_BIN_EXE_o3-server")), &root.join("o3"));
    assert_ne!(
        upstream.base, o3.base,
        "servers require separate loopback ports"
    );
    let mut views = vec![];
    for (name, server) in [("t3", &upstream), ("o3", &o3)] {
        eprintln!("concurrent differential: {name}");
        views.push(concurrent_scenario(server, &root.join(name), name == "t3"));
    }
    // Keep all reader fields in the comparison. Any normalization must be justified
    // against the accepted independent-operation ordering contract before adoption.
    let normalized: Vec<_> = views
        .iter()
        .map(|v| {
            let mut v = v.clone();
            v.as_object_mut().unwrap().remove("replayDifferences");
            v.as_object_mut().unwrap().remove("ignoredReferenceReplay");
            normalize_concurrent(&v, "")
        })
        .collect();
    let mut differences = vec![];
    reader_differences(&normalized[0], &normalized[1], "", &mut differences);
    let replay_differences: Vec<_> = views.iter().enumerate()
        .filter(|(_,v)| !v["replayDifferences"].as_array().unwrap().is_empty())
        .map(|(i,v)| json!({"server":if i==0 {"reference"}else{"o3"},"differences":v["replayDifferences"]})).collect();
    let report = json!({"dispatchFrames": views.iter().map(|v| &v["dispatchFrames"]).collect::<Vec<_>>(), "ignoredReferenceReplay": views[0]["ignoredReferenceReplay"], "replayDifferences":replay_differences,"referenceVersion":"0.1.7-nightly.20261031.3991","provider":"deterministic gated stdio peer; no credentials or inference","perServer":"two subscribers: snapshot before tail, strictly increasing sequences, unique run creation, four original user messages exactly once plus approval-pending stop, two complete replay checkpoints, reduced/fresh timeline/queue/approval readers and decision builder, compaction candidate reader (no token context), nonempty sidebar reader","differences":differences});
    let report_path = std::env::var_os("O3_CONCURRENT_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("reader-shapes.json"));
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert_eq!(
        views[0]["dispatchFrames"], views[1]["dispatchFrames"],
        "raw Effect dispatch exits differ"
    );
    assert!(
        differences.is_empty() && replay_differences.is_empty(),
        "reader differences: {}; sanitized report: {}",
        differences.len(),
        report_path.display()
    );
    drop(o3);
    drop(upstream);
    std::fs::remove_dir_all(root).unwrap();
}

// Opaque generated identities/revision hashes and wall-clock instants are not
// portable across fresh profiles. Preserve zero/null distinctions and all semantic
// strings, booleans and counts. Only independently dispatched user rows and their
// acknowledgement list may permute under the accepted interleaving contract.
fn normalize_concurrent(v: &Value, key: &str) -> Value {
    normalize_concurrent_ids(v, key, &mut std::collections::BTreeMap::new())
}
fn normalize_concurrent_ids(
    v: &Value,
    key: &str,
    ids: &mut std::collections::BTreeMap<String, String>,
) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    // Activity ids are server-derived; message ids stay client fixture values.
                    let derived = k == "id"
                        && v.is_string()
                        && (m.contains_key("activity") || matches!(key, "activity" | "entries"));
                    let value = if derived {
                        mapped_id(v, ids)
                    } else {
                        normalize_concurrent_ids(v, k, ids)
                    };
                    (k.clone(), value)
                })
                .collect(),
        ),
        Value::Array(a) => {
            let mut a: Vec<_> = a
                .iter()
                .map(|v| normalize_concurrent_ids(v, key, ids))
                .collect();
            if matches!(key, "messages" | "timeline" | "acknowledgedMessageIds") {
                let independent = |v: &Value| {
                    v.as_str()
                        .or_else(|| v["id"].as_str())
                        .is_some_and(|s| s.starts_with("queued-"))
                };
                let mut queued: Vec<_> = a.iter().filter(|v| independent(v)).cloned().collect();
                queued.sort_by_key(Value::to_string);
                let mut queued = queued.into_iter();
                for v in &mut a {
                    if independent(v) {
                        *v = queued.next().unwrap();
                    }
                }
            }
            Value::Array(a)
        }
        Value::Number(n)
            if (key.ends_with("At") || key == "startedAt") && n.as_i64().is_some_and(|n| n > 0) =>
        {
            json!(1)
        }
        Value::String(_) if key.ends_with("At") => json!("timestamp"),
        Value::String(_) if matches!(key, "rev" | "revision") => json!("opaque-revision"),
        Value::String(_)
            if matches!(
                key,
                "mergeBackRunId"
                    | "requestId"
                    | "activeRunId"
                    | "rev"
                    | "revision"
                    | "runId"
                    | "sourceItemId"
                    | "providerTurnId"
            ) =>
        {
            mapped_id(v, ids)
        }
        _ => v.clone(),
    }
}

#[test]
fn concurrent_normalization_preserves_identity_relationships() {
    let v = normalize_concurrent(
        &json!({"runId":"a","providerTurnId":"a","mergeBackRunId":"b","sourceItemId":null}),
        "",
    );
    assert_eq!(v["runId"], v["providerTurnId"]);
    assert_ne!(v["runId"], v["mergeBackRunId"]);
    assert!(v["sourceItemId"].is_null());
    assert_ne!(
        v,
        normalize_concurrent(
            &json!({"runId":"a","providerTurnId":"b","mergeBackRunId":"b","sourceItemId":null}),
            ""
        )
    );
}

fn mapped_id(v: &Value, ids: &mut std::collections::BTreeMap<String, String>) -> Value {
    let raw = v.as_str().unwrap();
    let next = format!("identity-{}", ids.len());
    json!(ids.entry(raw.to_owned()).or_insert(next))
}

fn reader_differences(a: &Value, b: &Value, path: &str, out: &mut Vec<Value>) {
    if a == b {
        return;
    }
    match (a,b) {
        (Value::Object(a),Value::Object(b))=>{
            let keys:std::collections::BTreeSet<_>=a.keys().chain(b.keys()).collect();
            for k in keys {
                match (a.get(k),b.get(k)) {
                    (Some(a),Some(b))=>reader_differences(a,b,&format!("{path}/{k}"),out),
                    (a,b)=>out.push(json!({"path":format!("{path}/{k}"),"referencePresent":a.is_some(),"o3Present":b.is_some()}))
                }
            }
        },
        (Value::Array(a),Value::Array(b)) if a.len()==b.len()=>{
            for (i,(a,b)) in a.iter().zip(b).enumerate() { reader_differences(a,b,&format!("{path}/{i}"),out); }
        },
        _=>out.push(json!({"path":path,"reference":shape(a,path.rsplit('/').next().unwrap_or("")),"o3":shape(b,path.rsplit('/').next().unwrap_or(""))}))
    }
}

// Replay is a per-server invariant. Only the pinned reference's null-to-absent
// serialization defect is excluded; no o3 replay or cross-server field is exempt.
fn replay_differences_checked(
    replay: &Value,
    live: &Value,
    reference: bool,
    out: &mut Vec<Value>,
) -> usize {
    fn visit(replay: &mut Value, live: &Value) -> usize {
        match (replay, live) {
            (Value::Object(r), Value::Object(l)) => {
                let mut ignored = 0;
                if !r.contains_key("lastSnoozeWakeAt")
                    && l.get("lastSnoozeWakeAt") == Some(&Value::Null)
                {
                    r.insert("lastSnoozeWakeAt".into(), Value::Null);
                    ignored += 1;
                }
                for (k, v) in r.iter_mut() {
                    if let Some(l) = l.get(k) {
                        ignored += visit(v, l);
                    }
                }
                ignored
            }
            (Value::Array(r), Value::Array(l)) if r.len() == l.len() => {
                r.iter_mut().zip(l).map(|(r, l)| visit(r, l)).sum()
            }
            _ => 0,
        }
    }
    let mut replay = replay.clone();
    let ignored = if reference {
        visit(&mut replay, live)
    } else {
        0
    };
    reader_differences(&replay, live, "/replay", out);
    ignored
}
#[test]
fn replay_exception_is_reference_only_null_to_absent() {
    let live = json!({"payload":{"lastSnoozeWakeAt":null}});
    for (reference, replay, expected, ignored) in [
        (true, json!({"payload":{}}), 0, 1),
        (false, json!({"payload":{}}), 1, 0),
        (true, json!({"payload":{"lastSnoozeWakeAt":1}}), 1, 0),
        (true, json!({"payload":{},"other":null}), 1, 1),
    ] {
        let mut differences = vec![];
        assert_eq!(
            replay_differences_checked(&replay, &live, reference, &mut differences),
            ignored
        );
        assert_eq!(differences.len(), expected);
    }
    let mut differences = vec![];
    assert_eq!(
        replay_differences_checked(&live, &json!({"payload":{}}), true, &mut differences),
        0
    );
    assert_eq!(differences.len(), 1);
}

fn wait_file(path: &Path) {
    let end = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(Instant::now() < end, "gate timed out: {}", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "isolated pinned binary restart and control race windows"]
fn upstream_control_windows() {
    let root = PathBuf::from("/var/tmp/o3-build/tmp").join(format!(
        "control-{}",
        o3_server::store::random_id().unwrap()
    ));
    let mut results = vec![];
    let version = cli_output(
        command(&reference_binary(), Path::new("/var/tmp/o3-build/tmp")).arg("--version"),
    );
    assert!(String::from_utf8_lossy(&version.stdout).contains("0.1.7-nightly.20261031.3991"));
    let selected = std::env::var("O3_CONTROL_CASE").ok();
    for instance in ["codex", "claudeAgent"] {
        for mode in ["init", "steer", "death", "restart", "disabled"] {
            for reference in [true, false] {
                if selected
                    .as_ref()
                    .is_some_and(|s| s != &format!("{instance}:{mode}"))
                {
                    continue;
                }
                if instance == "claudeAgent" && (reference || mode == "disabled") {
                    continue;
                }
                let home = root.join(if instance == "codex" {
                    format!("{mode}-{reference}")
                } else {
                    format!("claude-{mode}-{reference}")
                });
                std::fs::create_dir_all(home.join("bin")).unwrap();
                std::os::unix::fs::symlink(
                    Path::new(env!("CARGO_MANIFEST_DIR")).join(if instance == "codex" {
                        "tests/fixtures/concurrent-codex.py"
                    } else {
                        "tests/fixtures/concurrent-claude.py"
                    }),
                    home.join(if instance == "codex" {
                        "bin/codex"
                    } else {
                        "bin/claude"
                    }),
                )
                .unwrap();
                if mode == "init" {
                    std::fs::write(home.join("gate-init"), "").unwrap();
                }
                if mode == "death" {
                    std::fs::write(home.join("die-interrupt"), "").unwrap();
                }
                let bin = if reference {
                    reference_binary()
                } else {
                    PathBuf::from(env!("CARGO_BIN_EXE_o3-server"))
                };
                let mut server = start(&bin, &home);
                let c = connect(&server);
                c.rpc("server.updateSettings",json!({"patch":{"continueThreadsAfterServerUpdate": mode != "disabled","textGenerationModelSelection":{"instanceId":"unavailable","model":"fixture"}}})).unwrap();
                c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"fixture-project","title":"Fixture","workspaceRoot":home,"scripts":[]})).unwrap();
                dispatch_command(
                    &c,
                    json!({"type":"thread.create","commandId":"t","threadId":"concurrent","projectId":"fixture-project","title":"Fixture","modelSelection":{"instanceId":instance,"model":"fixture"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null}),
                );
                dispatch_command(
                    &c,
                    json!({"type":"message.dispatch","commandId":"m","messageId":"m","threadId":"concurrent","text":"first","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
                );
                wait_file(&home.join(if mode == "init" {
                    "init-received"
                } else {
                    "started-1"
                }));
                let p = if mode == "init" {
                    first_thread(&c)["projection"].clone()
                } else {
                    wait_projection(&c, |p| {
                        p["runs"][0]["status"] == "running" && !p["runs"][0]["startedAt"].is_null()
                    })
                };
                let rid = p["runs"][0]["id"].clone();
                if mode == "steer" {
                    dispatch_command(
                        &c,
                        json!({"type":"message.dispatch","commandId":"steer","messageId":"steer","threadId":"concurrent","text":"steer","attachments":[],"dispatchMode":{"type":"steer_active","targetRunId":rid}}),
                    );
                    wait_file(&home.join("steer-received"));
                }
                if mode != "disabled" {
                    dispatch_command(
                        &c,
                        json!({"type":"run.interrupt","commandId":"i","threadId":"concurrent","runId":rid,"holdQueue":true}),
                    );
                }
                if mode == "steer" {
                    // Within Codex's 30s RPC budget, beyond the former 15s
                    // acceptance deadline. The interrupt is still queued here.
                    if instance == "codex" {
                        std::thread::sleep(Duration::from_secs(20));
                    }
                    let queued = first_thread(&c)["projection"].clone();
                    assert_eq!(queued["runs"][0]["status"], "running");
                    assert!(!home.join("interrupt-received").exists());
                    std::fs::write(home.join("release-steer"), "").unwrap();
                    wait_file(&home.join("interrupt-received"));
                    let mid = first_thread(&c)["projection"].clone();
                    assert_eq!(
                        reader_outputs(&mid)["timeline"]["threadState"]["working"],
                        true
                    );
                    assert!(!mid["turnItems"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["type"] == "run_interrupt_result"));
                }
                if matches!(mode, "restart" | "disabled") {
                    if mode == "restart" {
                        wait_file(&home.join("interrupt-received"));
                    }
                    server.child.kill().unwrap();
                    server.child.wait().unwrap();
                    drop(server);
                    server = start(&bin, &home);
                } else {
                    std::fs::write(home.join("release-interrupt"), "").unwrap();
                }
                let c = connect(&server);
                let p = wait_projection(&c, |p| {
                    reader_outputs(p)["timeline"]["threadState"]["working"] == false
                });
                std::fs::write(home.join("release-init"), "").unwrap();
                if mode == "init" {
                    std::thread::sleep(Duration::from_millis(250));
                    assert!(
                        !home.join("started-1").exists(),
                        "cancelled init launched a late native turn"
                    );
                }
                if matches!(mode, "init" | "steer") {
                    assert_eq!(p["runs"][0]["status"], "interrupted");
                }
                if mode == "disabled" {
                    assert_eq!(p["runs"][0]["status"], "cancelled");
                    assert!(p["turnItems"].as_array().unwrap().iter().all(|i| !matches!(
                        i["type"].as_str(),
                        Some("run_interrupt_request" | "run_interrupt_result")
                    )));
                    std::fs::write(
                        home.join("disabled-readers.json"),
                        serde_json::to_vec_pretty(&reader_outputs(&p)).unwrap(),
                    )
                    .unwrap();
                }
                if !reference && matches!(mode, "death" | "restart") {
                    assert_eq!(p["runs"][0]["status"], "failed");
                    assert!(!p["turnItems"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|i| i["type"] == "run_interrupt_result"));
                    let db = rusqlite::Connection::open(home.join("userdata/o3.sqlite")).unwrap();
                    assert_eq!(db.query_row("SELECT json_extract(body,'$.status') FROM records WHERE kind='controlOutbox' AND id='i'",[],|r|r.get::<_,String>(0)).unwrap(), "delivery-unknown");
                }
                let result = json!({"provider":instance,"mode":mode,"reference":reference,"status":p["runs"][0]["status"],"items":p["turnItems"].as_array().unwrap().iter().map(|i|json!({"type":i["type"],"status":i["status"],"message":i["message"]})).collect::<Vec<_>>(),"working":reader_outputs(&p)["timeline"]["threadState"]["working"]});
                eprintln!("control window: {result}");
                results.push(result);
                drop(server);
            }
        }
    }
    if selected.is_some() {
        return;
    }
    let disabled: Vec<Value> = [true, false]
        .into_iter()
        .map(|r| {
            serde_json::from_slice(
                &std::fs::read(root.join(format!("disabled-{r}/disabled-readers.json"))).unwrap(),
            )
            .unwrap()
        })
        .collect();
    let mut differences = vec![];
    reader_differences(
        &normalize_concurrent(&disabled[0], ""),
        &normalize_concurrent(&disabled[1], ""),
        "",
        &mut differences,
    );
    eprintln!("disabled reader differences: {}", json!(differences));
    assert!(differences.is_empty(), "disabled restart readers differ");
    std::fs::write(
        root.join("results.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    eprintln!("control report: {}", root.join("results.json").display());
}

/// Isolated P1 reads preserve search ordering and actual downloaded bytes, not
/// scalar shapes. A difference is evidence to keep a row open, not an exemption.
#[test]
#[ignore = "isolated pinned 3991 P1 readers, no credentials or inference"]
fn upstream_p1_readers() {
    let root = PathBuf::from("/var/tmp/o3-build/tmp").join(format!(
        "p1-{}-{}",
        std::process::id(),
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let reference = reference_binary();
    let version = cli_output(command(&reference, &root).arg("--version"));
    assert!(String::from_utf8_lossy(&version.stdout).contains("0.1.7-nightly.20261031.3991"));
    let mut views = vec![];
    for (label, binary) in [
        ("reference", reference.as_path()),
        ("o3", Path::new(env!("CARGO_BIN_EXE_o3-server"))),
    ] {
        let home = root.join(label);
        let server = start(binary, &home);
        let c = connect(&server);
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("fixture.html"), b"<p>asset-proof</p>").unwrap();
        c.rpc("projects.mutate", json!({"type":"project.create","commandId":"project","projectId":"p","title":"P1","workspaceRoot":workspace})).unwrap();
        for (id, title) in [
            ("substring", "A needle elsewhere"),
            ("prefix", "Needle first"),
            ("exact", "Needle"),
            ("boundary", "A needle"),
        ] {
            c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":id,"threadId":id,"projectId":"p","title":title,"createdBy":"user","creationSource":"web","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
            c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":format!("send-{id}"),"threadId":id,"messageId":format!("m-{id}"),"text":match id { "substring"=>format!("{}needle{}", "x".repeat(350), "z".repeat(350)), "prefix"=>"needle first message".into(), "exact"=>"needle".into(), _=>"a needle".into() },"createdBy":"user","creationSource":"web","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
        }
        let mut searches = json!({});
        for query in ["needle", "eedl", "a needle"] {
            let found =
                t3_protocol::api::orchestration::search_threads(&c, query, Some(20)).unwrap();
            searches[query] = json!(found["matches"].as_array().unwrap().iter().map(|m| json!({"threadId":m["threadId"],"source":m["source"],"snippet":m["snippet"]})).collect::<Vec<_>>());
        }
        let upload = c
            .upload_attachment(
                "fixture.html",
                "text/html",
                18,
                std::io::Cursor::new(b"<p>asset-proof</p>"),
            )
            .unwrap();
        let asset = c.asset_bytes(json!({"_tag":"attachment","attachmentId":upload["id"],"fileName":"fixture.html","mimeType":"text/html"}), 1024).unwrap();
        assert_eq!(asset.bytes, b"<p>asset-proof</p>");
        views.push(json!({"searches":searches,"assetBytes":asset.bytes}));
    }
    let report = json!({"referenceVersion":"0.1.7-nightly.20261031.3991","reference":views[0],"o3":views[1],"equal":views[0]==views[1],"scope":"actual lib search_threads and asset_bytes readers on isolated server output; ordered title matches and uploaded attachment download only"});
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/p1-reader-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("P1 reader equality: {}", report["equal"]);
    assert_eq!(views[0], views[1], "consumed P1 reader mismatch");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "isolated pinned settings acceptance probe, no inference"]
fn upstream_generation_setting_acceptance() {
    let home = PathBuf::from("/var/tmp/o3-build/tmp")
        .join(format!("generation-setting-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let server = start(&reference_binary(), &home);
    let c = connect(&server);
    let result = c.rpc("server.updateSettings",json!({"patch":{"textGenerationModelSelection":{"instanceId":"codex","model":"fixture"}}})).unwrap();
    let saved = t3_protocol::api::server::config(&c).unwrap()["settings"].clone();
    assert_eq!(saved["textGenerationModelSelection"]["instanceId"], "codex");
    std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/generation-settings-reference.json"),serde_json::to_vec_pretty(&json!({"accepted":true,"savedSelection":saved["textGenerationModelSelection"],"responseHasSettings":result.get("settings").is_some()})).unwrap()).unwrap();
}

#[test]
#[ignore = "isolated pinned shell preview and automatic launch probe, no inference"]
fn upstream_shell_preview_and_launch() {
    let home = PathBuf::from("/var/tmp/o3-build/tmp")
        .join(format!("shell-preview-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let server = start(&reference_binary(), &home);
    let c = connect(&server);
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"textGenerationModelSelection":{"instanceId":"codex","model":"fixture"}}}),
    )
    .unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Fixture","workspaceRoot":home,"scripts":[]})).unwrap();
    let launch = outcome(c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Untitled","modelSelection":{"instanceId":"claudeAgent","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"generateTitle":true})));
    assert!(launch.get("ok").is_some(), "{launch}");
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"message","threadId":"t","messageId":"m","text":"x".repeat(32768),"createdBy":"user","creationSource":"web","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    let preview = &shell["threads"][0]["latestVisibleMessage"];
    let report = json!({"referenceVersion":"0.1.7-nightly.20261031.3991","launchAccepted":true,"scope":"empty automatic launch with saved Codex generation selection; deferred message, no inference","latestVisibleMessage":preview,"previewBytes":preview["text"].as_str().map(str::len)});
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/shell-preview-reference.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("pinned preview bytes: {}", report["previewBytes"]);
}

/// Compare the consumed history property, not nondeterministic CPU percentages.
#[test]
#[ignore = "isolated pinned 3991 P2 telemetry, deterministic terminal child"]
fn upstream_p2_telemetry_reader() {
    let root = PathBuf::from("/var/tmp/o3-build/tmp").join(format!(
        "p2-telemetry-{}-{}",
        std::process::id(),
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let reference = reference_binary();
    let version = cli_output(command(&reference, &root).arg("--version"));
    assert!(String::from_utf8_lossy(&version.stdout).contains("0.1.7-nightly.20261031.3991"));
    let mut views = Vec::new();
    for (label, bin) in [
        ("reference", reference.as_path()),
        ("o3", Path::new(env!("CARGO_BIN_EXE_o3-server"))),
    ] {
        let home = root.join(label);
        let s = start(bin, &home);
        let c = connect(&s);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let stream_conn = c.clone();
        let stream = std::thread::spawn(move || {
            stream_conn.subscribe(
                "server.getStorageCleanupReport",
                json!({}),
                || stopped.load(std::sync::atomic::Ordering::Relaxed),
                |_| Ok(()),
            )
        });
        c.rpc(
            "terminal.open",
            json!({"threadId":"telemetry","terminalId":"busy","cwd":home,"cols":80,"rows":24}),
        )
        .unwrap();
        let initial = t3_protocol::api::diagnostics::telemetry_history(&c, 60_000, 60_000).unwrap();
        c.rpc(
            "terminal.write",
            json!({"threadId":"telemetry","terminalId":"busy","data":include_str!("fixtures/telemetry-busy.sh")}),
        )
        .unwrap();
        // The busy process belongs only to this isolated server's process group.
        std::thread::sleep(Duration::from_secs(12));
        let history = t3_protocol::api::diagnostics::telemetry_history(&c, 60_000, 60_000).unwrap();
        c.rpc(
            "terminal.close",
            json!({"threadId":"telemetry","terminalId":"busy"}),
        )
        .unwrap();
        let average = |h: &Value| {
            h["buckets"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|b| b["avgCpuPercent"].as_f64())
                .fold(0.0, f64::max)
        };
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        stream.join().unwrap().unwrap();
        views.push(json!({"server":label,"initialAvgCpu":average(&initial),"busyAvgCpu":average(&history),"retainedSampleCount":history["retainedSampleCount"],"childCpuVisible":average(&history)>5.0,"health":history["health"]}));
    }
    let report = json!({"referenceVersion":"0.1.7-nightly.20261031.3991","scope":"real lib telemetry reader; isolated busy terminal child; percentages are observations, not exact equivalence","views":views});
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/p2-telemetry-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    assert_eq!(
        views[1]["childCpuVisible"], true,
        "o3 reader regression: {report}"
    );
    if views[0]["retainedSampleCount"].as_u64().unwrap_or(0) > 0 {
        assert_eq!(
            views[0]["childCpuVisible"], true,
            "reference collected samples but child CPU absent: {report}"
        );
    } else {
        eprintln!("Pinned reference returned no samples: CPU equivalence remains unproven");
    }
    std::fs::remove_dir_all(root).unwrap();
}
