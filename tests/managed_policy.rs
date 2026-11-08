//! Strict machine policy intake, checked against the installed official nightly.
use managed_policy::{Family, Policy, RuntimeMode};
use o3_server::managed_policy;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn stored_task(db: &rusqlite::Connection) -> Value {
    let text: String = db
        .query_row(
            "SELECT body FROM records WHERE kind='scheduledTasks' AND id='hook'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/managed-policy-4262.json")).unwrap()
}
fn sandbox() -> PathBuf {
    assert!(
        Path::new("/run/.containerenv").is_file() || Path::new("/.dockerenv").is_file(),
        "Use the rootless container runner"
    );
    let path = std::env::temp_dir().join(format!(
        "o3-managed-policy-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn strict_policy_schema_matches_pinned_oracle_cases() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let result = Policy::parse(
            Path::new("/test/policy.json"),
            case["json"].as_str().unwrap(),
        );
        assert_eq!(
            result.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
    }
}
#[test]
fn production_paths_ignore_profile_and_environment_overrides_and_file_errors_stop_intake() {
    let root = sandbox();
    let file = root.join("policy.json");
    assert!(Policy::load_file(&file).unwrap().document.is_none());
    std::fs::write(&file, "{ malformed policy").unwrap();
    let error = Policy::load_file(&file).unwrap_err();
    assert_eq!(managed_policy::EXIT_CODE, 77);
    assert_eq!(error.path, file);
    assert!(error
        .to_string()
        .starts_with("T3 Code cannot use the managed policy at "));
    assert!(error
        .to_string()
        .ends_with("It will not start until an administrator fixes this."));
    assert!(Policy::load_file(&root)
        .unwrap_err()
        .reason
        .contains("cannot be read"));
    assert_eq!(
        managed_policy::machine_path("linux"),
        "/etc/t3code/policy.json"
    );
    assert_eq!(
        managed_policy::machine_path("darwin"),
        "/Library/Application Support/T3Code/policy.json"
    );
    assert_eq!(
        managed_policy::machine_path("win32"),
        "C:\\Program Files\\T3Code\\policy.json"
    );
    std::env::set_var("T3CODE_DEV_MANAGED_POLICY_PATH", &file);
    // A release never adopts this user-writable development override.
    assert!(Policy::load_system().unwrap().document.is_none());
    std::env::remove_var("T3CODE_DEV_MANAGED_POLICY_PATH");
}
#[test]
fn capabilities_are_optional_and_a_known_unenforced_family_refuses_startup() {
    let policy = Policy::parse(
        Path::new("/test/policy.json"),
        r#"{"schemaVersion":1,"organization":"Example Corp","limits":{"webhookTasks":false}}"#,
    )
    .unwrap();
    assert_eq!(
        policy.denial("webhookTasks").as_deref(),
        Some("Example Corp's policy does not allow webhook tasks.")
    );
    assert!(policy.denial("telemetry").is_none());
    assert!(!policy.allows_auth("unsafe-no-auth"));
    assert!(Policy::unmanaged("/test/missing.json").allows_auth("unsafe-no-auth"));
    policy
        .require_enforced(&[Family::Authentication, Family::Webhooks])
        .unwrap();
    assert!(policy
        .require_enforced(&[Family::Authentication])
        .unwrap_err()
        .reason
        .contains("Webhooks"));
    for (limit, family) in [
        (json!({"maxRuntimeMode":"auto"}), "Runtime"),
        (json!({"clients":["desktop"]}), "Clients"),
        (json!({"connect":false}), "Connect"),
        (json!({"telemetry":false}), "Telemetry"),
        (json!({"browserCookieImport":false}), "BrowserCookies"),
        (
            json!({"providers":{"allowedDrivers":["codex"]}}),
            "Providers",
        ),
        (json!({"updates":{"mode":"off"}}), "Updates"),
    ] {
        let value = json!({"schemaVersion":1,"organization":"Example Corp","limits":limit});
        let policy = Policy::parse(Path::new("/test/policy.json"), &value.to_string()).unwrap();
        let error = policy
            .require_enforced(&[Family::Authentication, Family::Webhooks])
            .unwrap_err();
        assert!(error.reason.contains(family), "{error}");
    }
    let settings=Policy::parse(Path::new("/test/policy.json"),r#"{"schemaVersion":1,"organization":"Example Corp","settings":{"defaultRuntimeMode":{"value":"approval-required","locked":true}}}"#).unwrap();
    assert!(settings
        .require_enforced(&[Family::Authentication, Family::Webhooks])
        .unwrap_err()
        .reason
        .contains("Settings"));
}
#[test]
fn limits_use_native_messages_and_summaries_hide_launch_and_update_values() {
    let policy=Policy::parse(Path::new("/test/policy.json"),r#"{"schemaVersion":1,"organization":"Example Corp","limits":{"maxRuntimeMode":"auto-accept-edits","clients":["desktop"],"updates":{"mode":"auto","feedUrl":"https://updates.example/feed?token=fixture-only","allowNightly":false},"providers":{"instances":{"codex":{"locked":true,"environment":[{"name":"TOKEN","value":"fixture-only-secret"}]}}}}}"#).unwrap();
    assert!(!policy.allows_remote());
    assert!(!policy.allows_connect());
    assert!(policy.allows_auth("desktop-managed-local"));
    assert!(!policy.allows_auth("loopback-browser"));
    assert_eq!(
        policy.runtime_denial(RuntimeMode::Full).as_deref(),
        Some("Example Corp limits threads to Auto-accept edits.")
    );
    assert!(policy.runtime_denial(RuntimeMode::Supervised).is_none());
    let summary = policy.basic_summary().unwrap();
    assert_eq!(summary["limits"]["connect"], false);
    assert_eq!(summary["limits"]["updates"]["allowNightly"], false);
    assert!(!summary.to_string().contains("fixture-only"));
    let case=Policy::parse(Path::new("/test/policy.json"),r#"{"schemaVersion":1,"organization":"Example Corp","limits":{"providers":{"instances":{"codex":{"locked":true,"environment":[{"name":"Path","value":"a"},{"name":"PATH","value":"b"}]}}}}}"#).unwrap();
    case.validate_providers("linux").unwrap();
    assert!(case
        .validate_providers("win32")
        .unwrap_err()
        .reason
        .contains("repeats a variable"));
}

#[test]
fn macos_preferences_unwrap_only_the_unambiguous_native_profile_shape() {
    let document =
        json!({"schemaVersion":1,"organization":"Example Corp","limits":{"webhookTasks":false}});
    for value in [
        document.clone(),
        json!({"schemaVersion":1,"organization":"Example Corp","limits":{"webhookTasks":false},"PayloadUUID":"synthetic-profile","_manualProfile":true}),
        json!({"Forced":[{"mcx_preference_settings":document}]}),
    ] {
        let policy = Policy::parse_macos_preferences(
            Path::new(managed_policy::MACOS_PROFILE),
            &value.to_string(),
        )
        .unwrap();
        assert!(policy.denial("webhookTasks").is_some());
    }
    for value in [
        json!({"Forced":[{"mcx_preference_settings":document}],"limits":{"telemetry":false}}),
        json!({"Forced":[{"mcx_preference_settings":document},{"mcx_preference_settings":document}]}),
        json!({"Forced":[{"mcx_preference_settings":document,"futureRestriction":true}]}),
    ] {
        assert!(Policy::parse_macos_preferences(
            Path::new(managed_policy::MACOS_PROFILE),
            &value.to_string()
        )
        .is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immutable_machine_policy_blocks_http_task_enable_and_both_pending_workers() {
    use std::sync::Arc;
    let root = sandbox();
    let policy_file = root.join("policy.json");
    std::fs::write(
        &policy_file,
        r#"{"schemaVersion":1,"organization":"Example Corp","limits":{"webhookTasks":false}}"#,
    )
    .unwrap();
    let policy = Policy::load_file(&policy_file).unwrap();
    let state =
        Arc::new(o3_server::store::State::open_with_policy(&root.join("profile"), policy).unwrap());
    *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
    o3_server::events::mutate_project(&state,&json!({"type":"project.create","commandId":"project","projectId":"p","title":"Policy fixture","workspaceRoot":root})).unwrap();
    let input = json!({"id":"hook","title":"Hook","prompt":"Synthetic policy work","enabled":false,"schedule":{"type":"webhook"},"projectId":"p","workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"});
    let task = o3_server::scheduled::call(&state, "scheduledTasks.upsert", &input)
        .unwrap()
        .unwrap()["task"]
        .clone();
    let reason = "Example Corp's policy does not allow webhook tasks.";
    assert_eq!(task["blockedByPolicy"], reason);
    let error = o3_server::scheduled::call(
        &state,
        "scheduledTasks.setEnabled",
        &json!({"id":"hook","enabled":true}),
    )
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains(reason));
    let mut enabled = input.clone();
    enabled["enabled"] = json!(true);
    assert!(
        o3_server::scheduled::call(&state, "scheduledTasks.upsert", &enabled)
            .unwrap()
            .is_err()
    );
    let config = o3_server::settings::config(&state).unwrap();
    assert_eq!(config["managedPolicy"]["limits"]["webhookTasks"], false);
    // The policy is a startup snapshot. Editing the file cannot unblock a running server.
    std::fs::write(
        &policy_file,
        r#"{"schemaVersion":1,"organization":"Example Corp","limits":{"webhookTasks":true}}"#,
    )
    .unwrap();
    {
        let db = state.db.lock().unwrap();
        let mut existing = stored_task(&db);
        assert!(existing.get("blockedByPolicy").is_none());
        existing["enabled"] = json!(true);
        db.execute(
            "UPDATE records SET body=?1 WHERE kind='scheduledTasks' AND id='hook'",
            [existing.to_string()],
        )
        .unwrap();
        db.execute("INSERT INTO scheduled_task_webhook_pending VALUES('policy-pending','hook','',0,'pending',?1,?2,'[]')",rusqlite::params![existing.to_string(),json!({"method":"POST","path":"/api/hooks/hook","query":"","headers":{},"bodyText":"{}"}).to_string()]).unwrap();
        db.execute("INSERT INTO scheduled_task_webhook_replay_keys VALUES('hook','synthetic-replay','policy-pending',9223372036854775807)",[]).unwrap();
        db.execute("INSERT INTO hook_queue VALUES('legacy-pending','hook','',0,9223372036854775807,'Synthetic legacy policy work')",[]).unwrap();
    }
    // Direct fixture writes bypass the public admission wake-up; notify its
    // already started timer just as native-store recovery does after import.
    state.streams.ops.wake_scheduler();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(o3_server::serve(listener, state.clone()));
    let client = reqwest::Client::new();
    for path in [
        "/api/hooks/missing/invalid",
        task["webhook"]["path"].as_str().unwrap(),
    ] {
        let response = client
            .post(format!("http://{address}{path}"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.headers()["x-t3-hook-outcome"], "blocked_by_policy");
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let pending: i64 = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM scheduled_task_webhook_pending",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if pending == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Policy-blocked native delivery was not rejected before dispatch");
    let db = state.db.lock().unwrap();
    let task = stored_task(&db);
    assert_eq!(task["runCount"], 0);
    assert!(task["lastRunAt"].is_null());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scheduled_task_webhook_deliveries",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM hook_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scheduled_task_webhook_replay_keys",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    server.abort();
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference --test managed_policy in rootless Podman"]
fn official_policy_check_confirms_every_oracle_fixture_without_inference() {
    let root = sandbox();
    assert_eq!(std::env::var("O3_HEADLESS_CONTAINER").as_deref(), Ok("1"));
    let binary = std::env::var("T3_REFERENCE_BIN").unwrap();
    let version = std::process::Command::new(&binary)
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("20261106.4262"));
    let fixture = fixture();
    let from = std::env::var("O3_POLICY_ORACLE_FROM")
        .ok()
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(from < fixture["cases"].as_array().unwrap().len());
    println!("Official managed policy oracle begins at case {from}");
    for (index, case) in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .skip(from)
    {
        let path = root.join(format!("policy-{index}.json"));
        std::fs::write(&path, case["json"].as_str().unwrap()).unwrap();
        let result = std::process::Command::new(&binary)
            .env("T3CODE_HOME", root.join("profile"))
            .args(["policy", "check", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            case["accepted"].as_bool().unwrap(),
            "{}: status={}; stdout={}; stderr={}",
            case["name"],
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        if index % 16 == 15 {
            println!("Official managed policy cases verified: {}", index + 1);
        }
    }
}
