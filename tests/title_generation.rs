mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use t3_protocol::api::orchestration::{
    self, Command, InitialMessage, ThreadLaunch, WorkspaceStrategy,
};
struct Titles(Arc<Mutex<Vec<Value>>>);
impl Provider for Titles {
    fn execute<'a>(
        &'a self,
        input: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        _controls: tokio::sync::mpsc::Receiver<ProviderControl>,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if input.run_id.starts_with("title-") {
                self.0.lock().unwrap().push(json!({"selection":input.model_selection,"text":input.text,"native":input.native_session_id}));
                tokio::time::sleep(Duration::from_millis(150)).await;
                if input.model_selection["model"] == "fail" {
                    return Err("unavailable".into());
                }
                out.send(ProviderEvent::TextDelta("Semantic reader title".into()))
                    .await
                    .unwrap();
            }
            out.send(ProviderEvent::Completed).await.unwrap();
            Ok(())
        })
    }
}
fn wait(c: &t3_protocol::Connection, f: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let p = orchestration::thread_projection(c, "t").unwrap();
        if f(&p) {
            return p;
        }
        assert!(Instant::now() < deadline, "{p}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn generated_titles_settings_regeneration_and_rename_race_follow_reader_contract() {
    let seen = Arc::new(Mutex::new(vec![]));
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(Arc::new(Titles(seen.clone()))));
    let c = s.connect();
    let workspace = s.dir.join("repo");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Titles","workspaceRoot":workspace})).unwrap();
    c.rpc("server.updateSettings",json!({"patch":{"threadTitleInstructions":"Use concise English","textGenerationModelSelection":{"instanceId":"claudeAgent","model":"title-model","options":[{"id":"effort","value":"low"}]}}})).unwrap();
    let launch = ThreadLaunch {
        command_id: "launch".into(),
        project_id: "p".into(),
        title: "Untitled".into(),
        model_selection: json!({"instanceId":"codex","model":"chat"}),
        runtime_mode: "approval-required".into(),
        interaction_mode: "default".into(),
        workspace_strategy: WorkspaceStrategy::Root { branch: None },
        creation_source: None,
        thread_id: Some("t".into()),
        reuse_existing_thread: None,
        generate_title: Some(true),
        initial_message: Some(InitialMessage {
            message_id: Some("m".into()),
            text: "A long request to fix a bug".into(),
            attachments: vec![],
            context: None,
        }),
    };
    let result = orchestration::launch_thread(&c, &launch).unwrap();
    assert!(result["projection"]["thread"]["titleRegeneration"].is_object());
    let p = wait(&c, |p| {
        p["thread"]["title"] == "Semantic reader title"
            && p["thread"]["titleRegeneration"].is_null()
    });
    assert!(t3_protocol::timeline::view(
        &p,
        &json!({}),
        &t3_protocol::timeline::Render::neutral(false)
    )
    .unwrap()
    .is_object());
    let call = seen.lock().unwrap()[0].clone();
    assert_eq!(call["selection"]["instanceId"], "claudeAgent");
    assert_eq!(call["selection"]["model"], "title-model");
    assert_eq!(call["selection"]["options"][0]["value"], "low");
    assert!(call["text"]
        .as_str()
        .unwrap()
        .contains("Use concise English"));
    assert!(call["native"].is_null());
    let regen = Command::thread_action("regen", &json!({"thread":"t","action":"regenerate-title"}))
        .unwrap();
    orchestration::dispatch(&c, &regen).unwrap();
    let rename = Command::thread_action(
        "rename",
        &json!({"thread":"t","action":"thread-rename","title":"User title"}),
    )
    .unwrap();
    orchestration::dispatch(&c, &rename).unwrap();
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(
        orchestration::thread_projection(&c, "t").unwrap()["thread"]["title"],
        "User title"
    );
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"textGenerationModelSelection":{"instanceId":"codex","model":"fail"}}}),
    )
    .unwrap();
    orchestration::dispatch(
        &c,
        &Command::thread_action(
            "fallback",
            &json!({"thread":"t","action":"regenerate-title"}),
        )
        .unwrap(),
    )
    .unwrap();
    let p = wait(&c, |p| p["thread"]["titleRegeneration"].is_null());
    assert_eq!(p["thread"]["title"], "A long request to fix a bug");
    let mut mirror = result["projection"].clone();
    let mut seq = 0;
    let mut busy = false;
    let mut done = false;
    while let Some((next, event)) =
        o3_server::events::next(&s.state, seq, Some("t"), false).unwrap()
    {
        seq = next;
        if event["type"] == "thread.metadata-updated" {
            assert!(t3_protocol::projection::apply(&mut mirror, &event));
            busy |= mirror["thread"]["titleRegeneration"].is_object();
            done |= busy && mirror["thread"]["titleRegeneration"].is_null();
        }
    }
    assert!(busy && done);
}
