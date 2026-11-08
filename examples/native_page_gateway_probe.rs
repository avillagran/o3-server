//! Explicit private Chromium dependency proof, outside the ordinary unit suite.
//! The reviewed container runner supplies Chromium; absence is a hard failure.
use o3_server::{
    native_page_worker::{NativeOperation, OperationResult, Worker, WorkerSpec},
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
#[tokio::main(flavor = "current_thread")]
async fn main() {
    assert!(std::path::Path::new("/run/.containerenv").is_file());
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
    let retained = Arc::new(Retain::default());
    state
        .streams
        .p2
        .preview
        .set_profile_removal(retained.clone());
    let tab = open(&state, "private-thread").await;
    let owner = state
        .streams
        .p2
        .preview
        .capture_native_page("private-thread", &tab)
        .unwrap();
    let gateway = Arc::new(owner.gateway().await.unwrap());
    let home = PathBuf::from("/out/native-page-worker");
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut spec = WorkerSpec {
        node: "/node/bin/node".into(),
        entry: "/source/runtime/preview/entry.mjs".into(),
        private_home: home,
        configuration: json!({"nativeRoot":"/native","playwrightRoot":"/playwright","effectRoot":"/deps/node_modules/effect"}),
        startup_deadline: Duration::from_secs(20),
        close_deadline: Duration::from_secs(10),
    };
    gateway.configure_worker(&mut spec).unwrap();
    assert!(gateway.configure_worker(&mut spec).is_err());
    let worker = Worker::spawn(spec, gateway.clone()).await.unwrap();
    let executing = worker.clone();
    let captured = owner.clone();
    owner
        .control()
        .agent_admitted("private-agent", None, move |context| async move {
            let invocation = captured
                .admitted(&context)?
                .for_operation(NativeOperation::Evaluate)?;
            let result = executing
                .invoke_native_scoped(
                    &invocation,
                    "evaluate",
                    json!({"expression":"'native scoped result'"}),
                    Duration::from_secs(3),
                    true,
                )
                .await
                .map_err(|error| json!({"privateFault":format!("{error:?}")}))?;
            assert_eq!(
                result,
                OperationResult::Value(json!("native scoped result"))
            );
            let snapshot = captured
                .admitted(&context)?
                .for_operation(NativeOperation::Snapshot)?;
            assert!(executing
                .invoke_native_scoped(
                    &snapshot,
                    "evaluate",
                    json!({"expression":"'not allowed'"}),
                    Duration::from_secs(1),
                    true
                )
                .await
                .is_err());
            Ok(Value::Null)
        })
        .wait()
        .await
        .unwrap();
    let executing = worker.clone();
    let captured = owner.clone();
    owner
        .control()
        .agent_admitted("private-agent", None, move |context| async move {
            let invocation = captured
                .admitted(&context)?
                .for_operation(NativeOperation::Evaluate)?;
            let result = executing
                .invoke_native_scoped(
                    &invocation,
                    "evaluate",
                    json!({"expression":"'\\ud800'"}),
                    Duration::from_secs(3),
                    true,
                )
                .await
                .map_err(|error| json!({"privateFault":format!("{error:?}")}))?;
            let OperationResult::JavaScriptValue(raw) = result else {
                panic!("Expected exact JavaScript result")
            };
            assert_eq!(raw.json(), r#""\ud800""#);
            Ok(Value::Null)
        })
        .wait()
        .await
        .unwrap();
    let closed = worker.close().await;
    assert!(
        closed.worker_reaped && closed.owner_grant_closed && closed.graceful_worker,
        "{closed:?}"
    );
    let grant = gateway.shutdown().await.unwrap();
    assert!(grant.clean, "{grant:?}");
    let legacy = state
        .streams
        .p2
        .preview
        .evaluate_exact(
            "private-thread",
            &tab,
            br#"{"expression":"1","returnByValue":true}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert!(legacy.payload().is_some());
    preview::shutdown(&state).await;
    assert_eq!(retained.0.load(Ordering::SeqCst), 0);
    println!(
        "{}",
        json!({"complete":true,"actualActor":true,"nativePageWorker":true,"realGateway":true,"originalOperationClassRefusedMutation":true,"rawIsolatedUtf16":true,"borrowedTabSurvives":true,"workerReaped":closed.worker_reaped,"grantClean":grant.clean,"nativeCleanupRequests":closed.native_cleanup_requests,"executedRecursiveRemovals":closed.executed_recursive_removals,"profileRemovalRequests":retained.0.load(Ordering::SeqCst)})
    );
}
