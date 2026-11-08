//! Explicit private Chromium dependency proof, outside the ordinary unit suite.
//! The reviewed container runner supplies Chromium; absence is a hard failure.
use o3_server::{
    native_page_worker::{cdp::Payload, JavaScriptJson},
    preview,
    store::State,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
#[derive(Default)]
struct Retain(AtomicUsize);
impl o3_server::cleanup::Removal for Retain {
    fn execute(&self, _: &o3_server::cleanup::RemovalPlan) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "The private proof must retain its browser profile",
        ))
    }
}
async fn open(state: &State, thread: &str) -> String {
    preview::call(
        state,
        "preview.open",
        &json!({"threadId":thread,"runtime":"server","profileId":"default"}),
    )
    .await
    .unwrap()["tabId"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn remote_value(packet: &o3_server::native_page_worker::cdp::Packet) -> Value {
    let Some(Payload::Result(result)) = packet.payload() else {
        panic!("Expected result")
    };
    serde_json::from_str::<Value>(result.json()).unwrap()["result"]["value"].clone()
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    assert!(std::path::Path::new("/run/.containerenv").is_file());
    assert!(
        std::path::Path::new("/usr/bin/chromium").is_file(),
        "Admitted Chromium is required"
    );
    let root = PathBuf::from("/out/owner-pipe");
    std::fs::create_dir(&root).unwrap();
    let launcher = root.join("chromium-fixture.py");
    std::fs::write(
        &launcher,
        include_bytes!("../tests/fixtures/input/native-page-cdp-proxy.py"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("O3_BROWSER_BINARY", &launcher);
    let state = Arc::new(State::open(&root.join("profile")).unwrap());
    let browser = &state.streams.p2.preview;
    let retained = Arc::new(Retain::default());
    state
        .streams
        .p2
        .preview
        .set_profile_removal(retained.clone());
    let one = open(&state, "one").await;
    let two = open(&state, "two").await;
    assert_eq!(
        std::fs::read_dir(state.dir.join("browser-profiles"))
            .unwrap()
            .count(),
        1
    );
    fn params(raw: &str) -> JavaScriptJson {
        JavaScriptJson::parse(raw.into()).unwrap()
    }
    fn session(packet: &o3_server::native_page_worker::cdp::Packet) -> String {
        let Some(Payload::Result(result)) = packet.payload() else {
            panic!("attach result")
        };
        serde_json::from_str::<Value>(result.json()).unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .into()
    }
    let borrowed = browser
        .evaluate_exact(
            "one",
            &one,
            br#"{"expression":"1"}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap()
        .session()
        .unwrap()
        .to_owned();
    let owner = browser.capture_native_page("one", &one).unwrap();
    assert!(browser.capture_native_page("two", &one).is_err());
    let foreign = browser.capture_native_page("two", &two).unwrap();
    let foreign_target = foreign.target().to_owned();
    let mut events = owner.events();
    let captured = owner.clone();
    let (ended_tx, ended_rx) = tokio::sync::oneshot::channel();
    owner.control().agent_admitted("private-agent", None, move |context| async move {
        let invocation = captured.admitted(&context)?;
        let attach = invocation.command("Target.attachToTarget", params(&json!({"targetId":captured.target(),"flatten":true}).to_string()), None, Duration::from_secs(3)).await?;
        let own_session = session(&attach);
        assert_ne!(own_session, borrowed);
        assert!(invocation.command("Target.attachToTarget", params(&json!({"targetId":foreign_target,"flatten":true}).to_string()), None, Duration::from_secs(1)).await.is_err());
        assert!(invocation.command("Browser.close", params("{}"), None, Duration::from_secs(1)).await.is_err());
        invocation.command("Runtime.enable", params("{}"), Some(own_session.clone()), Duration::from_secs(3)).await?;
        let result = invocation.command("Runtime.evaluate", params(r#"{"expression":"console.log({['\ud800']:'\udfff'});'\ud800'","returnByValue":true}"#), Some(own_session.clone()), Duration::from_secs(3)).await?;
        let Some(Payload::Result(raw)) = result.payload() else { panic!("raw result") };
        assert!(raw.json().contains("\\ud800"));
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            loop { let event = events.recv().await.unwrap(); if event.method() == Some("Runtime.consoleAPICalled") { break event; } }
        }).await.unwrap();
        assert_eq!(event.session(), Some(own_session.as_str()));
        let raw = serde_json::to_string(&event).unwrap();
        assert!(raw.contains("\\ud800"));
        assert!(raw.contains("\\udfff"));
        invocation.command("Target.setAutoAttach", params(r#"{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}"#), Some(own_session.clone()), Duration::from_secs(3)).await?;
        invocation.command("Runtime.evaluate", params(r#"{"expression":"globalThis.nativeGrantWorker=new Worker(URL.createObjectURL(new Blob(['globalThis.childValue=41'],{type:'text/javascript'})));true","returnByValue":true}"#), Some(own_session.clone()), Duration::from_secs(3)).await?;
        let child = tokio::time::timeout(Duration::from_secs(3), async {
            loop { let event = events.recv().await.unwrap(); if event.method() == Some("Target.attachedToTarget") { break event; } }
        }).await.unwrap();
        assert_eq!(child.session(), Some(own_session.as_str()));
        let Some(Payload::Parameters(child)) = child.payload() else { panic!("child grant event") };
        let child: Value = serde_json::from_str(child.json()).unwrap();
        assert_eq!(child["targetInfo"]["type"], "worker");
        let child_session = child["sessionId"].as_str().unwrap().to_owned();
        invocation.command("Runtime.enable", params("{}"), Some(child_session.clone()), Duration::from_secs(3)).await?;
        let child_value = invocation.command("Runtime.evaluate", params(r#"{"expression":"globalThis.childValue","returnByValue":true}"#), Some(child_session), Duration::from_secs(3)).await?;
        assert_eq!(remote_value(&child_value), json!(41));
        ended_tx.send((invocation,own_session)).ok().unwrap();
        Ok(Value::Null)
    }).wait().await.unwrap();
    let (ended, old_native_session) = ended_rx.await.unwrap();
    assert!(ended
        .command("Runtime.enable", params("{}"), None, Duration::from_secs(1))
        .await
        .is_err());
    let receipt = owner.close(Duration::from_secs(3)).await;
    assert!(receipt.clean, "{receipt:?}");
    assert_eq!(receipt.owned_sessions, 0);
    assert_eq!(
        remote_value(
            &browser
                .evaluate_exact(
                    "one",
                    &one,
                    br#"{"expression":"'legacy alive'","returnByValue":true}"#,
                    Duration::from_secs(3)
                )
                .await
                .unwrap()
        ),
        json!("legacy alive")
    );

    let reopened = browser.capture_native_page("one", &one).unwrap();
    assert_eq!(owner.target(), reopened.target());
    let captured = reopened.clone();
    reopened
        .control()
        .observe_admitted(move |context| async move {
            let invocation = captured.admitted(&context)?;
            let attached = invocation
                .command(
                    "Target.attachToTarget",
                    params(&json!({"targetId":captured.target(),"flatten":true}).to_string()),
                    None,
                    Duration::from_secs(3),
                )
                .await?;
            let fresh_session = session(&attached);
            assert_ne!(fresh_session, old_native_session);
            assert_eq!(
                remote_value(
                    &invocation
                        .command(
                            "Runtime.evaluate",
                            params(
                                r#"{"expression":"'same target reopened'","returnByValue":true}"#
                            ),
                            Some(fresh_session),
                            Duration::from_secs(3)
                        )
                        .await?
                ),
                json!("same target reopened")
            );
            Ok(Value::Null)
        })
        .wait()
        .await
        .unwrap();
    assert!(reopened.close(Duration::from_secs(3)).await.clean);
    let three = open(&state, "late").await;
    let late = browser.capture_native_page("late", &three).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("delay-next-attach"))
        .unwrap();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let captured = late.clone();
    let running = late.control().agent_admitted("private-agent", None, move |context| async move {
        let invocation = captured.admitted(&context)?;
        tokio::select! {
            result = invocation.command("Target.attachToTarget", params(&json!({"targetId":captured.target(),"flatten":true}).to_string()), None, Duration::from_secs(3)) => panic!("reply must remain delayed: {result:?}"),
            _ = cancel_rx => {}
        }
        Ok(Value::Null)
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !root.join("native-attach-delayed.json").is_file() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel_tx.send(()).unwrap();
    running.wait().await.unwrap();
    let late_receipt = late.close(Duration::from_secs(3)).await;
    assert!(late_receipt.clean, "{late_receipt:?}");
    assert_eq!(late_receipt.receipt_totals[0..2], [1, 1]);
    assert_eq!(
        remote_value(
            &browser
                .evaluate_exact(
                    "two",
                    &two,
                    br#"{"expression":"'other tab alive'","returnByValue":true}"#,
                    Duration::from_secs(3)
                )
                .await
                .unwrap()
        ),
        json!("other tab alive")
    );
    let pending_tab = open(&state, "pending").await;
    let pending_page = browser
        .capture_native_page("pending", &pending_tab)
        .unwrap();
    let mut events = pending_page.events();
    let captured = pending_page.clone();
    let (background_tx, background_rx) = tokio::sync::oneshot::channel();
    pending_page.control().observe_admitted(move |context| async move {
        let invocation = captured.admitted(&context)?;
        let own = session(&invocation.command("Target.attachToTarget", params(&json!({"targetId":captured.target(),"flatten":true}).to_string()), None, Duration::from_secs(3)).await?);
        invocation.command("Runtime.enable", params("{}"), Some(own.clone()), Duration::from_secs(3)).await?;
        let background = tokio::spawn(async move {
            invocation.command("Runtime.evaluate",params(r#"{"expression":"console.log('native-end-watch-started');new Promise(resolve=>setTimeout(()=>resolve(true),350))","awaitPromise":true,"returnByValue":true}"#),Some(own),Duration::from_secs(3)).await
        });
        tokio::time::timeout(Duration::from_secs(3),async {
            loop { let event = events.recv().await.unwrap(); if event.method()==Some("Runtime.consoleAPICalled") { break; } }
        }).await.unwrap();
        background_tx.send(background).ok().unwrap();
        Ok(Value::Null)
    }).wait().await.unwrap();
    let abandoned = tokio::time::timeout(Duration::from_secs(1), background_rx.await.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(abandoned.unwrap_err()["reason"], "callback_ended");
    let pending_receipt = pending_page.close(Duration::from_secs(3)).await;
    assert!(pending_receipt.clean, "{pending_receipt:?}");
    let unknown_tab = open(&state, "unknown").await;
    let unknown = browser
        .capture_native_page("unknown", &unknown_tab)
        .unwrap();
    let captured = unknown.clone();
    unknown
        .control()
        .observe_admitted(move |context| async move {
            captured
                .admitted(&context)?
                .command(
                    "Target.attachToTarget",
                    params(&json!({"targetId":captured.target(),"flatten":true}).to_string()),
                    None,
                    Duration::from_secs(3),
                )
                .await?;
            Ok(Value::Null)
        })
        .wait()
        .await
        .unwrap();
    let unknown_receipt = unknown.close(Duration::ZERO).await;
    assert!(!unknown_receipt.clean);
    assert_eq!(
        browser
            .capture_native_page("unknown", &unknown_tab)
            .err()
            .unwrap()["reason"],
        "previous_grant_cleanup_unknown"
    );
    let recovered = unknown.close(Duration::from_secs(3)).await;
    assert_eq!(recovered.owned_sessions, 0);
    assert!(
        !recovered.clean,
        "An established unknown outcome remains sticky"
    );
    assert!(browser
        .capture_native_page("unknown", &unknown_tab)
        .is_err());
    preview::shutdown(&state).await;
    assert_eq!(retained.0.load(Ordering::SeqCst), 0);
    println!(
        "{}",
        json!({"complete":true,"realActorOwnedSession":true,"borrowedSessionPreserved":true,"foreignTargetAndGlobalCloseRefused":true,"rawOwnedConsoleEvent":true,"actualChildWorkerSession":true,"endedCallbackRefused":true,"lateAttachDrainedAndDetached":true,"otherTabSurvives":true,"normalClose":receipt,"lateClose":late_receipt,"endedCallbackCancelsCorrelationAndLateResultDrains":true,"pendingClose":pending_receipt,"cleanSameTargetRecaptureWorks":true,"unknownRecaptureRefused":true,"unknownClose":recovered,"profileRemovalRequests":retained.0.load(Ordering::SeqCst)})
    );
}
