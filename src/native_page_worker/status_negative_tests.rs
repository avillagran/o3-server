//! Actual State, caller, toolkit and factory negatives. Only the desktop
//! capability transport is synthetic; no callback result or CDP reply is.
use super::*;
use tokio::sync::Notify;

struct NegativeFactory {
    actual: OwnedPageFactory,
    pause: AtomicBool,
    entered: Notify,
    release: Notify,
    absent: AtomicUsize,
    refused: AtomicUsize,
    marks: AtomicUsize,
}
impl CallbackExecutor for Arc<NegativeFactory> {
    fn run(&self, callback: SourceCallback, deadline: FrozenDeadline) -> CallbackFuture {
        let owner = self.clone();
        Box::pin(async move {
            assert_eq!(callback.request().class, NativeOperation::Status);
            if owner.pause.swap(false, Ordering::AcqRel) {
                owner.entered.notify_one();
                owner.release.notified().await;
            }
            let output = owner.actual.run(callback, deadline).await?;
            match &output {
                CallbackOutput::StatusNoTab(_) => {
                    owner.absent.fetch_add(1, Ordering::SeqCst);
                }
                CallbackOutput::Value(OperationResult::NativeError(error)) => {
                    assert_eq!(error["tag"], "PreviewAutomationExecutionError");
                    owner.refused.fetch_add(1, Ordering::SeqCst);
                }
                _ => panic!("Unexpected actual Status factory result"),
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
struct Fixture {
    root: PathBuf,
    state: Arc<State>,
    context: Arc<crate::native_mcp::Context>,
    service: Arc<Service>,
    worker: Worker,
    factory: Arc<NegativeFactory>,
    page_homes: PathBuf,
    driver: crate::preview::DesktopDriver,
    _peer: tokio::io::DuplexStream,
    retained: Arc<Retain>,
}
impl Fixture {
    async fn new(name: &str) -> Self {
        crate::test_sandbox::assert_isolated();
        let root =
            std::env::temp_dir().join(format!("{name}-{}", crate::store::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            br#"{"enableAgentBrowserAccess":true}"#,
        )
        .unwrap();
        let state = Arc::new(State::open(&root).unwrap());
        let records: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/native-preview-caller-projection-4314.json"
        ))
        .unwrap();
        let input = &records["cases"]
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
                        crate::events::put(
                            &db,
                            kind,
                            record["id"].as_str().unwrap(),
                            "thread",
                            record,
                        )
                        .unwrap();
                    }
                }
            }
        }
        let (desktop_input, peer) = tokio::io::duplex(4096);
        let channel =
            crate::desktop_browser_channel::Channel::start(desktop_input, tokio::io::sink());
        let driver = crate::preview::install_desktop(state.clone(), channel).await;
        state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
        let context = state
            .native_mcp
            .obtain(&state, "thread", "codex", &root, &Value::Null)
            .await
            .unwrap();
        assert!(Caller::capture(&context).unwrap().preview_issued().is_ok());
        let retained = Arc::new(Retain::default());
        state
            .streams
            .p2
            .preview
            .set_profile_removal(retained.clone());
        let launcher = root.join("chromium-gate.py");
        std::fs::write(
            &launcher,
            include_bytes!("../../tests/fixtures/input/native-status-cdp-gate.py"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        *state
            .streams
            .p2
            .preview
            .browser
            .fixture_binary
            .lock()
            .unwrap() = Some(launcher);
        let node = PathBuf::from(
            std::env::var_os("O3_ADMITTED_NODE").expect("Admitted Node mount required"),
        );
        let page_homes = home(&root, "page-homes");
        let actual=OwnedPageFactory::new(&state,PageInstallation::new(WorkerSpec {
            node:node.clone(),entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/entry.mjs"),private_home:page_homes.clone(),configuration:json!({"nativeRoot":"/native","playwrightRoot":"/playwright","effectRoot":"/deps/node_modules/effect"}),startup_deadline:Duration::from_secs(20),close_deadline:Duration::from_secs(5),
        }).unwrap());
        let factory = Arc::new(NegativeFactory {
            actual,
            pause: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
            absent: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            marks: AtomicUsize::new(0),
        });
        let service = Service::new(&state, Arc::new(factory.clone())).unwrap();
        let worker=Worker::spawn_toolkit(WorkerSpec {
            node,entry:PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/preview/toolkit-entry.mjs"),private_home:home(&root,"toolkit-home"),configuration:json!({"sourceRoot":"/toolkit-native","nativeRoot":"/native","effectRoot":"/deps","attachmentsDir":home(&root,"attachments"),"environmentId":state.environment_id,"supportedOperations":["status"]}),startup_deadline:Duration::from_secs(20),close_deadline:Duration::from_secs(5),
        },service.clone()).await.unwrap();
        Self {
            root,
            state,
            context,
            service,
            worker,
            factory,
            page_homes,
            driver,
            _peer: peer,
            retained,
        }
    }
    fn status(&self, input: Value) -> InvocationFuture {
        self.service.invoke(
            &self.worker,
            &self.context,
            NativeOperation::Status,
            raw(input).unwrap(),
        )
    }
    async fn open(&self) -> Value {
        let agent = Caller::capture(&self.context)
            .unwrap()
            .agent_session_key()
            .unwrap();
        crate::preview::call(&self.state,"preview.open",&json!({"threadId":"thread","runtime":"server","profileId":"default","automationOwner":agent})).await.unwrap()
    }
    async fn clean(&self) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if self.service.ledger.lock().unwrap().clean() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_dir(&self.page_homes).unwrap().count(), 0);
    }
    async fn finish(self) {
        self.clean().await;
        let closed = self.worker.close().await;
        assert!(closed.worker_reaped && closed.owner_grant_closed && closed.graceful_worker);
        assert_eq!(closed.executed_recursive_removals, Some(0));
        self.driver.shutdown().await;
        crate::preview::shutdown(&self.state).await;
        assert_eq!(self.retained.0.load(Ordering::SeqCst), 0);
    }
}
fn value(result: OperationResult) -> Value {
    match result {
        OperationResult::Value(value) => value,
        OperationResult::JavaScriptValue(value) => serde_json::from_str(value.json()).unwrap(),
        other => panic!("Unexpected actual toolkit result: {other:?}"),
    }
}
async fn refused(fixture: &Fixture, input: Value) {
    let result = value(
        tokio::time::timeout(Duration::from_secs(20), fixture.status(input))
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(result["isError"], true);
    assert!(
        result
            .get("structuredContent")
            .is_none_or(|value| value.get("available").is_none()),
        "Present targets must never become a no-tab status"
    );
    fixture.clean().await;
}

#[tokio::test]
#[ignore = "requires admitted actual Chromium, nightly4314, Effect and Node mounts"]
async fn actual_present_and_reserved_closing_targets_refuse_without_no_tab_or_worker() {
    let fixture = Fixture::new("status-present-closing").await;
    let opened = fixture.open().await;
    let tab = opened["tabId"].as_str().unwrap();
    assert!(fixture
        .state
        .streams
        .p2
        .preview
        .browser
        .page("thread", tab)
        .is_ok());
    refused(&fixture, json!({"tabId":tab})).await;
    refused(&fixture, json!({})).await;
    fixture
        .state
        .streams
        .p2
        .preview
        .browser
        .reserve_close("thread", Some(tab));
    assert!(fixture
        .state
        .streams
        .p2
        .preview
        .browser
        .page("thread", tab)
        .is_err());
    assert!(fixture
        .state
        .streams
        .p2
        .preview
        .browser
        .latest_tab("thread", None)
        .is_none());
    refused(&fixture, json!({"tabId":tab})).await;
    refused(&fixture, json!({})).await;
    assert_eq!(fixture.factory.absent.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.factory.refused.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.factory.marks.load(Ordering::SeqCst), 4);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires admitted actual Chromium, nightly4314, Effect and Node mounts"]
async fn actual_pending_setup_with_held_native_attach_ack_refuses_then_same_setup_completes() {
    let fixture = Fixture::new("status-unready").await;
    std::fs::write(fixture.root.join("hold-next-attach"), b"hold").unwrap();
    {
        let open = fixture.open();
        tokio::pin!(open);
        let marker = fixture.root.join("actual-attach-held.json");
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    result=&mut open=>panic!("Actual setup completed before held ACK: {result}"),
                    _=tokio::time::sleep(Duration::from_millis(10))=>{},
                }
                if marker.is_file() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(fixture
            .state
            .streams
            .p2
            .preview
            .browser
            .latest_tab("thread", None)
            .is_none());
        refused(&fixture, json!({})).await;
        assert_eq!(fixture.factory.absent.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.factory.refused.load(Ordering::SeqCst), 1);
        std::fs::write(fixture.root.join("release-attach"), b"release").unwrap();
        let opened = tokio::time::timeout(Duration::from_secs(20), open)
            .await
            .unwrap();
        assert!(fixture
            .state
            .streams
            .p2
            .preview
            .browser
            .page("thread", opened["tabId"].as_str().unwrap())
            .is_ok());
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires admitted nightly4314, Effect and Node mounts"]
async fn actual_context_revocation_refuses_status_while_factory_handoff_is_paused() {
    let fixture = Fixture::new("status-revoked").await;
    // A provider ownership transition alone does not revoke this read-only
    // credential. Exercise the old caller after both transition and refresh.
    let caller = Caller::capture(&fixture.context).unwrap();
    let issued = caller.scope().unwrap();
    let mut thread = caller.thread_projection("thread").unwrap()["thread"].clone();
    thread["providerGeneration"] = json!("status-readonly-successor");
    crate::events::put(
        &fixture.state.db.lock().unwrap(),
        "thread",
        "thread",
        "thread",
        &thread,
    )
    .unwrap();
    let refreshed = fixture
        .context
        .refresh(&fixture.root, &Value::Null)
        .await
        .unwrap();
    assert_ne!(refreshed.id(), fixture.context.id());
    // Refresh creates a successor; it does not revoke this old read-only caller.
    assert_eq!(caller.scope().unwrap(), issued);
    let read = value(fixture.status(json!({})).await.unwrap());
    assert_eq!(read["isError"], false);
    assert_eq!(read["structuredContent"]["available"], false);
    fixture.clean().await;
    fixture.factory.pause.store(true, Ordering::Release);
    let status = fixture.status(json!({}));
    tokio::pin!(status);
    tokio::time::timeout(Duration::from_secs(15),async {
        tokio::select! { result=&mut status=>panic!("Status completed before pause: {}",result.is_ok()),_=fixture.factory.entered.notified()=>{} }
    }).await.unwrap();
    fixture.context.revoke();
    assert!(caller.scope().is_err());
    fixture.factory.release.notify_one();
    let output = value(
        tokio::time::timeout(Duration::from_secs(15), status)
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(output["isError"], true);
    fixture.clean().await;
    assert_eq!(fixture.factory.absent.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.factory.refused.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.factory.marks.load(Ordering::SeqCst), 2);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires admitted nightly4314, Effect and Node mounts"]
async fn actual_status_observer_drop_retains_callback_and_finally_until_actual_factory_finishes() {
    let fixture = Fixture::new("status-source-cancel").await;
    fixture.factory.pause.store(true, Ordering::Release);
    let mut status = fixture.status(json!({}));
    tokio::time::timeout(Duration::from_secs(15),async {
        tokio::select! { result=&mut status=>panic!("Status completed before pause: {}",result.is_ok()),_=fixture.factory.entered.notified()=>{} }
    }).await.unwrap();
    drop(status);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let ended = {
                let ledger = fixture.service.ledger.lock().unwrap();
                ledger.invocations.len() == 1
                    && ledger
                        .invocations
                        .values()
                        .all(|row| row.source_ended && row.observer_returned)
                    && ledger.callbacks.len() == 1
                    && ledger
                        .callbacks
                        .values()
                        .all(|row| row.phase == CallbackPhase::Running)
            };
            if ended {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!fixture.service.ledger.lock().unwrap().clean());
    assert_eq!(fixture.factory.absent.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.factory.marks.load(Ordering::SeqCst), 0);
    fixture.factory.release.notify_one();
    fixture.clean().await;
    assert_eq!(fixture.factory.absent.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.factory.refused.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.factory.marks.load(Ordering::SeqCst), 1);
    fixture.finish().await;
}
