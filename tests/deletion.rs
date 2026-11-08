mod common;
mod support;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use t3_protocol::Connection;
const WAIT: Duration = Duration::from_secs(12);
fn fixture() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var(
            "O3_BROWSER_BINARY",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
        )
    });
}
fn project(c: &Connection, id: &str, root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":format!("project-{id}"),"projectId":id,"title":id,"workspaceRoot":root})).unwrap();
}
fn thread(c: &Connection, id: &str, project: &str, instance: &str) {
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("thread-{id}"),"threadId":id,"projectId":project,"title":id,"modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
}
fn send(c: &Connection, id: &str, text: &str) {
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":text,"threadId":id,"messageId":text,"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
}
fn put(s: &o3_server::store::State, kind: &str, id: &str, tid: &str, value: Value) {
    s.db.lock().unwrap().execute("INSERT INTO records VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",rusqlite::params![kind,id,tid,value.to_string()]).unwrap();
}
fn record(s: &o3_server::store::State, kind: &str, id: &str) -> Value {
    let raw: String =
        s.db.lock()
            .unwrap()
            .query_row(
                "SELECT body FROM records WHERE kind=?1 AND id=?2",
                rusqlite::params![kind, id],
                |r| r.get(0),
            )
            .unwrap();
    serde_json::from_str(&raw).unwrap()
}
fn count(s: &o3_server::store::State, sql: &str) -> i64 {
    s.db.lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn effects_done(s: &o3_server::store::State) -> bool {
    count(
        s,
        "SELECT count(*) FROM deletion_effects WHERE completed_at IS NULL",
    ) == 0
}
fn auth(v: &Value) -> String {
    v["config"]["headers"]["Authorization"]
        .as_str()
        .or(v["config"]["http_headers"]["Authorization"].as_str())
        .unwrap()
        .into()
}
fn mcp(
    s: &support::Server,
    auth: &str,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> (u16, String, Value) {
    let mut h = vec![
        ("authorization", auth),
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    if let Some(id) = session {
        h.push(("mcp-session-id", id));
    }
    let (code, h, b) = common::http(
        &s.base,
        "POST",
        "/mcp",
        &h,
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            .to_string()
            .as_bytes(),
    );
    (code, h, serde_json::from_slice(&b).unwrap())
}
fn init(s: &support::Server, auth: &str) -> String {
    let (code, h, _) = mcp(
        s,
        auth,
        None,
        "initialize",
        json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"deletion-test","version":"1"},"capabilities":{}}),
    );
    assert_eq!(code, 200);
    h.lines()
        .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
        .unwrap()
        .into()
}
fn observe(root: &Path, label: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(format!("observe-{label}.json"))).unwrap())
        .unwrap()
}
fn projection(c: &Connection, id: &str) -> Value {
    t3_protocol::api::orchestration::thread_projection(c, id).unwrap()
}
#[test]
fn native_self_project_deletion_reaps_claude_and_unloads_only_its_codex_threads() {
    fixture();
    for instance in ["claudeAgent", "codex"] {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deletion-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let removals = Arc::new(support::profile_removal::Recording::default());
        let s = support::start(|s| {
            s.streams.p2.preview.set_profile_removal(removals.clone());
            s.streams.terminals.set_shell("/bin/sh");
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let c = s.connect();
        let root = s.dir.join("project");
        let survivor = s.dir.join("survivor");
        project(&c, "p", &root);
        project(&c, "q", &survivor);
        thread(&c, "target", "p", instance);
        thread(&c, "idle", "p", instance);
        thread(&c, "survivor", "q", instance);
        send(&c, "idle", "idle-cached");
        assert!(support::wait_for(WAIT, || projection(&c, "idle")["runs"]
            [0]["status"]
            == "completed"));
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.model-selection.set","commandId":"switch-idle-provider","threadId":"idle","modelSelection":{"instanceId":if instance=="claudeAgent"{"codex"}else{"claudeAgent"},"model":"test"}})).unwrap();
        send(&c, "idle", "idle-switched");
        assert!(support::wait_for(WAIT, || projection(&c, "idle")["runs"]
            .as_array()
            .unwrap()
            .len()
            == 2
            && projection(&c, "idle")["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "completed")));
        send(&c, "target", "wait-target");
        send(&c, "survivor", "wait-survivor");
        assert!(support::wait_for(WAIT, || projection(&c, "target")
            ["runtimeRequests"]
            .as_array()
            .is_some_and(|a| a.iter().any(|r| r["status"] == "pending"))
            && projection(&c, "survivor")["runtimeRequests"]
                .as_array()
                .is_some_and(|a| a
                    .iter()
                    .any(|r| r["status"] == "pending"))));
        let target = observe(&root, "wait-target");
        let idle = observe(&root, "idle-cached");
        let other = observe(&survivor, "wait-survivor");
        let credential = auth(&target);
        let native_session = init(&s, &credential);
        let other_auth = auth(&other);
        let other_session = init(&s, &other_auth);
        let (_, _, refused) = mcp(
            &s,
            &credential,
            Some(&native_session),
            "tools/call",
            json!({"name":"t3_project_delete","arguments":{"projectId":"p"}}),
        );
        assert_eq!(refused["result"]["isError"], true);
        assert!(record(&s.state, "thread", "target")["deletedAt"].is_null());
        let terminal = c
            .rpc(
                "terminal.open",
                json!({"threadId":"target","terminalId":"shell","cwd":root,"cols":80,"rows":24}),
            )
            .unwrap();
        drop(terminal);
        c.rpc(
            "terminal.write",
            json!({"threadId":"target","terminalId":"shell","data":"printf history; sleep 30\n"}),
        )
        .unwrap();
        c.rpc("preview.open",json!({"threadId":"target","runtime":"server","profileId":"incognito","url":"http://localhost:1234"})).unwrap();
        put(
            &s.state,
            "scheduledTasks",
            "scheduled",
            "",
            json!({"id":"scheduled","threadId":"target","enabled":false}),
        );
        let attachment = format!("target-{}-bin", o3_server::store::random_id().unwrap());
        let path = s.dir.join("attachments").join(format!("{attachment}.bin"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"attachment").unwrap();
        std::fs::write(format!("{}.txt", path.display()), "extracted").unwrap();
        s.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.attachments',json(?1)) WHERE kind='messages' AND id='wait-target'",[json!([{"id":attachment}]).to_string()]).unwrap();
        let (code, _, deleted) = mcp(
            &s,
            &credential,
            Some(&native_session),
            "tools/call",
            json!({"name":"t3_project_delete","arguments":{"projectId":"p","force":true}}),
        );
        assert_eq!(code, 200);
        assert_eq!(deleted["result"]["isError"], false, "{deleted}");
        assert!(support::wait_for(WAIT, || effects_done(&s.state)));
        assert!(support::wait_for(WAIT, || !removals
            .0
            .lock()
            .unwrap()
            .is_empty()));
        for plan in removals.0.lock().unwrap().iter() {
            assert!(plan
                .operations()
                .last()
                .unwrap()
                .path
                .join("fixture.pid")
                .is_file());
        }
        assert!(root.is_dir(), "the project folder stays");
        assert!(!path.exists());
        assert!(!Path::new(&format!("{}.txt", path.display())).exists());
        for tid in ["target", "idle"] {
            assert!(record(&s.state, "thread", tid)["deletedAt"].is_string());
        }
        let p = projection(&c, "target");
        assert!(p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "cancelled"));
        assert!(p["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] != "pending"));
        assert_eq!(
            mcp(&s, &credential, Some(&native_session), "ping", json!({})).0,
            401
        );
        assert_eq!(
            mcp(&s, &other_auth, Some(&other_session), "ping", json!({})).0,
            200
        );
        assert!(s
            .state
            .streams
            .terminals
            .summaries()
            .iter()
            .all(|v| v["threadId"] != "target"));
        assert_eq!(
            c.rpc("preview.list", json!({"threadId":"target"})).unwrap()["sessions"],
            json!([])
        );
        assert_eq!(
            count(
                &s.state,
                "SELECT count(*) FROM records WHERE kind='scheduledTasks' AND id='scheduled'"
            ),
            0
        );
        assert!(c
            .rpc(
                "terminal.open",
                json!({"threadId":"target","terminalId":"new","cwd":root,"cols":80,"rows":24})
            )
            .is_err());
        assert!(c
            .rpc(
                "preview.open",
                json!({"threadId":"target","url":"http://localhost"})
            )
            .is_err());
        let switched = observe(&root, "idle-switched");
        if instance == "codex" {
            assert!(support::wait_for(WAIT, || !Path::new(&format!(
                "/proc/{}",
                switched["pid"].as_u64().unwrap()
            ))
            .exists()));
        }
        assert!(c
            .rpc(
                "terminal.restart",
                json!({"threadId":"target","terminalId":"shell","cwd":root,"cols":80,"rows":24})
            )
            .is_err());
        if instance == "claudeAgent" {
            for peer in [&target, &idle] {
                assert!(support::wait_for(WAIT, || !Path::new(&format!(
                    "/proc/{}",
                    peer["pid"].as_u64().unwrap()
                ))
                .exists()));
            }
        } else {
            assert_eq!(target["pid"], other["pid"]);
            assert!(root.join("unload-retried").is_file());
            assert!(
                count(
                    &s.state,
                    "SELECT count(*) FROM deletion_effects WHERE attempts>0"
                ) > 0
            );

            assert!(Path::new(&format!("/proc/{}", other["pid"].as_u64().unwrap())).exists());
            let unloaded: Vec<Value> = std::fs::read_to_string(root.join("unloaded.jsonl"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            assert!(unloaded.contains(&target["native"]));
            assert!(unloaded.contains(&idle["native"]));
            assert!(!unloaded.contains(&other["native"]));
        }
        let alive = projection(&c, "survivor");
        let request = alive["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["status"] == "pending")
            .unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"finish-survivor","threadId":"survivor","requestId":request["id"],"decision":"accept"})).unwrap();
        assert!(support::wait_for(WAIT, || projection(&c, "survivor")
            ["runs"][0]["status"]
            == "completed"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            claude.shutdown().await;
            codex.shutdown().await;
        });
    }
}
fn seed(state: &Arc<o3_server::store::State>, id: &str) {
    o3_server::events::mutate_project(state,&json!({"type":"project.create","commandId":format!("p-{id}"),"projectId":id,"title":id,"workspaceRoot":state.dir})).unwrap();
    o3_server::events::dispatch(state,&json!({"type":"thread.create","commandId":format!("t-{id}"),"threadId":id,"projectId":id,"title":id,"modelSelection":{"instanceId":"scripted","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
}
fn serve(state: Arc<o3_server::store::State>) -> support::Server {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = state.dir.clone();
    let run = state.clone();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            o3_server::serve(tokio::net::TcpListener::from_std(listener).unwrap(), run)
                .await
                .unwrap()
        })
    });
    support::signals::shield_client_thread_from_sigchld();
    support::Server {
        base,
        secret: String::new(),
        state,
        dir,
    }
}
#[test]
fn deletion_is_receipt_atomic_and_recovery_never_resumes_cancelled_work() {
    fixture();
    let root = std::env::temp_dir().join(format!(
        "o3-delete-recover-{}",
        o3_server::store::random_id().unwrap()
    ));
    let state = Arc::new(o3_server::store::State::open(&root).unwrap());
    seed(&state, "dead");
    for (id, status) in [
        ("active", "running"),
        ("queued", "queued"),
        ("finished", "completed"),
    ] {
        put(
            &state,
            "runs",
            id,
            "dead",
            json!({"id":id,"threadId":"dead","providerInstanceId":"scripted","status":status,"ordinal":1,"queuePosition":2,"executionRequested":true,"recoveryPending":true}),
        );
    }
    put(
        &state,
        "runtimeRequests",
        "pending",
        "dead",
        json!({"id":"pending","nodeId":"active","status":"pending","kind":"command"}),
    );
    put(
        &state,
        "subagents",
        "child",
        "dead",
        json!({"id":"child","runId":"finished","origin":"app_owned","status":"running"}),
    );
    put(
        &state,
        "providerSessions",
        "session",
        "dead",
        json!({"id":"session","status":"ready","driver":"claudeAgent","providerInstanceId":"scripted"}),
    );
    let command =
        json!({"type":"project.delete","commandId":"delete","projectId":"dead","force":true});
    state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_deletion BEFORE INSERT ON receipts WHEN NEW.command_id='delete' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(o3_server::events::mutate_project(&state, &command).is_err());
    assert!(record(&state, "thread", "dead")["deletedAt"].is_null());
    assert_eq!(record(&state, "runs", "active")["status"], "running");
    assert_eq!(count(&state, "SELECT count(*) FROM deletion_effects"), 0);
    state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_deletion")
        .unwrap();
    let deleted = o3_server::events::mutate_project(&state, &command).unwrap();
    let before = count(&state, "SELECT count(*) FROM events");
    assert_eq!(
        o3_server::events::mutate_project(&state, &command).unwrap(),
        deleted
    );
    assert_eq!(count(&state, "SELECT count(*) FROM events"), before);
    assert_eq!(count(&state, "SELECT count(*) FROM deletion_effects"), 1);
    assert_eq!(record(&state, "runs", "active")["status"], "cancelled");
    assert_eq!(record(&state, "runs", "queued")["status"], "cancelled");
    assert_eq!(record(&state, "runs", "finished")["status"], "completed");
    assert_eq!(
        record(&state, "runs", "finished")["delegatedCompletion"]["disposition"],
        "disposed"
    );
    assert_eq!(
        record(&state, "subagents", "child")["completionDelivery"]["state"],
        "disposed"
    );
    assert_eq!(
        record(&state, "runtimeRequests", "pending")["responseCapability"]["type"],
        "not_resumable"
    );
    assert_eq!(
        record(&state, "providerSessions", "session")["status"],
        "stopped"
    );
    let attachment = format!("dead-{}-bin", o3_server::store::random_id().unwrap());
    let file = root.join("attachments").join(format!("{attachment}.bin"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "retained until restart").unwrap();
    // A legacy tombstone has no effect row yet; recovery must migrate it too.
    put(
        &state,
        "thread",
        "legacy",
        "legacy",
        json!({"id":"legacy","projectId":"dead","providerInstanceId":"scripted","deletedAt":"2026-11-04T00:00:00Z"}),
    );
    put(
        &state,
        "runs",
        "legacy-run",
        "legacy",
        json!({"id":"legacy-run","threadId":"legacy","status":"running","providerInstanceId":"scripted","recoveryPending":true}),
    );
    put(
        &state,
        "messages",
        "media",
        "legacy",
        json!({"id":"media","attachments":[{"id":attachment}]}),
    );
    drop(state);
    let reopened = Arc::new(o3_server::store::State::open(&root).unwrap());
    let s = serve(reopened);
    let c = s.connect();
    assert!(support::wait_for(WAIT, || effects_done(&s.state)
        && count(&s.state, "SELECT count(*) FROM deletion_effects")
            == 2));
    assert_eq!(
        record(&s.state, "runs", "legacy-run")["status"],
        "cancelled"
    );
    assert!(!file.exists());
    assert!(s.state.active.lock().unwrap().is_empty());
    assert!(c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"resurrect","threadId":"dead","messageId":"bad","text":"bad","attachments":[],"dispatchMode":{"type":"start_immediately"}})).is_err());
}
#[test]
fn failed_attachment_cleanup_retries_without_losing_sidecars_or_shared_files() {
    fixture();
    let s = support::start(|_| {});
    let c = s.connect();
    project(&c, "p", &s.dir.join("p"));
    project(&c, "q", &s.dir.join("q"));
    thread(&c, "dead", "p", "scripted");
    thread(&c, "other:dead", "q", "scripted");
    let id = format!("dead-{}-bin", o3_server::store::random_id().unwrap());
    let shared = format!("dead-{}-bin", o3_server::store::random_id().unwrap());
    std::fs::create_dir_all(s.dir.join("attachments")).unwrap();
    let path = s.dir.join("attachments").join(format!("{id}.bin"));
    let side = path.with_extension("bin.txt");
    let kept = s.dir.join("attachments").join(format!("{shared}.bin"));
    std::fs::write(&path, "data").unwrap();
    std::fs::create_dir(&side).unwrap();
    std::fs::write(&kept, "shared").unwrap();
    put(
        &s.state,
        "messages",
        "owned",
        "dead",
        json!({"id":"owned","attachments":[{"id":id},{"id":shared},{"id":"../../outside"}]}),
    );
    put(
        &s.state,
        "messages",
        "inherited",
        "other:dead",
        json!({"id":"inherited","attachments":[{"id":shared}]}),
    );
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"delete:other","threadId":"dead"}),
    )
    .unwrap();
    assert!(support::wait_for(WAIT, || count(
        &s.state,
        "SELECT count(*) FROM deletion_effects WHERE attempts>0"
    ) == 1));
    assert!(!effects_done(&s.state));
    std::fs::remove_dir(&side).unwrap();
    std::fs::write(&side, "late sidecar").unwrap();
    assert!(support::wait_for(WAIT, || effects_done(&s.state)));
    assert!(!path.exists());
    assert!(!side.exists());
    assert!(kept.exists());
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.delete","commandId":"delete","threadId":"other:dead"}),
    )
    .unwrap();
    assert!(support::wait_for(WAIT, || effects_done(&s.state)));
    assert!(!kept.exists());
}

#[test]
fn deletion_during_codex_binding_unloads_the_late_reply_without_stopping_a_sibling() {
    fixture();
    let codex = Arc::new(o3_server::codex::CodexProvider::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deletion-peer.py"),
        None,
    ));
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(codex.clone()));
    let c = s.connect();
    let target = s.dir.join("target");
    let sibling = s.dir.join("sibling");
    project(&c, "p", &target);
    project(&c, "q", &sibling);
    thread(&c, "target", "p", "codex");
    thread(&c, "survivor", "q", "codex");
    send(&c, "survivor", "wait-survivor");
    assert!(support::wait_for(WAIT, || projection(&c, "survivor")
        ["runtimeRequests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["status"] == "pending")));
    std::fs::write(target.join("hold-binding"), "").unwrap();
    send(&c, "target", "wait-target");
    assert!(support::wait_for(WAIT, || target
        .join("observe-binding-pending.json")
        .is_file()));
    let pending = observe(&target, "binding-pending");
    c.rpc(
        "projects.mutate",
        json!({"type":"project.delete","commandId":"delete-pending","projectId":"p","force":true}),
    )
    .unwrap();
    assert!(support::wait_for(WAIT, || effects_done(&s.state)));
    assert_eq!(
        mcp(&s, &auth(&pending), None, "initialize", json!({})).0,
        401
    );
    std::fs::write(target.join("release-binding"), "").unwrap();
    assert!(support::wait_for(WAIT, || {
        std::fs::read_to_string(target.join("unloaded.jsonl"))
            .ok()
            .is_some_and(|s| {
                s.lines()
                    .any(|v| serde_json::from_str::<Value>(v).unwrap() == pending["native"])
            })
    }));
    assert!(projection(&c, "survivor")["runtimeRequests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["status"] == "pending"));
    assert!(Path::new(&format!("/proc/{}", pending["pid"].as_u64().unwrap())).exists());
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(codex.shutdown());
}
