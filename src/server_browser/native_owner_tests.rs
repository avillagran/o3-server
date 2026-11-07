//! Actual provider credential and callback admission; actor input is retained and
//! inspected, so a denied receipt cannot accidentally reach a browser command.
use super::native_owner::*;
use super::*;
use crate::{events::put, native_page_worker::JavaScriptJson, store::State};
use std::sync::Arc;

#[tokio::test]
async fn actual_provider_scope_and_callback_lifetime_fence_original_receipts() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "native-page-caller-{}",
        crate::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("settings.json"),
        br#"{"enableAgentBrowserAccess":true}"#,
    )
    .unwrap();
    let state = Arc::new(State::open(&root).unwrap());
    let (input, _owned_input) = tokio::io::duplex(4096);
    let channel = crate::desktop_browser_channel::Channel::start(input, tokio::io::sink());
    let driver = crate::preview::install_desktop(state.clone(), channel).await;
    state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
    {
        let db = state.db.lock().unwrap();
        put(&db,"thread","thread","thread",&json!({"id":"thread","projectId":"p","providerInstanceId":"claudeAgent","runtimeMode":"full-access","interactionMode":"default","providerGeneration":"g"})).unwrap();
        put(
            &db,
            "runs",
            "r",
            "thread",
            &json!({"id":"r","providerInstanceId":"claudeAgent","status":"running"}),
        )
        .unwrap();
    }
    let registry: events::Shared = Default::default();
    let (_done_tx, done) = tokio::sync::watch::channel(false);
    let (exact_tx, mut exact_rx) = mpsc::channel(8);
    let (tx, _rx) = mpsc::channel(8);
    state
        .streams
        .p2
        .preview
        .browser
        .tabs
        .lock()
        .unwrap()
        .insert(
            ("thread".into(), "tab".into()),
            Tab {
                exact_tx,
                exact_wake: Default::default(),
                closing: false,
                keyboard: Default::default(),
                activity: Default::default(),
                control: Some(crate::preview_control::runtime::Control::new(None)),
                origin: Origin::Local,
                desktop_channel: None,
                navigation: None,
                events: Some(registry),
                process_done: done.clone(),
                session: Some("borrowed".into()),
                target: Some("target".into()),
                profile: "owned".into(),
                tx,
                done,
                cancel: Default::default(),
            },
        );
    let provider = state
        .native_mcp
        .obtain(&state, "thread", "claudeAgent", &root, &Value::Null)
        .await
        .unwrap();
    let page = state
        .streams
        .p2
        .preview
        .capture_native_page("thread", "tab")
        .unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (end_tx, end_rx) = oneshot::channel();
    let captured = page.clone();
    let credential = provider.clone();
    let running = page.control().observe_admitted(move |context| async move {
        let invocation =
            captured.admitted_provider(&context, &credential, NativeOperation::Evaluate)?;
        let read = captured.admitted_provider(&context, &credential, NativeOperation::Snapshot)?;
        drop(credential);
        let receipt = invocation.issue_receipt()?;
        assert_eq!(receipt.worker_receipt().len(), 48);
        entered_tx
            .send((invocation, read, receipt, context))
            .ok()
            .unwrap();
        end_rx.await.unwrap();
        Ok(Value::Null)
    });
    let (invocation, read, receipt, context) = entered_rx.await.unwrap();
    let original = receipt.worker_receipt().to_owned();
    page.validate_receipt_operation(&original, NativeOperation::Evaluate)
        .unwrap();
    assert!(page
        .validate_receipt_operation(&original, NativeOperation::Snapshot)
        .is_err());
    assert!(read.for_operation(NativeOperation::Evaluate).is_err());
    assert!(read.for_operation(NativeOperation::Act).is_err());

    {
        let db = state.db.lock().unwrap();
        let mut thread = crate::events::get(&db, "thread", "thread").unwrap();
        thread["archivedAt"] = json!("synthetic");
        put(&db, "thread", "thread", "thread", &thread).unwrap();
    }
    assert!(
        invocation.issue_receipt().is_err(),
        "operate admission checks actual archived/run state"
    );
    let read_receipt = read
        .issue_receipt()
        .expect("native metadata remains readable");
    page.validate_receipt_operation(read_receipt.worker_receipt(), NativeOperation::Snapshot)
        .unwrap();
    assert!(page
        .validate_receipt_operation(read_receipt.worker_receipt(), NativeOperation::Evaluate)
        .is_err());
    assert!(page
        .validate_receipt_operation(read_receipt.worker_receipt(), NativeOperation::Act)
        .is_err());
    page.finish_receipt(read_receipt).unwrap();
    for operation in [
        NativeOperation::Status,
        NativeOperation::Snapshot,
        NativeOperation::WaitFor,
    ] {
        let allowed = page
            .admitted_provider(&context, &provider, operation)
            .unwrap()
            .issue_receipt()
            .unwrap();
        page.validate_receipt_operation(allowed.worker_receipt(), operation)
            .unwrap();
        page.finish_receipt(allowed).unwrap();
    }
    for operation in [
        NativeOperation::Open,
        NativeOperation::Dialog,
        NativeOperation::Navigate,
        NativeOperation::Resize,
        NativeOperation::SetAppearance,
        NativeOperation::Act,
        NativeOperation::Evaluate,
        NativeOperation::RecordingStart,
        NativeOperation::RecordingStop,
    ] {
        assert!(
            page.admitted_provider(&context, &provider, operation)
                .is_err(),
            "{} must act as the actual caller",
            operation.tool_name()
        );
    }

    assert!(page
        .command_receipt(
            &original,
            "Browser.getVersion",
            JavaScriptJson::parse("{}".into()).unwrap(),
            None,
            Duration::from_secs(1)
        )
        .await
        .is_err());
    assert!(exact_rx.try_recv().is_err());
    {
        let db = state.db.lock().unwrap();
        let mut thread = crate::events::get(&db, "thread", "thread").unwrap();
        thread["archivedAt"] = Value::Null;
        thread["providerGeneration"] = json!("successor");
        put(&db, "thread", "thread", "thread", &thread).unwrap();
    }
    assert!(
        read.issue_receipt().is_err(),
        "read also checks actual provider generation"
    );
    {
        let db = state.db.lock().unwrap();
        let mut thread = crate::events::get(&db, "thread", "thread").unwrap();
        thread["providerGeneration"] = json!("g");
        put(&db, "thread", "thread", "thread", &thread).unwrap();
    }
    assert!(invocation.issue_receipt().is_ok());
    drop(provider);
    assert!(
        invocation.issue_receipt().is_err(),
        "Context drop revokes while callback is still entered"
    );
    assert!(page
        .command_receipt(
            &original,
            "Browser.getVersion",
            JavaScriptJson::parse("{}".into()).unwrap(),
            None,
            Duration::from_secs(1)
        )
        .await
        .is_err());
    assert!(exact_rx.try_recv().is_err());
    page.finish_receipt(receipt).unwrap();
    end_tx.send(()).unwrap();
    running.wait().await.unwrap();
    assert!(read.issue_receipt().is_err());
    assert!(page.close(Duration::from_secs(1)).await.clean);
    drop(driver);
}
