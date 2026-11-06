mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
fn projection(c: &t3_protocol::Connection) -> Value {
    projection_for(c, "t")
}
fn projection_for(c: &t3_protocol::Connection, tid: &str) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":tid}))
        .unwrap()
}
fn send(c: &t3_protocol::Connection, id: &str, text: &str, instance: &str) -> Value {
    send_to(c, "t", id, text, instance)
}
fn send_to(c: &t3_protocol::Connection, tid: &str, id: &str, text: &str, instance: &str) -> Value {
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":tid,"modelSelection":{"instanceId":instance,"model":"test","options":[]},"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(15), || {
        projection_for(c, tid)["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| {
                r["userMessageId"] == id
                    && matches!(r["status"].as_str(), Some("completed" | "failed"))
            })
    }));
    let p = projection_for(c, tid);
    let run = p["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == id)
        .unwrap();
    assert_eq!(run["status"], "completed", "{}", run["error"]);
    run.clone()
}
#[test]
fn account_overlays_share_native_history_but_keep_auth_and_local_runtime_separate() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            )));
    });
    let c = s.connect();
    let shared = s.dir.join("shared");
    let first = s.dir.join("account-a");
    let second = s.dir.join("account-b");
    for (home, owner) in [
        (&shared, "shared"),
        (&first, "account-a"),
        (&second, "account-b"),
    ] {
        std::fs::create_dir(home).unwrap();
        std::fs::write(
            home.join("auth.json"),
            json!({"fixtureOwner":owner}).to_string(),
        )
        .unwrap();
    }
    let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/provider-home-peer.py");
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{
  "first":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":shared,"shadowHomePath":first,"launchArgs":"--expected-owner account-a"}},
  "second":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":shared,"shadowHomePath":second,"launchArgs":"--expected-owner account-b"}}
 }}})).unwrap();
    let workspace = s.dir.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Homes","workspaceRoot":workspace})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Homes","modelSelection":{"instanceId":"first","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let seed = send(&c, "seed", "PRIVATE-PROFILE-WORD", "first");
    let before = projection(&c)["providerThreads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pt| pt["id"] == seed["providerThreadId"])
        .unwrap()["nativeThreadRef"]
        .clone();
    assert_eq!(
        std::fs::read_link(first.join("sessions")).unwrap(),
        shared.join("sessions")
    );
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    for id in ["first", "second"] {
        let provider = config["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["instanceId"] == id)
            .unwrap();
        assert_eq!(
            provider["continuation"]["groupKey"],
            format!("codex:home:{}", shared.display())
        );
    }

    // A second app thread remains bound to the original shared actor.
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"other-create","threadId":"other","projectId":"p","title":"Sibling","modelSelection":{"instanceId":"first","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let sibling = send_to(&c, "other", "other-seed", "SIBLING-PRIVATE-WORD", "first");
    let other_native = projection_for(&c, "other")["providerThreads"][0]["nativeThreadRef"].clone();
    let next = send(&c, "continue", "Recall earlier private context", "second");
    let again = send_to(&c, "other", "other-next", "Recall sibling context", "first");
    assert_eq!(again["providerThreadId"], sibling["providerThreadId"]);
    let other = projection_for(&c, "other");
    assert_eq!(other["providerThreads"][0]["nativeThreadRef"], other_native);
    assert!(other["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["runId"] == again["id"]
            && m["role"] == "assistant"
            && m["text"]
                .as_str()
                .is_some_and(|t| t.contains("SIBLING-PRIVATE-WORD"))));
    let processes: Vec<Value> = std::fs::read_to_string(first.join("log/turn-processes"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(processes.len(), 3);
    assert!(
        processes
            .iter()
            .all(|turn| turn["pid"] == processes[0]["pid"]),
        "detaching one thread must preserve its sibling's native actor"
    );
    let p = projection(&c);
    assert_eq!(next["providerThreadId"], seed["providerThreadId"]);
    let pt = p["providerThreads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pt| pt["id"] == next["providerThreadId"])
        .unwrap();
    assert_eq!(pt["nativeThreadRef"], before);
    assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
    assert!(p["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["runId"] == next["id"]
            && m["role"] == "assistant"
            && m["text"]
                .as_str()
                .unwrap_or("")
                .contains("PRIVATE-PROFILE-WORD")));
    for (home, owner) in [
        (&shared, "shared"),
        (&first, "account-a"),
        (&second, "account-b"),
    ] {
        assert!(!std::fs::symlink_metadata(home.join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(home.join("auth.json")).unwrap())
                .unwrap()["fixtureOwner"],
            owner
        );
    }
}

#[test]
fn configured_relative_homes_are_absolute_before_native_cwd_and_exact_aliases_keep_their_config() {
    for (driver, id) in [("codex", "codex-relative"), ("claudeAgent", "claude")] {
        let s = support::start(|s| {
            *s.provider.lock().unwrap() =
                Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    s.dir.clone(),
                )));
        });
        let c = s.connect();
        let home = s.dir.join("relative-profile");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(
            home.join("auth.json"),
            json!({"fixtureOwner":"relative-profile"}).to_string(),
        )
        .unwrap();
        let mut relative = std::path::PathBuf::new();
        for _ in std::env::current_dir()
            .unwrap()
            .components()
            .filter(|p| matches!(p, std::path::Component::Normal(_)))
        {
            relative.push("..");
        }
        relative.push(home.strip_prefix("/").unwrap());
        let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/provider-home-peer.py");
        let ignored = s.dir.join("ignored-shadow");
        c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{id:{"driver":driver,"enabled":true,"config":{"binaryPath":peer,"homePath":relative,"shadowHomePath":if driver=="claudeAgent"{json!(ignored)}else{json!("")},"launchArgs":"--expected-owner relative-profile"},"environment":[{"name":if driver=="codex"{"CODEX_HOME"}else{"CLAUDE_CONFIG_DIR"},"value":"/ignored/by-explicit-config"}]}}}})).unwrap();
        let workspace = s.dir.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Relative","workspaceRoot":workspace})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Relative","modelSelection":{"instanceId":id,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        send(&c, "relative", "relative home proof", id);
        assert!(home.join("log/peer-starts").is_file());
        assert!(!ignored.exists());
    }
}

