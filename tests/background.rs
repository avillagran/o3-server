#[path = "support/signals.rs"]
mod signals;
use serde_json::{json, Value};
use t3_protocol::{pair_bearer, ConnectionOptions, PairingClient};
fn first(c: &t3_protocol::Connection, tag: &str, p: Value) -> Value {
    let done = std::cell::Cell::new(false);
    let mut out = None;
    c.subscribe(
        tag,
        p,
        || done.get(),
        |v| {
            if let Some(v) = v {
                out = Some(v);
                done.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    out.unwrap()
}
fn readers(c: &t3_protocol::Connection, working: bool) -> Value {
    let p =
        first(c, "orchestration.subscribeThread", json!({"threadId":"t"}))["projection"].clone();
    let view = t3_protocol::timeline::view(
        &p,
        &json!({}),
        &t3_protocol::timeline::Render::neutral(false),
    )
    .unwrap();
    assert_eq!(view["threadState"]["working"], working, "{view}");
    assert_eq!(view["threadState"]["activeRunId"].is_string(), working);
    assert_eq!(t3_protocol::queue::derive(&p)["items"], json!([]));
    let shell = first(c, "orchestration.subscribeShell", json!({}))["snapshot"].clone();
    let rows = t3_protocol::sidebar::view::rows(&shell, "test", 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["working"], working, "{rows:?}");
    let phase = t3_protocol::awareness::phase(&shell["threads"][0]);
    assert_eq!(phase == Some("running"), working);
    p
}
fn wait(mut f: impl FnMut() -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !f() {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn owned_background_work_and_prebinding_identity_through_real_client() {
    background_cases(&[
        ("claudeAgent", false, false, false),
        ("codex", false, false, false),
        ("claudeAgent", true, false, false),
        ("codex", true, false, false),
        ("claudeAgent", false, true, false),
        ("codex", false, true, false),
    ]);
}
#[test]
fn claude_interrupt_after_result_needs_no_second_terminal() {
    background_cases(&[("claudeAgent", true, false, true)]);
}
#[test]
fn codex_interrupt_after_result_needs_no_second_terminal() {
    background_cases(&[("codex", true, false, true)]);
}
fn background_cases(cases: &[(&str, bool, bool, bool)]) {
    for &(instance, interrupt, legacy, single_terminal) in cases {
        let dir = std::env::temp_dir().join(format!(
            "o3-background-{}",
            o3_server::store::random_id().unwrap()
        ));
        let root = dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        std::fs::write(root.join("proof"), "before\n").unwrap();
        if single_terminal {
            std::fs::write(root.join("single-terminal"), "").unwrap();
        }
        let state = o3_server::store::State::open(&dir.join("state")).unwrap();
        let secret = state.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
            .as_str()
            .unwrap()
            .to_owned();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let handle = std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async move{
            let peer=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/background-peer.py");
            let claude=std::sync::Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
            let codex=std::sync::Arc::new(o3_server::codex::CodexProvider::new(peer,None));
            *state.provider.lock().unwrap()=Some(std::sync::Arc::new(o3_server::provider::Providers{claude:Some(claude.clone()),codex:codex.clone()}));
            tokio::select!{r=o3_server::serve(tokio::net::TcpListener::from_std(listener).unwrap(),std::sync::Arc::new(state))=>r.unwrap(),_=stopped=>{}}
            claude.shutdown().await;codex.shutdown().await;
        });
        });
        signals::shield_client_thread_from_sigchld();
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

        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root,"scripts":[]})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Background","modelSelection":{"instanceId":instance,"model":"haiku","options":[]},"runtimeMode":"full-access","interactionMode":"default","worktreePath":root,"branch":null})).unwrap();
        if legacy {
            let db = rusqlite::Connection::open(dir.join("state/o3.sqlite")).unwrap();
            let key = if instance == "codex" {
                "codexNativeThreadId"
            } else {
                "activeProviderThreadId"
            };
            db.execute("UPDATE records SET body=json_set(body,?,'native-bg') WHERE kind='thread' AND id='t'",[format!("$.{key}")]).unwrap();
        }
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"m","messageId":"m","threadId":"t","text":"background","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let snapshot = first(&c, "orchestration.subscribeThread", json!({"threadId":"t"}));
        let p = &snapshot["projection"];
        let id = p["thread"]["activeProviderThreadId"].clone();
        assert!(id.is_string());
        assert_eq!(p["providerThreads"][0]["id"], id);
        assert_eq!(
            p["providerThreads"][0]["nativeThreadRef"].is_null(),
            !legacy
        );
        assert_eq!(p["attempts"][0]["providerThreadId"], id);
        assert_eq!(p["nodes"][0]["providerThreadId"], id);
        std::fs::write(root.join("bind"), "").unwrap();
        wait(|| root.join("result").exists());
        let projection = || {
            c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap()
        };
        wait(|| {
            projection()["providerThreads"][0]["pendingBackgroundTasks"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        });
        let p = readers(&c, true);
        assert_eq!(p["providerThreads"].as_array().unwrap().len(), 1);
        assert_eq!(p["providerThreads"][0]["id"], id);
        assert_eq!(
            p["providerThreads"][0]["nativeThreadRef"]["nativeId"],
            "native-bg"
        );
        assert_eq!(p["runs"][0]["status"], "running");
        assert!(p["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["status"] == "running"));
        assert!(p["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["status"] == "running"));
        assert!(!p["thread"]["pendingBackgroundTasks"]
            .as_array()
            .unwrap()
            .is_empty());
        let shell = first(&c, "orchestration.subscribeShell", json!({}));
        assert_eq!(
            shell["snapshot"]["threads"][0]["activityRunStatus"],
            "running"
        );
        assert!(!shell["snapshot"]["threads"][0]["pendingBackgroundTasks"]
            .as_array()
            .unwrap()
            .is_empty());
        let seq = first(&c, "orchestration.subscribeThread", json!({"threadId":"t"}))
            ["snapshotSequence"]
            .as_i64()
            .unwrap();
        if interrupt {
            c.rpc("orchestration.dispatchCommand",json!({"type":"run.interrupt","commandId":"interrupt","threadId":"t","runId":p["runs"][0]["id"]})).unwrap();
        } else {
            std::fs::write(root.join("finish"), "").unwrap();
        }
        wait(|| {
            projection()["runs"][0]["status"]
                == if interrupt {
                    "interrupted"
                } else {
                    "completed"
                }
        });
        assert!(projection()["providerThreads"][0]["pendingBackgroundTasks"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(projection()["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] != "running"));
        if instance == "claudeAgent" && !interrupt {
            assert_eq!(projection()["subagents"][0]["status"], "cancelled");
        }
        let done = std::cell::Cell::new(false);
        let mut event = None;
        c.subscribe(
            "orchestration.subscribeThread",
            json!({"threadId":"t","afterSequence":seq}),
            || done.get(),
            |v| {
                if let Some(v) = v {
                    if v["event"]["type"] == "provider-thread.updated"
                        && v["event"]["payload"]["pendingBackgroundTasks"]
                            .as_array()
                            .is_some_and(|a| a.is_empty())
                    {
                        event = Some(v);
                        done.set(true);
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        let event = event.unwrap();
        let mut mirror = p.clone();
        assert!(t3_protocol::projection::apply(&mut mirror, &event["event"]));
        assert_eq!(
            mirror["providerThreads"][0]["pendingBackgroundTasks"],
            json!([])
        );
        let final_p = readers(&c, false);
        assert_eq!(
            final_p["providerThreads"][0]["pendingBackgroundTasks"],
            json!([])
        );
        assert_eq!(event["event"]["type"], "provider-thread.updated");
        assert!(
            event["event"]["payload"]["pendingBackgroundTasks"]
                .as_array()
                .unwrap()
                .len()
                < p["providerThreads"][0]["pendingBackgroundTasks"]
                    .as_array()
                    .unwrap()
                    .len()
        );
        let _ = stop.send(());
        handle.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
#[ignore = "one short installed Claude Haiku background-shell turn"]
async fn live_claude_background_shell() {
    use o3_server::provider::{Provider, ProviderEvent, RunInput};
    let dir = std::env::temp_dir().join(format!(
        "o3-live-background-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = o3_server::claude::ClaudeProvider::discover().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let (_control, controls) = tokio::sync::mpsc::channel(8);
    let (_cancel, cancel) = tokio::sync::watch::channel(false);
    let input=RunInput{provider_instance_id:"claudeAgent".into(),native_session_id:None,thread_id:"background-live".into(),run_id:"live".into(),cwd:dir.clone(),text:"Use Bash with run_in_background=true to execute: sleep 2; printf BACKGROUND_OK. Reply STARTED immediately after launching, without waiting or calling another tool.".into(),model_selection:json!({"instanceId":"claudeAgent","model":"haiku"}),runtime_mode:"full-access".into(),interaction_mode:"default".into(),attachment_root:dir.clone(),attachments:json!([]),context:None};
    let read = async {
        let mut task = false;
        let mut done = false;
        let mut completed = false;
        while let Some(e) = rx.recv().await {
            match e {
                ProviderEvent::Native(v) if v["type"] == "task" => {
                    let f = &v["frame"];
                    if f["subtype"] == "task_started"
                        || f["subtype"] == "background_tasks_changed"
                            && f["tasks"].as_array().is_some_and(|a| !a.is_empty())
                    {
                        task = true;
                    }
                    if f["subtype"] == "task_notification" {
                        done = true;
                    }
                }
                ProviderEvent::Completed => {
                    assert!(
                        task && done,
                        "completion requires observed native background start and end"
                    );
                    completed = true;
                }
                ProviderEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(completed);
    };
    let (r, ()) = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        tokio::join!(provider.execute(input, tx, controls, cancel), read)
    })
    .await
    .unwrap();
    r.unwrap();
    provider.shutdown().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn restart_closes_background_ownership_without_inventing_a_native_binding() {
    for bound in [false, true] {
        let dir = std::env::temp_dir().join(format!(
            "o3-bg-recovery-{}",
            o3_server::store::random_id().unwrap()
        ));
        let s = o3_server::store::State::open(&dir).unwrap();
        {
            let db = s.db.lock().unwrap();
            for (kind, id, v) in [
                (
                    "thread",
                    "t",
                    json!({"id":"t","providerInstanceId":"claudeAgent","activeProviderThreadId":"reserved","pendingBackgroundTasks":[{"kind":"command","taskId":"bg"}]}),
                ),
                (
                    "providerThreads",
                    "reserved",
                    json!({"id":"reserved","appThreadId":"t","nativeThreadRef":if bound {json!({"nativeId":"native-bg"})}else{Value::Null},"pendingBackgroundTasks":[{"kind":"command","taskId":"bg"}]}),
                ),
                (
                    "runs",
                    "r",
                    json!({"id":"r","threadId":"t","providerThreadId":"reserved","status":"running"}),
                ),
                (
                    "backgroundTasks",
                    "bg",
                    json!({"id":"bg","runId":"r","taskId":"bg","kind":"command","status":"running"}),
                ),
            ] {
                db.execute(
                    "INSERT INTO records(kind,id,thread_id,body) VALUES(?,?,'t',?)",
                    rusqlite::params![kind, id, v.to_string()],
                )
                .unwrap();
            }
        }
        drop(s);
        let s = o3_server::store::State::open(&dir).unwrap();
        o3_server::recovery::recover(&mut s.db.lock().unwrap()).unwrap();
        let p = o3_server::events::projection(&s, "t").unwrap();
        let view = t3_protocol::timeline::view(
            &p,
            &json!({}),
            &t3_protocol::timeline::Render::neutral(false),
        )
        .unwrap();
        assert_eq!(view["threadState"]["working"], bound);
        assert_eq!(t3_protocol::queue::derive(&p)["items"], json!([]));
        assert_eq!(
            p["runs"][0]["status"],
            if bound { "starting" } else { "failed" }
        );
        assert!(p["providerThreads"][0]["pendingBackgroundTasks"]
            .as_array()
            .unwrap()
            .is_empty());
        let db = s.db.lock().unwrap();
        let body: String = db
            .query_row(
                "SELECT body FROM records WHERE kind='backgroundTasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "failed");
        assert!(v["resolutionReason"].is_string());
        drop(db);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
