//! Private physical acceptance probe; never a product command or an MCP route.
use o3_server::native_page_worker::{Fault, GrantOwner, OperationResult, Worker, WorkerSpec};
use serde_json::{json, Value};
use std::{
    future::Future,
    io::{BufRead, Read, Write},
    net::TcpStream,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::watch;
struct Owner {
    target: String,
    endpoint: String,
    nonce: String,
    control: Mutex<TcpStream>,
    revoked: watch::Sender<bool>,
    fenced: AtomicBool,
}
impl Owner {
    fn exchange(&self, kind: &str) -> Result<(), Fault> {
        self.exchange_reply(kind).map(|_| ())
    }
    fn exchange_reply(&self, kind: &str) -> Result<Value, Fault> {
        let mut socket = self.control.lock().map_err(|_| Fault::CleanupUnknown)?;
        let line = json!({"kind":kind,"nonce":self.nonce}).to_string() + "\n";
        socket
            .write_all(line.as_bytes())
            .map_err(|_| Fault::CleanupUnknown)?;
        let mut body = Vec::new();
        let mut byte = [0];
        while body.len() < 8192 {
            socket
                .read_exact(&mut byte)
                .map_err(|_| Fault::CleanupUnknown)?;
            if byte[0] == b'\n' {
                break;
            }
            body.push(byte[0]);
        }
        let reply: Value = serde_json::from_slice(&body).map_err(|_| Fault::CleanupUnknown)?;
        if reply["kind"] != format!("{kind}-ack") {
            return Err(Fault::CleanupUnknown);
        }
        Ok(reply)
    }
}
impl GrantOwner for Owner {
    fn target_id(&self) -> &str {
        &self.target
    }
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn revocation(&self) -> watch::Receiver<bool> {
        self.revoked.subscribe()
    }
    fn revoke(&self) {
        if self.exchange("revoke").is_ok() {
            self.fenced.store(true, Ordering::SeqCst);
        }
        let _ = self.revoked.send(true);
    }
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
        Box::pin(async move {
            if !self.fenced.load(Ordering::SeqCst) {
                return Err(Fault::CleanupUnknown);
            }
            self.exchange("close")
        })
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    assert!(
        std::path::Path::new("/run/.containerenv").exists(),
        "Private Podman required"
    );
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    assert!(line.len() < 65536);
    let c: Value = serde_json::from_str(&line).unwrap();
    let socket =
        TcpStream::connect(("127.0.0.1", c["controlPort"].as_u64().unwrap() as u16)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (revoked, _) = watch::channel(false);
    let owner = Arc::new(Owner {
        target: c["targetId"].as_str().unwrap().into(),
        endpoint: c["endpoint"].as_str().unwrap().into(),
        nonce: c["controlNonce"].as_str().unwrap().into(),
        control: Mutex::new(socket),
        revoked,
        fenced: AtomicBool::new(false),
    });
    let spec = WorkerSpec {
        node: PathBuf::from("/node/bin/node"),
        entry: PathBuf::from("/source/runtime/preview/entry.mjs"),
        private_home: PathBuf::from("/out/worker-home"),
        configuration: json!({"nativeRoot":"/native","playwrightRoot":"/playwright","effectRoot":"/deps/node_modules/effect"}),
        startup_deadline: Duration::from_secs(20),
        close_deadline: Duration::from_secs(3),
    };
    let worker = Worker::spawn(spec, owner.clone()).await.unwrap();
    if c["mode"] == "revoke" {
        let call = worker.clone();
        let pending = tokio::spawn(async move {
            call.invoke(
                "waitFor",
                json!({"text":"not-ever","timeoutMs":5000}),
                Duration::from_secs(5),
            )
            .await
        });
        let barrier = owner.clone();
        tokio::task::spawn_blocking(move || barrier.exchange("await-read"))
            .await
            .unwrap()
            .unwrap();
        owner.revoke();
        assert_eq!(pending.await.unwrap().unwrap_err(), Fault::Revoked);
        let receipt = worker.close().await;
        assert!(receipt.worker_reaped && receipt.owner_grant_closed);
        assert_eq!(receipt.executed_recursive_removals, Some(0));
        println!(
            "{}",
            json!({"complete":true,"rustSupervisor":true,"nativePageChildProcess":true,"revokedLiveNativeWait":true,"workerReaped":receipt.worker_reaped,"ownerClosed":receipt.owner_grant_closed,"nativeRecursiveCleanupRequests":receipt.native_cleanup_requests,"executedRecursiveRemovals":receipt.executed_recursive_removals})
        );
        return;
    }
    let control = o3_server::preview_control::runtime::Control::new(Some("agent".into()));
    let controlled = o3_server::native_page_worker::control::ControlledPage::new(
        worker.clone(),
        control.clone(),
    );
    let html="<input id='field'><button id='button' onclick=\"this.textContent='Clicked'\">Click</button>";
    let expression = format!(
        "document.body.innerHTML={}; true",
        serde_json::to_string(html).unwrap()
    );
    assert!(matches!(
        worker
            .invoke(
                "evaluate",
                json!({"expression":expression}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::Value(_)
    ));
    worker
        .invoke(
            "preview_act",
            json!({"action":"type","locator":"#field","text":"native","clear":true}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    worker
        .invoke(
            "preview_press",
            json!({"key":"End"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    worker
        .invoke(
            "preview_act",
            json!({"action":"press","key":"x"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        worker
            .invoke(
                "evaluate",
                json!({"expression":"document.querySelector('#field').value"}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::Value(json!("nativex"))
    );
    assert!(matches!(
        worker
            .invoke(
                "preview_press",
                json!({"key":"NotARealKey","modifiers":["Control"]}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::NativeError(_)
    ));
    worker
        .invoke(
            "preview_act",
            json!({"action":"press","key":"a"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    worker
        .invoke(
            "preview_press",
            json!({"key":"Control"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    worker
        .invoke(
            "preview_act",
            json!({"action":"press","key":"z"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        worker
            .invoke(
                "evaluate",
                json!({"expression":"document.querySelector('#field').value"}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::Value(json!("z"))
    );
    worker
        .invoke(
            "click",
            json!({"locator":"#button"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        worker
            .invoke(
                "evaluate",
                json!({"expression":"document.querySelector('#button').textContent"}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::Value(json!("Clicked"))
    );
    let OperationResult::Value(snapshot) = worker
        .invoke("snapshot", json!({}), Duration::from_secs(5))
        .await
        .unwrap()
    else {
        panic!("Native snapshot failed")
    };
    assert!(snapshot["accessibilityTree"]
        .as_str()
        .unwrap()
        .contains("ref=t3-"));
    let tree = snapshot["accessibilityTree"].as_str().unwrap();
    let old_ref = tree
        .split("ref=")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    control.take("viewer").wait().await.unwrap();
    control
        .release("viewer", || async { Ok(Value::Null) })
        .wait()
        .await
        .unwrap();
    controlled
        .agent(
            "agent",
            "evaluate",
            json!({"expression":"true"}),
            Duration::from_secs(3),
            true,
        )
        .await
        .unwrap();
    let OperationResult::NativeError(stale) = controlled
        .agent(
            "agent",
            "click",
            json!({"locator":format!("aria-ref={old_ref}"),"timeoutMs":100}),
            Duration::from_secs(3),
            true,
        )
        .await
        .unwrap()
    else {
        panic!("Expected stale reference refusal")
    };
    assert_eq!(stale["tag"], "PreviewAutomationInvalidSelectorError");
    assert_eq!(stale["detail"]["staleRef"], true);
    worker
        .invoke(
            "preview_press",
            json!({"key":"End"}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    worker
        .invoke(
            "preview_act",
            json!({"action":"type","locator":"#field","text":"after-handoff","clear":true}),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(
        worker
            .invoke(
                "evaluate",
                json!({"expression":"document.querySelector('#field').value"}),
                Duration::from_secs(3)
            )
            .await
            .unwrap(),
        OperationResult::Value(json!("after-handoff"))
    );
    for (expression, expected) in [
        ("String.fromCharCode(0xd800)", r#""\ud800""#),
        (
            "({[String.fromCharCode(0xd800)]:String.fromCharCode(0xdfff)})",
            r#"{"\ud800":"\udfff"}"#,
        ),
    ] {
        let OperationResult::JavaScriptValue(value) = controlled
            .agent(
                "agent",
                "evaluate",
                json!({"expression":expression}),
                Duration::from_secs(3),
                true,
            )
            .await
            .unwrap()
        else {
            panic!("Actual native Unicode value lost")
        };
        assert_eq!(value.json(), expected);
    }
    let captured = owner.exchange_reply("unicode").unwrap();
    let packets = captured["packets"].as_array().unwrap();
    assert_eq!(packets.len(), 2);
    for raw in packets {
        let raw = raw.as_str().unwrap().as_bytes();
        assert!(serde_json::from_slice::<Value>(raw).is_err());
        let decoded = o3_server::native_page_worker::cdp::Packet::decode(raw, 4096).unwrap();
        assert!(decoded.id().is_some() && decoded.method().is_none());
        let encoded = serde_json::to_vec(&decoded).unwrap();
        assert_eq!(
            decoded,
            o3_server::native_page_worker::cdp::Packet::decode(&encoded, 4096).unwrap()
        );
    }
    // Entered native callbacks drain through handoff and close. The fixture
    // signals only a synthetic promise on this exact owned target/session.
    let entered = controlled.clone();
    let running = tokio::spawn(async move {
        entered.agent("agent", "evaluate", json!({"expression":"new Promise(resolve => { globalThis.__drainOne = () => resolve('drained-handoff'); })"}), Duration::from_secs(5), true).await
    });
    let barrier = owner.clone();
    tokio::task::spawn_blocking(move || barrier.exchange("await-drain-one"))
        .await
        .unwrap()
        .unwrap();
    let taking = control.take("viewer");
    // Establish fresh references after the immediate invalidation, while the
    // old operation still owns the draining tail. Its finally must clear these.
    controlled.synchronize_references().await.unwrap();
    let OperationResult::Value(during) = worker
        .invoke("snapshot", json!({}), Duration::from_secs(3))
        .await
        .unwrap()
    else {
        panic!("snapshot during handoff")
    };
    let during_ref = during["accessibilityTree"]
        .as_str()
        .unwrap()
        .split("ref=")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_owned();
    owner.exchange("release-drain-one").unwrap();
    assert_eq!(
        running.await.unwrap().unwrap(),
        OperationResult::Value(json!("drained-handoff"))
    );
    taking.wait().await.unwrap();
    let OperationResult::NativeError(stale) = worker
        .invoke(
            "click",
            json!({"locator":format!("aria-ref={during_ref}"),"timeoutMs":100}),
            Duration::from_secs(3),
        )
        .await
        .unwrap()
    else {
        panic!("finally did not invalidate references")
    };
    assert_eq!(stale["detail"]["staleRef"], true);
    control
        .release("viewer", || async { Ok(Value::Null) })
        .wait()
        .await
        .unwrap();
    let entered = controlled.clone();
    let running = tokio::spawn(async move {
        entered.agent("agent", "evaluate", json!({"expression":"new Promise(resolve => { globalThis.__drainTwo = () => resolve('drained-close'); })"}), Duration::from_secs(5), true).await
    });
    let barrier = owner.clone();
    tokio::task::spawn_blocking(move || barrier.exchange("await-drain-two"))
        .await
        .unwrap()
        .unwrap();
    let mut changed = control.subscribe();
    let closing_page = controlled.clone();
    let closing = tokio::spawn(async move { closing_page.close().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !changed.borrow().closed {
            changed.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    owner.exchange("release-drain-two").unwrap();
    assert_eq!(
        running.await.unwrap().unwrap(),
        OperationResult::Value(json!("drained-close"))
    );
    let owner_close = closing.await.unwrap();
    assert!(owner_close.worker_reaped && owner_close.owner_grant_closed);
    let receipt = controlled.close().await;
    control.close().wait().await.unwrap();
    control.shutdown().await.unwrap();
    assert!(
        receipt.worker_reaped && receipt.owner_grant_closed && receipt.graceful_worker,
        "{receipt:?}"
    );
    assert_eq!(receipt.executed_recursive_removals, Some(0));
    println!(
        "{}",
        json!({"complete":true,"rustSupervisor":true,"nativePageChildProcess":true,"sameKeyboardAliases":true,"failedChordStateCrossesAliases":true,"ownerRefInvalidationPreservesPage":true,"nativeRawCdpRoundtrip":true,"controlRetainsExactJavaScript":true,"enteredHandoffFinallyInvalidates":true,"enteredCloseDrainsBeforeReap":true,"workerReaped":receipt.worker_reaped,"ownerClosed":receipt.owner_grant_closed,"nativeRecursiveCleanupRequests":receipt.native_cleanup_requests,"executedRecursiveRemovals":receipt.executed_recursive_removals})
    );
}