#[test]
fn private_auth_symlink_rejects_launch_and_recovers_after_an_explicit_file_fix() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            )));
    });
    let c = s.connect();
    let shared = s.dir.join("shared");
    let shadow = s.dir.join("shadow");
    for home in [&shared, &shadow] {
        std::fs::create_dir(home).unwrap();
    }
    std::fs::write(
        shared.join("auth.json"),
        json!({"fixtureOwner":"shared"}).to_string(),
    )
    .unwrap();
    std::os::unix::fs::symlink(shared.join("auth.json"), shadow.join("auth.json")).unwrap();
    let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/provider-home-peer.py");
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"work":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":shared,"shadowHomePath":shadow,"launchArgs":"--expected-owner private"}}}}})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Private auth","modelSelection":{"instanceId":"work","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"bad","messageId":"bad","threadId":"t","text":"must not launch","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(15), || projection(
        &c
    )["runs"][0]["status"]
        == "failed"));
    assert!(!shadow.join("log/peer-starts").exists());
    assert!(projection(&c)["runs"][0]["error"]
        .as_str()
        .unwrap()
        .contains("CodexShadowHomePrivateEntrySymlinkError"));
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let descriptor = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["instanceId"] == "work")
        .unwrap();
    assert_eq!(descriptor["status"], "error");
    assert!(descriptor["message"]
        .as_str()
        .unwrap()
        .contains("must be a real file"));
    std::fs::remove_file(shadow.join("auth.json")).unwrap();
    std::fs::write(
        shadow.join("auth.json"),
        json!({"fixtureOwner":"private"}).to_string(),
    )
    .unwrap();
    c.rpc("server.refreshProviders", json!({})).unwrap();
    send(&c, "fixed", "private profile recovered", "work");
    assert!(shadow.join("log/peer-starts").is_file());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(shared.join("auth.json")).unwrap()).unwrap()
            ["fixtureOwner"],
        "shared"
    );
}

