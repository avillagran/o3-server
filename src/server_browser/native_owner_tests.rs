//! Actual provider credential and callback admission; actor input is retained and
//! inspected, so a denied receipt cannot accidentally reach a browser command.
use super::native_owner::*;
use super::*;
use crate::{events::put, native_page_worker::JavaScriptJson, store::State};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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

    // Real private WebSocket admission, actual caller receipt and exact actor
    // queue. The actor result below is synthetic, not a Chromium acceptance.
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let gateway = page.gateway().await.unwrap();
    assert!(page.gateway().await.is_err(), "one worker binding per Page");
    let endpoint = url::Url::parse(gateway.endpoint()).unwrap();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", endpoint.port().unwrap()))
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::client_async(gateway.endpoint(), stream)
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({
                "id":1,"method":"Browser.getVersion","params":{},
                "__o3OwnerReceipt":original,"__o3OwnerOperation":"preview_snapshot"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let rejected = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let rejected: Value = serde_json::from_str(&rejected).unwrap();
    assert!(rejected["error"].is_object());
    assert!(
        exact_rx.try_recv().is_err(),
        "wrong class never enters actor"
    );
    socket
        .send(Message::Text(
            json!({
                "id":2,"method":"Browser.getVersion","params":{},
                "__o3OwnerReceipt":original,"__o3OwnerOperation":"preview_evaluate"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let admitted = tokio::time::timeout(Duration::from_secs(1), exact_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.method, "Browser.getVersion");
    socket
        .send(Message::Text(
            json!({
                "id":3,"method":"O3Owner.endScope","params":{},
                "__o3OwnerReceipt":original,"__o3OwnerOperation":"preview_evaluate"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), socket.next())
            .await
            .is_err(),
        "marker cannot acknowledge a merely queued frame"
    );
    assert!(admitted.permit.begin());
    let marker = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let marker: Value = serde_json::from_str(&marker).unwrap();
    assert_eq!(marker["id"], 3);
    assert_eq!(marker["result"]["scopeEnded"], true);
    // No actor result exists yet: marker completion is actual begin, not the
    // response or an invented extension of the original callback tail.
    socket
        .send(Message::Text(
            json!({
                "id":4,"method":"Browser.getVersion","params":{},
                "__o3OwnerReceipt":original,"__o3OwnerOperation":"preview_evaluate"
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let sealed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let sealed: Value = serde_json::from_str(&sealed).unwrap();
    assert_eq!(sealed["id"], 4);
    assert!(sealed["error"].is_object());
    assert!(exact_rx.try_recv().is_err());
    let answer = crate::native_page_worker::cdp::Packet::decode(
        br#"{"id":99,"result":{"product":"private-fixture","lone":"\ud800"}}"#,
        4096,
    )
    .unwrap();
    admitted.observer.as_ref().unwrap().completed(&answer);
    admitted.reply.send(Ok(answer)).ok().unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert!(answered.contains(r#""lone":"\ud800""#));
    assert!(answered.contains(r#""id":2"#));
    let fifo = invocation.issue_receipt().unwrap();
    for index in 0..16 {
        socket
            .send(Message::Text(
                json!({
                    "id":5+index,"method":"Browser.getVersion","params":{},
                    "__o3OwnerReceipt":fifo.worker_receipt(),"__o3OwnerOperation":"preview_evaluate"
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    }
    let mut queued = Vec::new();
    // Draining the actor queue requires no CDP reply. A gateway that serializes
    // responses instead of enqueue would stall on the second request here.
    for _ in 0..16 {
        let request = tokio::time::timeout(Duration::from_secs(2), exact_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(request.permit.begin());
        queued.push(request);
    }
    for (ordinal, request) in queued.into_iter().enumerate() {
        let answer = crate::native_page_worker::cdp::Packet::decode(
            json!({"id":100+ordinal,"result":{"ordinal":ordinal}})
                .to_string()
                .as_bytes(),
            4096,
        )
        .unwrap();
        request.observer.as_ref().unwrap().completed(&answer);
        request.reply.send(Ok(answer)).ok().unwrap();
    }
    let mut ordered = std::collections::BTreeMap::new();
    for _ in 0..16 {
        let answer = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        ordered.insert(
            answer["id"].as_u64().unwrap(),
            answer["result"]["ordinal"].as_u64().unwrap(),
        );
    }
    assert_eq!(
        ordered,
        (0..16).map(|index| (5 + index, index)).collect(),
        "actual actor enqueue follows WebSocket production order on a multithread runtime"
    );
    page.finish_receipt(fifo).unwrap();

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
    let waiting_page = page.clone();
    let waiting_receipt = original.clone();
    let waiting = tokio::spawn(async move {
        waiting_page
            .command_receipt(
                &waiting_receipt,
                "Browser.getVersion",
                JavaScriptJson::parse("{}".into()).unwrap(),
                None,
                Duration::from_secs(5),
            )
            .await
    });
    let unwritten = tokio::time::timeout(Duration::from_secs(1), exact_rx.recv())
        .await
        .unwrap()
        .unwrap();
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
    let stopped = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(stopped["reason"], "provider_revoked");
    assert!(
        context.is_active(),
        "credential revocation is distinct from callback completion"
    );
    assert!(
        !unwritten.permit.begin(),
        "the original queued frame cannot start after revocation"
    );
    drop(unwritten);
    page.finish_receipt(receipt).unwrap();
    end_tx.send(()).unwrap();
    running.wait().await.unwrap();
    assert!(read.issue_receipt().is_err());
    assert!(page.close(Duration::from_secs(1)).await.clean);
    assert!(gateway.shutdown().await.unwrap().clean);
    drop(socket);
    drop(driver);
}

#[test]
fn toolkit_class_cannot_select_another_handlers_page_operation() {
    let cases = [
        (NativeOperation::Status, vec!["status"]),
        (NativeOperation::Open, vec!["open", "navigate"]),
        (NativeOperation::Dialog, vec!["dialog"]),
        (NativeOperation::Navigate, vec!["navigate"]),
        (NativeOperation::Resize, vec!["resize"]),
        (NativeOperation::SetAppearance, vec!["setColorScheme"]),
        (NativeOperation::Snapshot, vec!["snapshot"]),
        (
            NativeOperation::Act,
            vec![
                "click", "type", "hover", "select", "drag", "upload", "press", "scroll",
            ],
        ),
        (NativeOperation::Evaluate, vec!["evaluate"]),
        (NativeOperation::WaitFor, vec!["waitFor"]),
        (NativeOperation::RecordingStart, vec!["recordingStart"]),
        (NativeOperation::RecordingStop, vec!["recordingStop"]),
    ];
    let operations: Vec<_> = cases
        .iter()
        .flat_map(|(_, operations)| operations.iter().copied())
        .collect();
    for (class, allowed) in cases {
        assert_eq!(NativeOperation::from_tool(class.tool_name()), Some(class));
        for operation in &operations {
            assert_eq!(
                class.permits_page_operation(operation),
                allowed.contains(operation),
                "{}: {operation}",
                class.tool_name()
            );
        }
        for forbidden in [
            "preview_act",
            "setAppearance",
            "diagnostics",
            "close",
            "Runtime.evaluate",
            "",
            "status\0",
        ] {
            assert!(!class.permits_page_operation(forbidden));
        }
    }
}
