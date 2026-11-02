use o3_server::{
    claude::ClaudeProvider,
    codex::CodexProvider,
    provider::{Provider, ProviderEvent, RunInput},
};
use serde_json::json;
use tokio::sync::{mpsc, watch};
async fn turn(
    p: &dyn Provider,
    instance: &str,
    dir: &std::path::Path,
    fork: Option<&str>,
    resume: Option<String>,
    text: &str,
) -> String {
    let (out, mut events) = mpsc::channel(16);
    let (_control, controls) = mpsc::channel(8);
    let (_cancel, cancel) = watch::channel(false);
    let input = RunInput {
        provider_instance_id: instance.into(),
        native_session_id: resume.clone(),
        thread_id: if fork.is_some() { "fork" } else { "source" }.into(),
        run_id: "run".into(),
        cwd: dir.into(),
        text: text.into(),
        model_selection: json!({"instanceId":instance,"model":if resume.is_some() {"sonnet"} else {"haiku"},"nativeForkSource":fork}),
        runtime_mode: "full-access".into(),
        interaction_mode: "default".into(),
        attachment_root: dir.into(),
        attachments: json!([]),
        context: None,
    };
    let read = async {
        let mut id = String::new();
        let mut complete = false;
        let mut text = String::new();
        while let Some(e) = events.recv().await {
            match e {
                ProviderEvent::Native(v) if v["type"] == "session" => {
                    id = v["session_id"].as_str().unwrap().into()
                }
                ProviderEvent::Codex(v) if v["method"] == "o3/binding" => {
                    id = v["params"]["threadId"].as_str().unwrap().into()
                }
                ProviderEvent::TextDelta(t) => text.push_str(&t),
                ProviderEvent::Completed => complete = true,
                ProviderEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(complete);
        if fork.is_some() && instance == "claudeAgent" && text.len() > 2 {
            assert!(text.contains("SAPPHIRE-813"), "{text}");
        }
        id
    };
    let (result, id) = tokio::join!(p.execute(input, out, controls, cancel), read);
    result.unwrap();
    id
}
#[tokio::test]
async fn native_forks_are_distinct_and_resume_preserves_identity() {
    let dir = std::env::temp_dir().join(format!("o3-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lifecycle-peer.py");
    let claude = ClaudeProvider::new(peer.clone());
    let codex = CodexProvider::new(peer, None);
    for (p, instance) in [
        (&claude as &dyn Provider, "claudeAgent"),
        (&codex as &dyn Provider, "codex"),
    ] {
        assert_eq!(turn(p, instance, &dir, None, None, "seed").await, "native");
        assert_eq!(
            turn(p, instance, &dir, None, Some("native".into()), "new model").await,
            "native"
        );
        assert_eq!(
            turn(p, instance, &dir, Some("native"), None, "continue").await,
            "native-fork"
        );
    }
    claude.shutdown().await;
    codex.shutdown().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "uses installed Claude and two short Haiku turns"]
async fn live_claude_native_fork() {
    let dir = std::env::temp_dir().join(format!("o3-live-fork-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let provider = ClaudeProvider::discover().unwrap();
    let source = turn(
        &provider,
        "claudeAgent",
        &dir,
        None,
        None,
        "Remember code SAPPHIRE-813. Reply OK only.",
    )
    .await;
    let fork = turn(
        &provider,
        "claudeAgent",
        &dir,
        Some(&source),
        None,
        "Reply with the code I gave you, no tools.",
    )
    .await;
    assert!(!source.is_empty());
    assert_ne!(source, fork);
    provider.shutdown().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn claude_init_guard_emits_no_started_event_after_cancel_or_control() {
    for cancel_watch in [true, false] {
        let dir = std::env::temp_dir().join(o3_server::store::random_id().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gate-init"), "").unwrap();
        std::fs::write(dir.join("release-interrupt"), "").unwrap();
        let provider = std::sync::Arc::new(
            ClaudeProvider::new(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/concurrent-claude.py"),
            )
            .with_environment(vec![("T3CODE_HOME".into(), dir.to_str().unwrap().into())]),
        );
        let (out, mut events) = mpsc::channel(16);
        let (control, controls) = mpsc::channel(8);
        let (cancel, cancelled) = watch::channel(false);
        let input = RunInput {
            provider_instance_id: "claudeAgent".into(),
            native_session_id: None,
            thread_id: "t".into(),
            run_id: "r".into(),
            cwd: dir.clone(),
            text: "fixture".into(),
            model_selection: json!({"model":"haiku"}),
            runtime_mode: "full-access".into(),
            interaction_mode: "default".into(),
            attachment_root: dir.clone(),
            attachments: json!([]),
            context: None,
        };
        let p = provider.clone();
        let task = tokio::spawn(async move { p.execute(input, out, controls, cancelled).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !dir.join("init-received").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if cancel_watch {
            cancel.send(true).unwrap();
        } else {
            control
                .send(o3_server::provider::ProviderControl::Interrupt)
                .await
                .unwrap();
        }
        std::fs::write(dir.join("release-init"), "").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        // Keep the receiver alive: a missing guard deterministically queues Started
        // even when the child is reaped before it can read the initial user frame.
        while let Some(event) = events.recv().await {
            assert!(
                !matches!(event, ProviderEvent::Started),
                "cancelled init sent a user message"
            );
        }
        provider.shutdown().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
