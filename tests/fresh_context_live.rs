//! Opt-in native acceptance. Only the rootless container runner may execute this test.
#[path = "support/signals.rs"]
mod signals;
use o3_server::{claude::ClaudeProvider, codex::CodexProvider, provider::Providers, store::State};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use t3_protocol::{pair_bearer, Connection, ConnectionOptions, PairingClient};

fn projection(c: &Connection) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap()
}
fn run(c: &Connection, id: &str, instance: &str, model: &str, text: &str, fresh: bool) -> Value {
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","createdBy":"user","creationSource":"web","commandId":id,"threadId":"t","messageId":id,"modelSelection":{"instanceId":instance,"model":model},"text":text,"attachments":[],"freshContext":fresh,"dispatchMode":{"type":"queue_after_active"}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(100);
    loop {
        let p = projection(c);
        let run = p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["userMessageId"] == id)
            .unwrap();
        assert_ne!(
            run["status"], "failed",
            "native stage {id} failed: {}",
            run["error"]
        );
        if run["status"] == "completed" {
            println!("FRESH_CONTEXT_LIVE_STAGE_PASS {id} {instance}");
            return p;
        }
        assert!(Instant::now() < deadline, "native stage {id} timed out");
        std::thread::sleep(Duration::from_millis(120));
    }
}
fn answer(p: &Value, id: &str) -> String {
    let r = p["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == id)
        .unwrap();
    p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["runId"] == r["id"] && m["role"] == "assistant")
        .filter_map(|m| m["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
fn native(p: &Value, id: &str) -> String {
    let r = p["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == id)
        .unwrap();
    p["providerThreads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pt| pt["id"] == r["providerThreadId"])
        .unwrap()["nativeThreadRef"]["nativeId"]
        .as_str()
        .unwrap()
        .into()
}

struct ReferenceProcess(std::process::Child);
impl Drop for ReferenceProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start_reference(
    binary: &str,
    profile: &std::path::Path,
    evidence: &std::path::Path,
) -> (Connection, ReferenceProcess) {
    use std::process::{Command, Stdio};
    std::fs::create_dir_all(profile).unwrap();
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("20261106.4262"));
    let issued = Command::new(binary)
        .env("T3CODE_HOME", profile)
        .args([
            "auth",
            "session",
            "issue",
            "--subject",
            "fresh-context-reference",
            "--ttl",
            "1h",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(
        issued.status.success(),
        "Reference auth failed: {}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let token = String::from_utf8(issued.stdout).unwrap().trim().to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let log = std::fs::File::create(evidence.join("reference-server.log")).unwrap();
    let child = Command::new(binary)
        .env("T3CODE_HOME", profile)
        .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let mut process = ReferenceProcess(child);
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if t3_protocol::fetch_descriptor(&base).is_ok() {
            break;
        }
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "Reference exited; inspect reference-server.log"
        );
        assert!(Instant::now() < deadline, "Reference startup timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
    signals::shield_client_thread_from_sigchld();
    let connection = Connection::new(
        &base,
        None,
        t3_protocol::Credential::bearer(&token).unwrap(),
    )
    .unwrap();
    (connection, process)
}
type NativeBackend = (
    Connection,
    Option<Arc<State>>,
    Option<ReferenceProcess>,
    PathBuf,
    PathBuf,
);
fn native_backend() -> NativeBackend {
    assert_eq!(
        std::env::var("O3_FRESH_CONTEXT_CONTAINER").as_deref(),
        Ok("1")
    );
    assert!(
        std::path::Path::new("/run/.containerenv").is_file(),
        "rootless container required"
    );
    let output = PathBuf::from(std::env::var("O3_FRESH_CONTEXT_EVIDENCE").unwrap());
    assert_eq!(output, PathBuf::from("/evidence"));
    let dir = PathBuf::from("/tmp/fresh-context-profile");
    let (c, state, reference) = if let Ok(binary) = std::env::var("O3_FRESH_CONTEXT_REFERENCE") {
        let (connection, process) = start_reference(&binary, &dir, &output);
        (connection, None, Some(process))
    } else {
        let state = Arc::new(State::open(&dir).unwrap());
        let claude = Arc::new(ClaudeProvider::discover().expect("installed Claude"));
        let providers = Arc::new(Providers {
            claude: Some(claude),
            codex: Arc::new(CodexProvider::default()),
        });
        *state.provider.lock().unwrap() = Some(providers);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let serving = state.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    o3_server::serve(
                        tokio::net::TcpListener::from_std(listener).unwrap(),
                        serving,
                    )
                    .await
                    .unwrap()
                })
        });
        signals::shield_client_thread_from_sigchld();
        let secret = state
            .issue_pairing(Some("isolated-native-acceptance"), 60_000)
            .unwrap()["credential"]
            .as_str()
            .unwrap()
            .to_owned();
        let c = pair_bearer(
            &base,
            &secret,
            None,
            &PairingClient::default(),
            &ConnectionOptions::default(),
        )
        .unwrap()
        .connection(&ConnectionOptions::default())
        .unwrap();
        (c, Some(state), None)
    };
    (c, state, reference, dir, output)
}

#[test]
#[ignore = "eight real Claude/Codex turns; execute with tools/live-fresh-context inside rootless podman"]
fn native_resets_forget_prior_context_but_keep_history_and_new_continuations() {
    let (c, state, _reference, dir, output) = native_backend();
    let engine = if state.is_some() { "o3" } else { "t3-4262" };
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Fresh context acceptance","workspaceRoot":workspace})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","createdBy":"user","creationSource":"web","branch":null,"worktreePath":null,"commandId":"create","threadId":"t","projectId":"p","title":"Fresh context acceptance","runtimeMode":"approval-required","interactionMode":"default","modelSelection":{"instanceId":"claudeAgent","model":"haiku"}})).unwrap();
    let words = (0..3)
        .map(|_| {
            format!(
                "WORD{}",
                o3_server::store::random_id().unwrap().replace('-', "")
            )
        })
        .collect::<Vec<_>>();
    let p = run(
        &c,
        "seed",
        "claudeAgent",
        "haiku",
        &format!(
            "Remember codeword {}. Reply READY only. Do not use tools.",
            words[0]
        ),
        false,
    );
    let old_claude = native(&p, "seed");
    let p = run(
        &c,
        "recall-old",
        "claudeAgent",
        "haiku",
        "Reply only with the codeword from the earlier user message. Do not use tools.",
        false,
    );
    assert!(answer(&p, "recall-old").contains(&words[0]));
    assert_eq!(native(&p, "recall-old"), old_claude);
    let prompt = |word: &str| {
        format!("For future messages, remember new codeword {word}. Did any EARLIER user message before this one name a different codeword? Reply OLD:<that earlier word> or OLD:NONE if no such earlier message is available. Do not use tools.")
    };
    let p = run(
        &c,
        "clear-claude",
        "claudeAgent",
        "haiku",
        &prompt(&words[1]),
        true,
    );
    assert!(
        answer(&p, "clear-claude").contains("NONE"),
        "fresh Claude unexpectedly recalls history"
    );
    assert!(!answer(&p, "clear-claude").contains(&words[0]));
    let fresh_claude = native(&p, "clear-claude");
    assert_ne!(fresh_claude, old_claude);
    let p=run(&c,"recall-claude","claudeAgent","haiku","Reply only with the NEW codeword introduced in the previous user message. Do not use tools.",false);
    assert!(answer(&p, "recall-claude").contains(&words[1]));
    assert_eq!(native(&p, "recall-claude"), fresh_claude);
    let p = run(
        &c,
        "switch-codex",
        "codex",
        "gpt-6.1-sol",
        "Reply only with the newest codeword introduced in this conversation. Do not use tools.",
        false,
    );
    assert!(answer(&p, "switch-codex").contains(&words[1]));
    assert!(!answer(&p, "switch-codex").contains(&words[0]));
    let old_codex = native(&p, "switch-codex");
    let p = run(
        &c,
        "clear-codex",
        "codex",
        "gpt-6.1-sol",
        &prompt(&words[2]),
        true,
    );
    assert!(
        answer(&p, "clear-codex").contains("NONE"),
        "fresh Codex unexpectedly recalls history"
    );
    assert!(!answer(&p, "clear-codex").contains(&words[1]));
    let fresh_codex = native(&p, "clear-codex");
    assert_ne!(fresh_codex, old_codex);
    let p=run(&c,"recall-codex","codex","gpt-6.1-sol","Reply only with the NEW codeword introduced in the previous user message. Do not use tools.",false);
    assert!(answer(&p, "recall-codex").contains(&words[2]));
    assert_eq!(native(&p, "recall-codex"), fresh_codex);
    let p = run(
        &c,
        "return-claude",
        "claudeAgent",
        "haiku",
        "Reply only with the newest codeword introduced in this conversation. Do not use tools.",
        false,
    );
    let response = answer(&p, "return-claude");
    assert!(response.contains(&words[2]));
    assert!(!response.contains(&words[0]) && !response.contains(&words[1]));
    assert_ne!(native(&p, "return-claude"), fresh_claude);
    assert!(
        p["messages"].to_string().contains(&words[0]),
        "reset must retain visible history"
    );
    let marked = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "user_message" && i["freshContext"] == true)
        .count();
    assert_eq!(marked, 2);
    std::fs::write(
        output.join("projection.json"),
        serde_json::to_vec_pretty(&p).unwrap(),
    )
    .unwrap();
    if let Some(state) = state {
        let db = state.db.lock().unwrap();
        let mut copy = rusqlite::Connection::open(output.join("o3.sqlite")).unwrap();
        rusqlite::backup::Backup::new(&db, &mut copy)
            .unwrap()
            .run_to_completion(128, Duration::from_millis(10), None)
            .unwrap();
    } else {
        let db = rusqlite::Connection::open_with_flags(
            dir.join("userdata/statev2.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut copy = rusqlite::Connection::open(output.join("t3-reference.sqlite")).unwrap();
        rusqlite::backup::Backup::new(&db, &mut copy)
            .unwrap()
            .run_to_completion(128, Duration::from_millis(10), None)
            .unwrap();
        let migration: i64 = copy
            .query_row(
                "SELECT max(migration_id) FROM effect_sql_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(migration, 80);
    }
    std::fs::write(output.join("result.json"),serde_json::to_vec_pretty(&json!({"engine":engine,"nativeTurns":8,"freshMarkers":marked,"claudeReset":true,"codexReset":true,"oldHistoryVisible":true,"postResetSwitch":true,"nativeIdentityChanges":true})).unwrap()).unwrap();
    println!("FRESH_CONTEXT_NATIVE_ACCEPTANCE_PASS: eight real turns, fresh identities, forgetting, continuation, provider switch and visible history");
}

fn capture_native_task_text() {
    fn visit(dir: &std::path::Path, messages: &mut Vec<Value>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                visit(&entry.path(), messages);
            } else if kind.is_file() && entry.path().extension().is_some_and(|v| v == "jsonl") {
                let Ok(text) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                for line in text.lines() {
                    let Ok(record) = serde_json::from_str::<Value>(line) else {
                        continue;
                    };
                    if !matches!(record["type"].as_str(), Some("user" | "assistant")) {
                        continue;
                    }
                    let content = &record["message"]["content"];
                    let text = if let Some(text) = content.as_str() {
                        text.to_owned()
                    } else {
                        content
                            .as_array()
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter(|b| b["type"] == "text")
                                    .filter_map(|b| b["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default()
                    };
                    if text.contains("codeword")
                        || text.contains("WORD")
                        || text == "READY"
                        || text == "OLD:NONE"
                    {
                        messages.push(json!({"sessionId":record["sessionId"],"type":record["type"],"text":text}));
                    }
                }
            }
        }
    }
    let mut messages = Vec::new();
    visit(
        std::path::Path::new("/tmp/home/.claude/projects"),
        &mut messages,
    );
    std::fs::write(
        "/evidence/scheduled-native-text.json",
        serde_json::to_vec_pretty(&messages).unwrap(),
    )
    .unwrap();
}

fn task_turn(c: &Connection, id: &str) -> (Value, Value) {
    let shell = || {
        c.request_json("GET", "/api/orchestration/shell", None, true)
            .unwrap()
    };
    let before = shell()["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect::<Vec<_>>();
    let task = c.rpc("scheduledTasks.runNow", json!({"id":id})).unwrap()["task"].clone();
    assert_eq!(task["lastRunStatus"], "succeeded", "{task}");
    let thread = task["threadId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            shell()["threads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| !before.contains(&t["id"]))
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        });
    let deadline = Instant::now() + Duration::from_secs(100);
    loop {
        let p = c
            .rpc(
                "orchestration.getThreadProjection",
                json!({"threadId":thread}),
            )
            .unwrap();
        let runs = p["runs"].as_array().unwrap();
        if let Some(run) = runs.last() {
            assert!(
                !matches!(
                    run["status"].as_str(),
                    Some("failed" | "cancelled" | "interrupted")
                ),
                "{run}"
            );
            if run["status"] == "completed" {
                std::fs::write(
                    format!("/evidence/scheduled-{id}-{}.json", task["runCount"]),
                    serde_json::to_vec_pretty(&p).unwrap(),
                )
                .unwrap();
                capture_native_task_text();
                println!("SCHEDULED_NATIVE_STAGE_PASS {id} {}", task["runCount"]);
                return (task, p);
            }
        }
        assert!(Instant::now() < deadline, "Scheduled native turn timed out");
        std::thread::sleep(Duration::from_millis(120));
    }
}
fn last_answer(p: &Value) -> String {
    let runs = p["runs"].as_array().unwrap();
    answer(p, runs.last().unwrap()["userMessageId"].as_str().unwrap())
}

#[test]
#[ignore = "real scheduled CLI turns; use tools/live-fresh-context --scheduled-tasks"]
fn scheduled_tasks_native_conversation_modes() {
    let (c, state, _reference, dir, output) = native_backend();
    let engine = if state.is_some() { "o3" } else { "t3-4262" };
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Scheduled acceptance","workspaceRoot":workspace})).unwrap();
    let words = (0..2)
        .map(|_| {
            format!(
                "WORD{}",
                o3_server::store::random_id().unwrap().replace('-', "")
            )
        })
        .collect::<Vec<_>>();
    let mut input = json!({"id":"bound","title":"Scheduled acceptance","prompt":format!("Remember codeword {}. Reply READY only. Do not use tools.",words[0]),"enabled":false,"schedule":{"type":"interval","everyMs":60000},"projectId":"p","threadMode":"continue","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"claudeAgent","model":"haiku"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"agent","creationSource":"server"});
    let first = c.rpc("scheduledTasks.upsert", input.clone()).unwrap()["task"].clone();
    assert_eq!(first["threadMode"], "continue");
    assert!(first["threadId"].is_null());
    let (first, p) = task_turn(&c, "bound");
    assert!(last_answer(&p).contains("READY"));
    let first_native = native(
        &p,
        p["runs"].as_array().unwrap().last().unwrap()["userMessageId"]
            .as_str()
            .unwrap(),
    );
    input["prompt"] =
        json!("Reply only with the codeword from the earlier user message. Do not use tools.");
    c.rpc("scheduledTasks.upsert", input.clone()).unwrap();
    let (second, p) = task_turn(&c, "bound");
    assert_eq!(second["threadId"], first["threadId"]);
    assert!(last_answer(&p).contains(&words[0]));
    assert_eq!(
        native(
            &p,
            p["runs"].as_array().unwrap().last().unwrap()["userMessageId"]
                .as_str()
                .unwrap()
        ),
        first_native
    );
    input["threadMode"] = json!("fresh_session");
    input["modelSelection"] = json!({"instanceId":"codex","model":"gpt-6.1-sol"});
    input["prompt"]=json!(format!("For future messages, remember new codeword {}. Did any EARLIER user message before this one name a different codeword? Reply OLD:<that earlier word> or OLD:NONE if no such earlier message is available. Do not use tools.",words[1]));
    let updated = c.rpc("scheduledTasks.upsert", input.clone()).unwrap()["task"].clone();
    assert_eq!(updated["threadId"], first["threadId"]);
    let (third, p) = task_turn(&c, "bound");
    assert_eq!(third["threadId"], first["threadId"]);
    assert!(last_answer(&p).contains("NONE"), "{}", last_answer(&p));
    assert!(!last_answer(&p).contains(&words[0]));
    let run = p["runs"].as_array().unwrap().last().unwrap();
    assert_eq!(run["freshContext"], true);
    assert_eq!(run["providerInstanceId"], "claudeAgent");
    assert_ne!(
        native(&p, run["userMessageId"].as_str().unwrap()),
        first_native
    );
    input["threadMode"] = json!("continue");
    input["threadId"] = Value::Null;
    input["prompt"]=json!("Reply only with the NEW codeword introduced in the previous user message. Do not use tools.");
    c.rpc("scheduledTasks.upsert", input.clone()).unwrap();
    let (fourth, p) = task_turn(&c, "bound");
    assert_eq!(fourth["threadId"], first["threadId"]);
    assert!(last_answer(&p).contains(&words[1]));
    assert_eq!(p["messages"].as_array().unwrap().len(), 8);
    for message in p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
    {
        assert_eq!(message["scheduledTaskId"], "bound");
        assert_eq!(message["createdBy"], "agent");
        assert_eq!(message["creationSource"], "server");
    }
    let items = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "user_message")
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 4);
    assert!(items.iter().all(|i| i["scheduledTaskId"] == "bound"));
    std::fs::write(
        output.join("scheduled-projection.json"),
        serde_json::to_vec_pretty(&p).unwrap(),
    )
    .unwrap();
    input["id"] = json!("new");
    input["threadMode"] = json!("new_thread");
    input["modelSelection"] = json!({"instanceId":"claudeAgent","model":"haiku"});
    input["prompt"] = json!("Use no tools. Reply with only READY.");
    c.rpc("scheduledTasks.upsert", input).unwrap();
    let (new_first, p1) = task_turn(&c, "new");
    let (new_second, p2) = task_turn(&c, "new");
    assert!(new_first["threadId"].is_null());
    assert!(new_second["threadId"].is_null());
    assert_ne!(p1["thread"]["id"], p2["thread"]["id"]);
    assert_ne!(p1["thread"]["id"], first["threadId"]);
    assert_eq!(p1["messages"].as_array().unwrap().len(), 2);
    assert_eq!(p2["messages"].as_array().unwrap().len(), 2);
    let mut copy = rusqlite::Connection::open(output.join("scheduled.sqlite")).unwrap();
    if let Some(state) = &state {
        rusqlite::backup::Backup::new(&state.db.lock().unwrap(), &mut copy)
            .unwrap()
            .run_to_completion(128, Duration::ZERO, None)
            .unwrap();
    } else {
        let source = rusqlite::Connection::open(dir.join("userdata/statev2.sqlite")).unwrap();
        rusqlite::backup::Backup::new(&source, &mut copy)
            .unwrap()
            .run_to_completion(128, Duration::ZERO, None)
            .unwrap();
    }
    std::fs::write(output.join("scheduled-result.json"),serde_json::to_vec_pretty(&json!({"engine":engine,"nativeTurns":6,"firstRunBinds":true,"continuesSameSession":true,"freshSessionForgets":true,"boundProviderPreserved":true,"newThreadRunsUnbound":true,"taskMessageProvenance":true,"oldHistoryVisible":true})).unwrap()).unwrap();
    println!("SCHEDULED_NATIVE_ACCEPTANCE_PASS: bound continuation, fresh-session forgetting, provider selection, new-thread isolation and task provenance");
}

#[test]
#[ignore = "exports a disposable native acceptance database inside rootless podman; no native turns"]
fn shared_native_projection_export() {
    assert_eq!(
        std::env::var("O3_FRESH_CONTEXT_CONTAINER").as_deref(),
        Ok("1")
    );
    assert!(std::path::Path::new("/run/.containerenv").is_file());
    let dir = PathBuf::from("/tmp/shared-export-profile");
    std::fs::create_dir(&dir).unwrap();
    std::fs::copy("/input/native.sqlite", dir.join("o3.sqlite")).unwrap();
    let state = State::open(&dir).unwrap();
    let shared = rusqlite::Connection::open("/evidence/statev2.sqlite").unwrap();
    shared
        .execute_batch(include_str!("fixtures/t3-statev2-80.sql"))
        .unwrap();
    drop(shared);
    let mut shared = o3_server::t3_shared::open(std::path::Path::new("/evidence"))
        .unwrap()
        .unwrap();
    let exported = o3_server::t3_shared::export(&state, &mut shared).unwrap();
    assert!(exported > 0);
    let marked:i64=shared.query_row("SELECT count(*) FROM orchestration_v2_projection_runs WHERE json_extract(payload_json,'$.freshContext')=1",[],|r|r.get(0)).unwrap();
    assert_eq!(marked, 2);
    let messages: i64 = shared
        .query_row(
            "SELECT count(*) FROM orchestration_v2_projection_messages",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(messages, 16);
    std::fs::write("/evidence/shared-result.json",serde_json::to_vec_pretty(&json!({"exportedRecords":exported,"freshRuns":marked,"messages":messages,"migrationHead":80})).unwrap()).unwrap();
    if let Ok(binary) = std::env::var("O3_FRESH_CONTEXT_REFERENCE") {
        drop(shared);
        let reference_profile = PathBuf::from("/tmp/shared-handover-reference");
        let userdata = reference_profile.join("userdata");
        std::fs::create_dir_all(&userdata).unwrap();
        std::fs::copy("/evidence/statev2.sqlite", userdata.join("statev2.sqlite")).unwrap();
        // Both servers take turns owning the same profile; retain O3's private records and cursor.
        let mut local = rusqlite::Connection::open(userdata.join("o3.sqlite")).unwrap();
        rusqlite::backup::Backup::new(&state.db.lock().unwrap(), &mut local)
            .unwrap()
            .run_to_completion(128, Duration::ZERO, None)
            .unwrap();
        drop(local);
        let (client, process) = start_reference(
            &binary,
            &reference_profile,
            std::path::Path::new("/evidence"),
        );
        let before = projection(&client);
        assert_eq!(before["messages"].as_array().unwrap().len(), 16);
        assert_eq!(
            before["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["freshContext"] == true)
                .count(),
            2
        );
        client.rpc("orchestration.dispatchCommand", json!({"type":"thread.metadata.update","commandId":"reference-title","threadId":"t","title":"Renamed by T3 nightly"})).unwrap();
        client.rpc("projects.mutate", json!({"type":"project.update","commandId":"reference-project","projectId":"p","pinned":true,"archived":true})).unwrap();
        assert_eq!(
            projection(&client)["thread"]["title"],
            "Renamed by T3 nightly"
        );
        drop(client);
        drop(process);
        let recovered = Arc::new(State::open(&userdata).unwrap());
        let mut shared = o3_server::t3_shared::start(&recovered).unwrap().unwrap();
        let p = o3_server::events::projection(&recovered, "t").unwrap();
        assert_eq!(p["thread"]["title"], "Renamed by T3 nightly");
        assert_eq!(p["messages"].as_array().unwrap().len(), 16);
        let project: Value = serde_json::from_str(
            &recovered
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT body FROM records WHERE kind='project' AND id='p'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(!project["pinnedAt"].is_null());
        assert!(!project["archivedAt"].is_null());
        o3_server::events::dispatch(&recovered, &json!({"type":"thread.metadata.update","commandId":"o3-title","threadId":"t","title":"Returned by O3"})).unwrap();
        o3_server::events::mutate_project(&recovered, &json!({"type":"project.update","commandId":"o3-project","projectId":"p","pinned":false,"archived":false})).unwrap();
        assert!(o3_server::t3_shared::export(&recovered, &mut shared).unwrap() > 0);
        let rejected: i64 = recovered
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM t3_rejects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rejected, 0);
        drop(shared);
        drop(recovered);
        let (client, process) = start_reference(
            &binary,
            &reference_profile,
            std::path::Path::new("/evidence"),
        );
        let p = projection(&client);
        assert_eq!(p["thread"]["title"], "Returned by O3");
        assert_eq!(p["messages"].as_array().unwrap().len(), 16);
        drop(client);
        drop(process);
        let source = rusqlite::Connection::open(userdata.join("statev2.sqlite")).unwrap();
        let mut output = rusqlite::Connection::open("/evidence/handover.sqlite").unwrap();
        rusqlite::backup::Backup::new(&source, &mut output)
            .unwrap()
            .run_to_completion(128, Duration::ZERO, None)
            .unwrap();
        std::fs::write("/evidence/handover-result.json", serde_json::to_vec_pretty(&json!({"reference":"v0.1.8-nightly.20261106.4262","migrationHead":80,"o3ToT3":true,"t3ToO3":true,"o3ReturnToT3":true,"projectPinArchivePreserved":true,"messages":16,"rejected":0})).unwrap()).unwrap();
        println!("FRESH_CONTEXT_SHARED_RUNTIME_HANDOVER_PASS: O3 to T3 to O3 to T3, retained history and project flags");
    }
    println!("FRESH_CONTEXT_SHARED_EXPORT_PASS: {exported} records, two fresh runs, sixteen preserved messages");
}
