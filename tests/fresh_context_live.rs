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
#[test]
#[ignore = "eight real Claude/Codex turns; execute with tools/live-fresh-context inside rootless podman"]
fn native_resets_forget_prior_context_but_keep_history_and_new_continuations() {
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
    let (c, state, _reference) = if let Ok(binary) = std::env::var("O3_FRESH_CONTEXT_REFERENCE") {
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
        .execute_batch(include_str!("fixtures/t3-statev2-73.sql"))
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
    std::fs::write("/evidence/shared-result.json",serde_json::to_vec_pretty(&json!({"exportedRecords":exported,"freshRuns":marked,"messages":messages,"migrationHead":73})).unwrap()).unwrap();
    println!("FRESH_CONTEXT_SHARED_EXPORT_PASS: {exported} records, two fresh runs, sixteen preserved messages");
}
