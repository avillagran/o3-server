//! Actual framed Node entry and whole original toolkit, with a deliberately
//! synthetic callback executor. No Page, CDP owner, browser or model is launched.
use super::*;
use crate::preview_control::runtime::Control;
use std::{path::PathBuf, sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::Notify;

struct SyntheticExecutor {
    control: Control,
    entered: Notify,
    release: Notify,
    evaluate: AtomicUsize,
    metadata: AtomicUsize,
    marked: AtomicUsize,
}
impl CallbackExecutor for Arc<SyntheticExecutor> {
    fn run(&self, callback: SourceCallback, _deadline: FrozenDeadline) -> CallbackFuture {
        let executor = self.clone();
        Box::pin(async move {
            let request = callback.request().clone();
            match request.operation.as_str() {
                "evaluate" => {
                    executor.evaluate.fetch_add(1, Ordering::SeqCst);
                    let entered = executor.clone();
                    executor
                        .control
                        .source_observe_admitted(callback.entry_gate(), move |_| async move {
                            callback.validate()?;
                            entered.entered.notify_one();
                            entered.release.notified().await;
                            callback.validate()?;
                            Ok(Value::Null)
                        })
                        .wait()
                        .await
                        .map_err(|_| Fault::Revoked)?;
                    Ok(CallbackOutput::Value(OperationResult::Value(json!(
                        "late synthetic evaluation"
                    ))))
                }
                "status" => {
                    executor.metadata.fetch_add(1, Ordering::SeqCst);
                    Ok(CallbackOutput::Value(OperationResult::Value(
                        json!({"available":true,"visible":false,"tabId":null,"url":null,"title":null,"loading":false}),
                    )))
                }
                _ => Err(Fault::Protocol),
            }
        })
    }
    fn mark_used(&self, request: &CallbackRequest) -> Result<(), Fault> {
        assert_eq!(request.operation, "evaluate");
        self.marked.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
        Box::pin(async move {
            self.control
                .close()
                .wait()
                .await
                .map(|_| ())
                .map_err(|_| Fault::CleanupUnknown)
        })
    }
}

#[tokio::test]
#[ignore = "requires the private admitted nightly4314/Effect/Node fixture mounts"]
async fn actual_entry_observer_drop_ends_source_before_late_callback_without_metadata_producer() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "toolkit-owner-cancel-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("settings.json"),
        r#"{"enableAgentBrowserAccess":true}"#,
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
    // Explicit synthetic desktop slot makes the real capability predicate
    // available without an executable or Page. This is not a CDP proof.
    let (_event_peer, event_input) = tokio::io::duplex(4096);
    let (control_output, _control_peer) = tokio::io::duplex(4096);
    let channel = crate::desktop_browser_channel::Channel::start(event_input, control_output);
    let driver = crate::preview::install_desktop(state.clone(), channel).await;
    state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
    let context = state
        .native_mcp
        .obtain(&state, "thread", "codex", &root, &Value::Null)
        .await
        .unwrap();
    assert!(Caller::capture(&context).unwrap().preview_current().is_ok());
    let executor = Arc::new(SyntheticExecutor {
        control: Control::new(None),
        entered: Notify::new(),
        release: Notify::new(),
        evaluate: AtomicUsize::new(0),
        metadata: AtomicUsize::new(0),
        marked: AtomicUsize::new(0),
    });
    let owner = Service::new(&state, Arc::new(executor.clone())).unwrap();
    let home = root.join("node-home");
    std::fs::create_dir(&home).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let attachments = root.join("attachments");
    std::fs::create_dir(&attachments).unwrap();
    let spec = super::super::WorkerSpec {
        node: PathBuf::from(
            std::env::var_os("O3_ADMITTED_NODE").expect("private admitted Node mount required"),
        ),
        entry: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/toolkit-entry.mjs"),
        private_home: home,
        configuration: json!({"sourceRoot":"/toolkit-native","nativeRoot":"/native","effectRoot":"/deps","attachmentsDir":attachments,"environmentId":state.environment_id,"supportedOperations":["status","evaluate"]}),
        startup_deadline: Duration::from_secs(20),
        close_deadline: Duration::from_secs(5),
    };
    let worker = Worker::spawn_toolkit(spec, owner.clone()).await.unwrap();
    let mut invocation = owner.invoke(
        &worker,
        &context,
        NativeOperation::Evaluate,
        raw(json!({"expression":"held synthetic callback"})).unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            result = &mut invocation => panic!("invocation ended before synthetic callback: {}", result.is_ok()),
            _ = executor.entered.notified() => {},
        }
    }).await.unwrap();
    // Drop the actual owned invocation future, not just its JoinHandle.
    drop(invocation);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ended = {
                let ledger = owner.ledger.lock().unwrap();
                ledger.invocations.len() == 1
                    && ledger
                        .invocations
                        .values()
                        .all(|invocation| invocation.source_ended && invocation.observer_returned)
                    && ledger.callbacks.len() == 1
                    && ledger
                        .callbacks
                        .values()
                        .all(|callback| callback.phase == CallbackPhase::Running)
            };
            if ended {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(executor.evaluate.load(Ordering::SeqCst), 1);
    assert_eq!(executor.metadata.load(Ordering::SeqCst), 0);
    assert_eq!(executor.marked.load(Ordering::SeqCst), 0);
    assert!(!owner.ledger.lock().unwrap().clean());
    executor.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let clean = owner.ledger.lock().unwrap().clean();
            if clean {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(executor.evaluate.load(Ordering::SeqCst), 1);
    assert_eq!(executor.metadata.load(Ordering::SeqCst), 0);
    assert_eq!(executor.marked.load(Ordering::SeqCst), 1);
    let receipt = worker.close().await;
    assert!(receipt.worker_reaped);
    assert!(receipt.graceful_worker);
    assert!(receipt.owner_grant_closed);
    assert_eq!(receipt.executed_recursive_removals, Some(0));
    driver.shutdown().await;
    // All fixture data remains owned and retained for container teardown.
}
