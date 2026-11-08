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

struct ReadonlyRaceExecutor {
    control: Control,
    entered: Notify,
    release: Notify,
    callbacks: AtomicUsize,
    executed_writes: AtomicUsize,
    refused_writes: AtomicUsize,
}
impl CallbackExecutor for Arc<ReadonlyRaceExecutor> {
    fn run(&self, callback: SourceCallback, _deadline: FrozenDeadline) -> CallbackFuture {
        let executor = self.clone();
        Box::pin(async move {
            executor.callbacks.fetch_add(1, Ordering::SeqCst);
            if callback.request().class == NativeOperation::Status {
                callback.validate().map_err(|_| Fault::Revoked)?;
                return Ok(CallbackOutput::Value(OperationResult::Value(json!({
                    "available":true,"visible":false,"tabId":null,"url":null,
                    "title":null,"loading":false
                }))));
            }
            assert_eq!(callback.request().class, NativeOperation::Evaluate);
            let observed = executor.clone();
            let result = executor
                .control
                .source_observe_admitted(callback.entry_gate(), move |_| async move {
                    callback.validate()?;
                    observed.entered.notify_one();
                    observed.release.notified().await;
                    if let Err(error) = callback.validate() {
                        observed.refused_writes.fetch_add(1, Ordering::SeqCst);
                        return Err(error);
                    }
                    observed.executed_writes.fetch_add(1, Ordering::SeqCst);
                    Ok(json!("synthetic callback was executed"))
                })
                .wait()
                .await;
            Ok(CallbackOutput::Value(match result {
                Ok(value) => OperationResult::Value(value),
                Err(error) => OperationResult::NativeError(json!({
                    "tag":"PreviewAutomationExecutionError",
                    "message":error["message"].as_str().unwrap_or("The native caller changed.")
                })),
            }))
        })
    }
    fn mark_used(&self, _: &CallbackRequest) -> Result<(), Fault> {
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
fn toolkit_value(result: OperationResult) -> Value {
    match result {
        OperationResult::Value(value) => value,
        OperationResult::JavaScriptValue(value) => serde_json::from_str(value.json()).unwrap(),
        other => panic!("The whole original toolkit must produce this result: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires the private admitted nightly4314/Effect/Node fixture mounts"]
async fn actual_context_foreign_reads_reach_original_toolkit_and_late_writes_remain_fenced() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "toolkit-issued-read-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let settings = root.join("settings.json");
    std::fs::write(&settings, br#"{"enableAgentBrowserAccess":true}"#).unwrap();
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
    // Explicit synthetic desktop transport makes the issued capability real;
    // the callback executor below is synthetic and never creates a Page/CDP.
    let (desktop_input, _desktop_peer) = tokio::io::duplex(4096);
    let channel = crate::desktop_browser_channel::Channel::start(desktop_input, tokio::io::sink());
    let driver = crate::preview::install_desktop(state.clone(), channel).await;
    state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
    let context = state
        .native_mcp
        .obtain(&state, "thread", "codex", &root, &Value::Null)
        .await
        .unwrap();
    let caller = Caller::capture(&context).unwrap();
    let issued = caller.scope().unwrap();
    let executor = Arc::new(ReadonlyRaceExecutor {
        control: Control::new(None),
        entered: Notify::new(),
        release: Notify::new(),
        callbacks: AtomicUsize::new(0),
        executed_writes: AtomicUsize::new(0),
        refused_writes: AtomicUsize::new(0),
    });
    let service = Service::new(&state, Arc::new(executor.clone())).unwrap();
    let home = root.join("node-home");
    std::fs::create_dir(&home).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let attachments = root.join("attachments");
    std::fs::create_dir(&attachments).unwrap();
    let worker = Worker::spawn_toolkit(super::super::WorkerSpec {
        node: PathBuf::from(std::env::var_os("O3_ADMITTED_NODE").expect("private admitted Node mount required")),
        entry: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/toolkit-entry.mjs"),
        private_home: home,
        configuration: json!({"sourceRoot":"/toolkit-native","nativeRoot":"/native","effectRoot":"/deps","attachmentsDir":attachments,"environmentId":state.environment_id,"supportedOperations":["status","evaluate"]}),
        startup_deadline: Duration::from_secs(20), close_deadline: Duration::from_secs(5),
    }, service.clone()).await.unwrap();
    let mut foreign = input["thread"].clone();
    foreign["providerInstanceId"] = json!("foreign-provider");
    crate::events::put(
        &state.db.lock().unwrap(),
        "thread",
        "thread",
        "thread",
        &foreign,
    )
    .unwrap();
    assert_eq!(caller.scope().unwrap(), issued);
    assert_eq!(Caller::capture(&context).unwrap().scope().unwrap(), issued);
    assert_eq!(
        caller.thread_projection("thread").unwrap()["thread"],
        foreign
    );
    let read = toolkit_value(
        service
            .invoke(
                &worker,
                &context,
                NativeOperation::Status,
                raw(json!({})).unwrap(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(read["isError"], false);
    let callbacks = executor.callbacks.load(Ordering::SeqCst);
    assert_eq!(callbacks, 1);
    let write = toolkit_value(
        service
            .invoke(
                &worker,
                &context,
                NativeOperation::Evaluate,
                raw(json!({"expression":"denied foreign write"})).unwrap(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(write["isError"], true);
    // Effect's original declaredFailureResult formats Error.message as plain
    // text, rather than returning the OrchestratorMcpFailure.code field.
    assert_eq!(
        write["content"],
        json!([{"type":"text","text":"The calling provider no longer owns an active thread run."}]),
        "The original ownership failure must retain its native formatting: {write}"
    );
    assert_eq!(
        executor.callbacks.load(Ordering::SeqCst),
        callbacks,
        "The original prelude rejects before any host callback"
    );
    assert!(service.ledger.lock().unwrap().clean());
    crate::events::put(
        &state.db.lock().unwrap(),
        "thread",
        "thread",
        "thread",
        &input["thread"],
    )
    .unwrap();
    let mut late = service.invoke(
        &worker,
        &context,
        NativeOperation::Evaluate,
        raw(json!({"expression":"held synthetic write"})).unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            result = &mut late => panic!("Write ended before its owned callback entry: {result:?}"),
            _ = executor.entered.notified() => {},
        }
    })
    .await
    .unwrap();
    let mut successor = input["thread"].clone();
    successor["providerGeneration"] = json!("successor-generation");
    crate::events::put(
        &state.db.lock().unwrap(),
        "thread",
        "thread",
        "thread",
        &successor,
    )
    .unwrap();
    assert_eq!(caller.scope().unwrap(), issued);
    executor.release.notify_one();
    let late = toolkit_value(
        tokio::time::timeout(Duration::from_secs(15), late)
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(late["isError"], true);
    assert_eq!(executor.refused_writes.load(Ordering::SeqCst), 1);
    assert_eq!(executor.executed_writes.load(Ordering::SeqCst), 0);
    assert!(service.ledger.lock().unwrap().clean());
    std::fs::write(&settings, br#"{"enableAgentBrowserAccess":false}"#).unwrap();
    assert_eq!(caller.scope().unwrap(), issued);
    let withheld = state
        .native_mcp
        .obtain(&state, "thread", "codex", &root, &Value::Null)
        .await
        .unwrap();
    let callbacks = executor.callbacks.load(Ordering::SeqCst);
    let withheld_result = toolkit_value(
        service
            .invoke(
                &worker,
                &withheld,
                NativeOperation::Status,
                raw(json!({})).unwrap(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(withheld_result["isError"], true);
    assert_eq!(
        withheld_result["content"],
        json!([{"type":"text","text":"MCP credential does not grant the preview capability: browser preview tools are off for this thread. Do not retry them. To check a page, use a headless browser from the shell, such as Playwright, or curl. The user can turn on \"Agent browser access\" in Settings; it applies when the agent session next starts."}]),
        "The original missing-preview failure must retain its native message: {withheld_result}"
    );
    assert_eq!(executor.callbacks.load(Ordering::SeqCst), callbacks);
    context.revoke();
    assert!(matches!(
        service
            .invoke(
                &worker,
                &context,
                NativeOperation::Status,
                raw(json!({})).unwrap()
            )
            .await,
        Err(Fault::Revoked)
    ));
    std::fs::write(&settings, br#"{"enableAgentBrowserAccess":true}"#).unwrap();
    let replacement = context.refresh(&root, &Value::Null).await.unwrap();
    assert_ne!(replacement.id(), context.id());
    let replacement_scope = Caller::capture(&replacement).unwrap().scope().unwrap();
    assert_eq!(
        replacement_scope["thread"]["providerSessionId"],
        replacement.id()
    );
    assert!(caller.scope().is_err());
    let closed = worker.close().await;
    assert!(closed.worker_reaped && closed.owner_grant_closed && closed.graceful_worker);
    assert_eq!(closed.executed_recursive_removals, Some(0));
    driver.shutdown().await;
    // All fixture state remains in private container memory for teardown.
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
    assert!(Caller::capture(&context).unwrap().preview_issued().is_ok());
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
