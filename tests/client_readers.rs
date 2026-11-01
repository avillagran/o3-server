mod support;
use serde_json::{json, Value};
use t3_protocol::{
    api::orchestration::{self, Command, ThreadLaunch, WorkspaceStrategy},
    timeline,
};

#[test]
fn real_builders_and_readers_consume_server_output() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(std::sync::Arc::new(o3_server::provider::ScriptedProvider))
    });
    let c = s.connect();
    let workspace = s.dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate", json!({"type":"project.create","commandId":"p","projectId":"p","title":"Readers","workspaceRoot":workspace})).unwrap();
    let launch = ThreadLaunch {
        command_id: "launch".into(),
        project_id: "p".into(),
        title: "Reader evidence".into(),
        model_selection: json!({"instanceId":"codex","model":"test"}),
        runtime_mode: "approval-required".into(),
        interaction_mode: "default".into(),
        workspace_strategy: WorkspaceStrategy::Root { branch: None },
        creation_source: None,
        thread_id: Some("t".into()),
        reuse_existing_thread: None,
        generate_title: None,
        initial_message: None,
    };
    orchestration::launch_thread(&c, &launch).unwrap();
    let pin = Command::sidebar("settle", "settle", "t", None).unwrap();
    orchestration::dispatch(&c, &pin).unwrap();
    let p = orchestration::thread_projection(&c, "t").unwrap();
    assert_eq!(p["thread"]["title"], "Reader evidence");
    assert!(p["thread"]["settledAt"].is_string());
    assert_eq!(t3_protocol::approvals::pending(&p), json!([]));
    assert_eq!(t3_protocol::queue::derive(&p)["items"], json!([]));
    assert!(
        timeline::view(&p, &json!({}), &timeline::Render::neutral(false))
            .unwrap()
            .is_object()
    );
    let stop = std::cell::Cell::new(false);
    let mut shell = Value::Null;
    c.subscribe(
        "orchestration.subscribeShell",
        json!({}),
        || stop.get(),
        |v| {
            if let Some(v) = v {
                shell = v["snapshot"].clone();
                stop.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    let rows = t3_protocol::sidebar::view::rows(&shell, "env", 0).unwrap();
    assert_eq!(rows.len(), 1);
    let envs = vec![json!({"id":"env","name":"test","state":"ready","shell":shell})];
    assert!(
        t3_protocol::awareness::view(&t3_protocol::awareness::Input {
            environments: &envs,
            now: 0,
            baseline: None,
            on_screen: None,
            preferences: Default::default(),
            lifecycle: "cold",
            locale: "en"
        })
        .unwrap()
        .is_object()
    );
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let raw = c.rpc("server.getSettings", json!({})).unwrap();
    assert!(
        t3_protocol::provider_instances::projection(&raw, &config, true)["instances"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(t3_protocol::models::catalog(&config)["providers"].is_array());
    assert!(t3_protocol::scoped_settings::target_view(
        &json!({"environmentId":"env"}),
        &config,
        &raw,
        None,
        None,
        true
    )["fields"]
        .is_array());
    assert!(t3_protocol::storage_settings::capabilities(&config).is_object());
    use orchestration::{DispatchMode, MessageDispatch};
    orchestration::dispatch(
        &c,
        &Command::MessageDispatch(MessageDispatch::new(
            "send",
            "t",
            "m",
            "Reader message",
            "web",
        )),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let p = loop {
        let p = orchestration::thread_projection(&c, "t").unwrap();
        if !t3_protocol::approvals::pending(&p)
            .as_array()
            .unwrap()
            .is_empty()
        {
            break p;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let pending = t3_protocol::approvals::pending(&p);
    assert_eq!(pending[0]["responseCapability"], "live");
    let mut queued = MessageDispatch::new("queue", "t", "q", "Queued message", "web");
    queued.dispatch_mode = DispatchMode::QueueAfterActive;
    orchestration::dispatch(&c, &Command::MessageDispatch(queued)).unwrap();
    let p = orchestration::thread_projection(&c, "t").unwrap();
    assert_eq!(
        t3_protocol::queue::derive(&p)["items"][0]["text"],
        "Queued message"
    );
    let approve = Command::approve(
        "approve",
        &json!({"thread":"t","runtimeRequestId":pending[0]["requestId"],"decision":"accept"}),
        &p,
    )
    .unwrap();
    orchestration::dispatch(&c, &approve).unwrap();
}
