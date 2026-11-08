//! Real Manager, Context, toolkit service, canonical NativePage/Gateway and
//! Chromium. The private launcher only proxies real browser pipe replies.
use super::super::toolkit_pages::{OwnedPageFactory, PageInstallation};
use super::super::WorkerSpec;
use super::*;
use std::{path::PathBuf, sync::atomic::AtomicUsize, time::Duration};

#[derive(Default)]
struct Retain(AtomicUsize);
impl crate::cleanup::Removal for Retain {
    fn execute(&self, _: &crate::cleanup::RemovalPlan) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "The private proof retains all browser profiles",
        ))
    }
}
struct ObservedFactory {
    actual: OwnedPageFactory,
    evaluate: AtomicUsize,
    metadata_refused: AtomicUsize,
    marks: AtomicUsize,
}
impl CallbackExecutor for Arc<ObservedFactory> {
    fn run(&self, callback: SourceCallback, deadline: FrozenDeadline) -> CallbackFuture {
        let observer = self.clone();
        let operation = callback.request().operation.clone();
        let metadata = callback.request().metadata_status;
        let future = self.actual.run(callback, deadline);
        Box::pin(async move {
            let output = future.await?;
            if operation == "evaluate" {
                assert!(matches!(
                    &output,
                    CallbackOutput::Value(OperationResult::Value(_))
                        | CallbackOutput::Value(OperationResult::JavaScriptValue(_))
                ));
                observer.evaluate.fetch_add(1, Ordering::SeqCst);
            } else {
                assert_eq!(operation, "status");
                assert!(metadata);
                assert!(
                    matches!(&output, CallbackOutput::Value(OperationResult::NativeError(error)) if error["tag"]=="PreviewAutomationExecutionError")
                );
                observer.metadata_refused.fetch_add(1, Ordering::SeqCst);
            }
            Ok(output)
        })
    }
    fn mark_used(&self, request: &CallbackRequest) -> Result<(), Fault> {
        self.actual.mark_used(request)?;
        self.marks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
        self.actual.close()
    }
}
fn home(root: &std::path::Path, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}
#[tokio::test]
#[ignore = "requires the private admitted nightly4314/Effect/Node fixture mounts"]
async fn actual_toolkit_factory_evaluates_real_page_and_accounts_optional_metadata_refusal() {
    crate::test_sandbox::assert_isolated();
    assert!(
        std::path::Path::new("/usr/bin/chromium").is_file(),
        "The private admitted real Chromium dependency is required"
    );
    let root = PathBuf::from("/out/owner-pipe");
    std::fs::create_dir(&root).unwrap();
    let launcher = root.join("chromium-fixture.py");
    std::fs::write(
        &launcher,
        include_bytes!("../../tests/fixtures/input/native-page-cdp-proxy.py"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let state_root = home(&root, "profile");
    std::fs::write(
        state_root.join("settings.json"),
        r#"{"enableAgentBrowserAccess":true}"#,
    )
    .unwrap();
    let state = Arc::new(State::open(&state_root).unwrap());
    *state
        .streams
        .p2
        .preview
        .browser
        .fixture_binary
        .lock()
        .unwrap() = Some(launcher);
    let retained = Arc::new(Retain::default());
    state
        .streams
        .p2
        .preview
        .set_profile_removal(retained.clone());
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/native-preview-caller-projection-4314.json"
    ))
    .unwrap();
    let input = &fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["status"] == "running" && case["background"] == false)
        .unwrap()["input"];
    {
        let db = state.db.lock().unwrap();
        crate::events::put(&db, "thread", "thread", "thread", &input["thread"]).unwrap();
        for (kind, value) in input.as_object().unwrap() {
            if let Some(records) = value.as_array() {
                for record in records {
                    crate::events::put(&db, kind, record["id"].as_str().unwrap(), "thread", record)
                        .unwrap();
                }
            }
        }
    }
    state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
    let context = state
        .native_mcp
        .obtain(&state, "thread", "codex", &state_root, &Value::Null)
        .await
        .unwrap();
    let caller = Caller::capture(&context).unwrap();
    let key = caller.agent_session_key().unwrap();
    assert!(caller.preview_issued().is_ok());
    let opened=crate::preview::call(&state,"preview.open",&json!({"threadId":"thread","runtime":"server","profileId":"default","automationOwner":key})).await.unwrap();
    let tab = opened["tabId"].as_str().unwrap().to_owned();
    let native = state
        .streams
        .p2
        .preview
        .capture_native_page("thread", &tab)
        .unwrap();
    let node = PathBuf::from(
        std::env::var_os("O3_ADMITTED_NODE").expect("Admitted Node26.13 mount required"),
    );
    let factory=OwnedPageFactory::new(&state,PageInstallation::new(super::super::WorkerSpec{
        node:node.clone(),entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/entry.mjs"),
        private_home:home(&root,"page-homes"),configuration:json!({"nativeRoot":"/native","playwrightRoot":"/playwright","effectRoot":"/deps/node_modules/effect"}),
        startup_deadline:Duration::from_secs(20),close_deadline:Duration::from_secs(10),
    }).unwrap());
    let observed = Arc::new(ObservedFactory {
        actual: factory,
        evaluate: AtomicUsize::new(0),
        metadata_refused: AtomicUsize::new(0),
        marks: AtomicUsize::new(0),
    });
    let service = Service::new(&state, Arc::new(observed.clone())).unwrap();
    let worker=Worker::spawn_toolkit(super::super::WorkerSpec{
        node,entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/toolkit-entry.mjs"),
        private_home:home(&root,"toolkit-home"),configuration:json!({"sourceRoot":"/toolkit-native","nativeRoot":"/native","effectRoot":"/deps","attachmentsDir":home(&root,"attachments"),"environmentId":state.environment_id,"supportedOperations":["status","evaluate"]}),
        startup_deadline:Duration::from_secs(20),close_deadline:Duration::from_secs(10),
    },service.clone()).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        service.invoke(
            &worker,
            &context,
            NativeOperation::Evaluate,
            raw(json!({"tabId":tab,"expression":"'native toolkit physical result'"})).unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let output = match result {
        OperationResult::Value(value) => value,
        OperationResult::JavaScriptValue(value) => serde_json::from_str(value.json()).unwrap(),
        other => panic!("Actual source evaluation failed: {other:?}"),
    };
    assert_eq!(output["isError"], false);
    assert_eq!(
        output["structuredContent"]["value"],
        "native toolkit physical result"
    );
    assert!(
        output.get("toolIcon").is_none(),
        "Refused optional status must not invent a website icon"
    );
    assert_eq!(observed.evaluate.load(Ordering::SeqCst), 1);
    assert_eq!(observed.metadata_refused.load(Ordering::SeqCst), 1);
    assert_eq!(observed.marks.load(Ordering::SeqCst), 2);
    assert!(service.ledger.lock().unwrap().clean());
    let again = state
        .streams
        .p2
        .preview
        .capture_native_page("thread", &tab)
        .unwrap();
    assert!(
        native.same_owner(&again),
        "Metadata must preserve the canonical physical Page"
    );
    let closed = worker.close().await;
    assert!(closed.worker_reaped && closed.owner_grant_closed && closed.graceful_worker);
    assert_eq!(closed.executed_recursive_removals, Some(0));
    // Closing toolkit resources must not close the actual UI-owned target or
    // its borrowed legacy CDP session. This check is not the source action path.
    let legacy = state
        .streams
        .p2
        .preview
        .evaluate_exact(
            "thread",
            &tab,
            br#"{"expression":"1","returnByValue":true}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert!(legacy.payload().is_some());
    crate::preview::shutdown(&state).await;
    assert_eq!(retained.0.load(Ordering::SeqCst), 0);
}

struct NoTabFactory {
    actual: OwnedPageFactory,
    statuses: AtomicUsize,
    marks: AtomicUsize,
}
impl CallbackExecutor for Arc<NoTabFactory> {
    fn run(&self, callback: SourceCallback, deadline: FrozenDeadline) -> CallbackFuture {
        let owner = self.clone();
        let future = self.actual.run(callback, deadline);
        Box::pin(async move {
            let output = future.await?;
            assert!(matches!(&output, CallbackOutput::StatusNoTab(_)));
            owner.statuses.fetch_add(1, Ordering::SeqCst);
            Ok(output)
        })
    }
    fn mark_used(&self, request: &CallbackRequest) -> Result<(), Fault> {
        self.actual.mark_used(request)?;
        self.marks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
        self.actual.close()
    }
}

#[tokio::test]
#[ignore = "requires the private admitted nightly4314/Effect/Node fixture mounts"]
async fn actual_toolkit_and_factory_format_absent_status_without_starting_page_worker() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "status-no-tab-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("settings.json"),
        br#"{"enableAgentBrowserAccess":true}"#,
    )
    .unwrap();
    let state = Arc::new(State::open(&root).unwrap());
    let fixture: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/native-preview-caller-projection-4314.json"
    ))
    .unwrap();
    let input = &fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["status"] == "running" && case["background"] == false)
        .unwrap()["input"];
    {
        let db = state.db.lock().unwrap();
        crate::events::put(&db, "thread", "thread", "thread", &input["thread"]).unwrap();
        for (kind, value) in input.as_object().unwrap() {
            if let Some(records) = value.as_array() {
                for record in records {
                    crate::events::put(&db, kind, record["id"].as_str().unwrap(), "thread", record)
                        .unwrap();
                }
            }
        }
    }
    // Synthetic desktop transport supplies the actual issued capability only;
    // callback execution uses the actual factory, without a Page or CDP target.
    let (desktop_input, _desktop_peer) = tokio::io::duplex(4096);
    let channel = crate::desktop_browser_channel::Channel::start(desktop_input, tokio::io::sink());
    let driver = crate::preview::install_desktop(state.clone(), channel).await;
    state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
    let context = state
        .native_mcp
        .obtain(&state, "thread", "codex", &root, &Value::Null)
        .await
        .unwrap();
    assert!(Caller::capture(&context).unwrap().preview_issued().is_ok());
    let node = PathBuf::from(
        std::env::var_os("O3_ADMITTED_NODE").expect("Admitted Node26.13 mount required"),
    );
    let page_homes = home(&root, "page-homes");
    let factory = OwnedPageFactory::new(&state, PageInstallation::new(WorkerSpec {
        node:node.clone(), entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/entry.mjs"),
        private_home:page_homes.clone(), configuration:json!({"nativeRoot":"/native","playwrightRoot":"/playwright","effectRoot":"/deps/node_modules/effect"}),
        startup_deadline:Duration::from_secs(20), close_deadline:Duration::from_secs(5),
    }).unwrap());
    let observed = Arc::new(NoTabFactory {
        actual: factory,
        statuses: AtomicUsize::new(0),
        marks: AtomicUsize::new(0),
    });
    let service = Service::new(&state, Arc::new(observed.clone())).unwrap();
    let worker = Worker::spawn_toolkit(WorkerSpec {
        node, entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/toolkit-entry.mjs"),
        private_home:home(&root,"toolkit-home"), configuration:json!({"sourceRoot":"/toolkit-native","nativeRoot":"/native","effectRoot":"/deps","attachmentsDir":home(&root,"attachments"),"environmentId":state.environment_id,"supportedOperations":["status"]}),
        startup_deadline:Duration::from_secs(20), close_deadline:Duration::from_secs(5),
    }, service.clone()).await.unwrap();
    let reporter = state.streams.p2.preview.profile_reporter().unwrap();
    for (index, request) in [json!({}), json!({"tabId":"missing-target"})]
        .into_iter()
        .enumerate()
    {
        if index == 1 {
            reporter
                .report(
                    crate::browser_profile_catalog::Report::decode(&json!({"profiles":[
                {"id":"work","name":"Trabajo 🚀","kind":"persistent"},
                {"id":"private","name":"Private","kind":"incognito"}
            ],"defaultProfileId":"work"}))
                    .unwrap(),
                )
                .unwrap();
        }
        let expected_profiles = state.streams.p2.preview.profile_catalog().status();
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            service.invoke(
                &worker,
                &context,
                NativeOperation::Status,
                raw(request).unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let output = match result {
            OperationResult::Value(value) => value,
            OperationResult::JavaScriptValue(value) => serde_json::from_str(value.json()).unwrap(),
            other => panic!("Actual source status failed: {other:?}"),
        };
        assert_eq!(output["isError"], false);
        let status = &output["structuredContent"];
        assert_eq!(status["available"], false);
        assert_eq!(status["visible"], false);
        assert_eq!(status["tabId"], Value::Null);
        assert_eq!(status["url"], Value::Null);
        assert_eq!(status["title"], Value::Null);
        assert_eq!(status["loading"], false);
        assert_eq!(status["profiles"], expected_profiles["profiles"]);
        assert_eq!(
            status.get("defaultProfileId"),
            expected_profiles.get("defaultProfileId")
        );
        assert!(service.ledger.lock().unwrap().clean());
    }
    assert_eq!(observed.statuses.load(Ordering::SeqCst), 2);
    assert_eq!(observed.marks.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read_dir(&page_homes).unwrap().count(), 0);
    let closed = worker.close().await;
    assert!(closed.worker_reaped && closed.owner_grant_closed && closed.graceful_worker);
    assert_eq!(closed.executed_recursive_removals, Some(0));
    driver.shutdown().await;
    crate::preview::shutdown(&state).await;
}

#[path = "status_negative_tests.rs"]
mod status_negative_tests;
