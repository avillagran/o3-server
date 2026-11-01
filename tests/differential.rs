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
    let path = format!("{}:/usr/bin:/bin", node.parent().unwrap().display());
    c.env_clear()
        .env("PATH", path)
        .env("HOME", home.join("home"))
        .env("T3CODE_HOME", home)
        .env("TMPDIR", "/var/tmp/o3-build/tmp")
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
#[test]
#[ignore = "starts isolated real T3 and o3; tools/conformance-diff"]
fn upstream_differential() {
    let root = PathBuf::from("/var/tmp/o3-build/tmp/differential")
        .join(format!("run-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let t3 = std::env::var_os("T3_REFERENCE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let versions =
                PathBuf::from(std::env::var_os("HOME").unwrap()).join(".t3/runtime/versions");
            let mut bins: Vec<_> = std::fs::read_dir(versions)
                .unwrap()
                .flatten()
                .map(|e| e.path().join("t3"))
                .filter(|p| p.is_file())
                .collect();
            bins.sort();
            bins.pop().expect("set T3_REFERENCE_BIN")
        });
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