#[test]
#[ignore = "three installed Codex gpt-6.1-sol turns; shared native history across two private account overlays"]
fn live_codex_account_overlays_resume_shared_history_with_private_auth() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let codex = Arc::new(o3_server::codex::CodexProvider::default());
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                Arc::new(o3_server::provider::Providers {
                    claude: None,
                    codex: codex.clone(),
                }),
                s.dir.clone(),
            )));
    });
    let c = s.connect();
    let workspace = s.dir.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Native overlays","workspaceRoot":workspace,"scripts":[]})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Native overlays","modelSelection":{"instanceId":"codex","model":"gpt-6.1-sol"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let first = format!(
        "FIRST{}",
        o3_server::store::random_id().unwrap().replace('-', "")
    );
    let second = format!(
        "SECOND{}",
        o3_server::store::random_id().unwrap().replace('-', "")
    );
    let turn = |id: &str, text: String, instance: &str| {
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"t","modelSelection":{"instanceId":instance,"model":"gpt-6.1-sol","options":[]},"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(90), || {
            let p = projection(&c);
            let run = p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["userMessageId"] == id)
                .unwrap();
            assert_ne!(
                run["status"], "failed",
                "turn={id} instance={instance}: {}",
                run["error"]
            );
            run["status"] == "completed"
        }));
        println!("Installed Codex turn {id} ({instance}) completed");
        let p = projection(&c);
        p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["userMessageId"] == id)
            .unwrap()
            .clone()
    };
    let seed = turn(
        "seed",
        format!("Remember first codeword {first}. Reply READY only. Do not use any tools."),
        "codex",
    );
    let shared = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
    let a = s.dir.join("account-a");
    let b = s.dir.join("account-b");
    for home in [&a, &b] {
        std::fs::create_dir(home).unwrap();
        std::fs::copy(shared.join("auth.json"), home.join("auth.json")).unwrap();
        std::fs::set_permissions(
            home.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{
  "account-a":{"driver":"codex","enabled":true,"config":{"homePath":shared,"shadowHomePath":a}},"account-b":{"driver":"codex","enabled":true,"config":{"homePath":shared,"shadowHomePath":b}}
 }}})).unwrap();
    let before = projection(&c)["providerThreads"][0]["nativeThreadRef"].clone();
    for (id,instance,text,word) in [("a","account-a",format!("Remember second codeword {second}. Reply only with the first codeword from earlier. Do not use any tools."),&first),("b","account-b","Reply only with the second codeword from earlier. Do not use any tools.".into(),&second)]{
  let run=turn(id,text,instance);let p=projection(&c);assert_eq!(run["providerThreadId"],seed["providerThreadId"]);assert_eq!(p["providerThreads"][0]["nativeThreadRef"],before);assert!(p["contextHandoffs"].as_array().unwrap().is_empty());assert!(p["messages"].as_array().unwrap().iter().any(|m|m["runId"]==run["id"]&&m["role"]=="assistant"&&m["text"].as_str().is_some_and(|text|text.contains(word))));
 }
    assert_eq!(
        std::fs::read_link(a.join("sessions")).unwrap(),
        shared.join("sessions")
    );
    assert_eq!(
        std::fs::read_link(b.join("sessions")).unwrap(),
        shared.join("sessions")
    );
    let auth_a = std::fs::symlink_metadata(a.join("auth.json")).unwrap();
    let auth_b = std::fs::symlink_metadata(b.join("auth.json")).unwrap();
    assert!(auth_a.is_file() && auth_b.is_file());
    assert_ne!((auth_a.dev(), auth_a.ino()), (auth_b.dev(), auth_b.ino()));
    assert!(projection(&c)["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| !matches!(
            item["type"].as_str(),
            Some("dynamic_tool" | "command_execution" | "file_change")
        )));
    println!("Installed Codex: direct home → account A → account B preserve one native conversation and private codewords; auth files remain separate");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            codex.shutdown().await;
        });
}
