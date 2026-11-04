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
    let mut reused = launch.clone();
    reused.command_id = "reuse-reader".into();
    reused.reuse_existing_thread = Some(true);
    let result = orchestration::launch_thread(&c, &reused).unwrap();
    assert_eq!(result["resumed"], false);
    let replay = orchestration::launch_thread(&c, &reused).unwrap();
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["threadId"], result["threadId"]);

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
    let mut config_stream = support::subscribe(s.connect(), "subscribeServerConfig", json!({}));
    let event = config_stream.next(std::time::Duration::from_secs(2));
    let streamed = &event["config"];
    let mut quota = Value::Null;
    assert!(t3_protocol::quota_sources::apply(&mut quota, &event));
    assert!(t3_protocol::native_settings::snapshot(&streamed["settings"])["fields"].is_array());
    assert!(t3_protocol::scoped_settings::target_view(
        &json!({"environmentId":"env"}),
        streamed,
        &streamed["settings"],
        None,
        None,
        true
    )["fields"]
        .is_array());
    assert!(
        t3_protocol::provider_instances::projection(&streamed["settings"], streamed, true)
            ["instances"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert_eq!(
        t3_protocol::storage_settings::capabilities(streamed)["storageCleanup"],
        true
    );
    assert!(t3_protocol::models::catalog(streamed)["providers"].is_array());
    config_stream.cancel();

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
    for key in ["storageCleanupRun", "worktreesDirectory"] {
        assert_eq!(
            t3_protocol::storage_settings::capabilities(&config)[key],
            true,
            "{key}"
        );
    }
    let project = json!({"id":"p","workspaceRoot":workspace});
    let patch = t3_protocol::scoped_settings::write_patch(
        &json!({"action":"update","key":"sidebarAutoSettleAfterDays","value":7}),
        &config,
        &raw,
        Some(&project),
        true,
    )
    .unwrap();
    let saved = c.rpc("server.updateSettings", patch).unwrap();
    let (effective, _, _) = t3_protocol::scoped_settings::effective(&saved, Some(&project), None);
    assert_eq!(effective["sidebarAutoSettleAfterDays"], 7);
    {
        let (key, value) = ("worktreesDirectory", json!(s.dir.join("trees")));
        let patch = t3_protocol::scoped_settings::write_patch(
            &json!({"action":"update","key":key,"value":value}),
            &config,
            &raw,
            None,
            true,
        )
        .unwrap();
        c.rpc("server.updateSettings", patch).unwrap();
    }
    assert_eq!(
        t3_protocol::storage_settings::capabilities(&config)["storageCleanup"],
        true
    );
    assert_eq!(
        t3_protocol::storage_settings::capabilities(&config)["projectWorktreeCleanup"],
        true
    );
    assert!(t3_protocol::scoped_settings::write_patch(
        &json!({"action":"update","key":"storageCleanup.logsAfterDays","value":14}),
        &config,
        &raw,
        None,
        true
    )
    .is_ok());
    c.rpc("server.runStorageCleanup", json!({})).unwrap();
    orchestration::dispatch(
        &c,
        &Command::sidebar("unsettle", "unsettle", "t", None).unwrap(),
    )
    .unwrap();
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
    let timeline =
        t3_protocol::timeline::view(&p, &json!({}), &timeline::Render::neutral(false)).unwrap();
    assert!(timeline["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "m" && m["recallableText"] == "Reader message"));
    let stop = std::cell::Cell::new(false);
    let mut snap = Value::Null;
    c.subscribe(
        "orchestration.subscribeShell",
        json!({}),
        || stop.get(),
        |v| {
            if let Some(v) = v {
                snap = v["snapshot"].clone();
                stop.set(true);
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        t3_protocol::sidebar::view::rows(&snap, "env", 0).unwrap()[0]["reason"],
        "request"
    );
    let envs = vec![json!({"id":"env","state":"ready","shell":snap})];
    let awareness = t3_protocol::awareness::view(&t3_protocol::awareness::Input {
        environments: &envs,
        now: 0,
        baseline: None,
        on_screen: None,
        preferences: Default::default(),
        lifecycle: "cold",
        locale: "en",
    })
    .unwrap();
    assert_eq!(awareness["card"]["phase"], "waiting_for_approval");
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

#[test]
fn imported_create_is_atomic_idempotent_and_readable() {
    use orchestration::ThreadCreate;
    let s = support::start(|_| {});
    let c = s.connect();
    let mut create = ThreadCreate {
        command_id: "import".into(),
        thread_id: "imported".into(),
        project_id: "p".into(),
        title: "Imported".into(),
        model_selection: json!({"instanceId":"codex","model":"test"}),
        runtime_mode: "approval-required".into(),
        interaction_mode: "default".into(),
        branch: None,
        worktree_path: None,
        created_by: "user".into(),
        creation_source: "web".into(),
        imported_native_thread: Some(
            json!({"ref":{"driver":"codex","nativeId":"native-test","strength":"strong"},"metadata":{"title":"Imported"}}),
        ),
    };
    let command = Command::ThreadCreate(create.clone());
    orchestration::dispatch(&c, &command).unwrap();
    orchestration::dispatch(&c, &command).unwrap();
    let p = orchestration::thread_projection(&c, "imported").unwrap();
    assert_eq!(p["providerThreads"].as_array().unwrap().len(), 1);
    assert_eq!(
        p["providerThreads"][0]["nativeThreadRef"]["nativeId"],
        "native-test"
    );
    assert!(
        timeline::view(&p, &json!({}), &timeline::Render::neutral(false))
            .unwrap()
            .is_object()
    );
    assert_eq!(t3_protocol::queue::derive(&p)["items"], json!([]));
    create.command_id = "bad-import".into();
    create.thread_id = "bad-import".into();
    create.imported_native_thread.as_mut().unwrap()["ref"]["driver"] = json!("claudeAgent");
    assert!(orchestration::dispatch(&c, &Command::ThreadCreate(create)).is_err());
    assert!(orchestration::thread_projection(&c, "bad-import").is_err());
}
