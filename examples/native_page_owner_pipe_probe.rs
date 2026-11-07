//! Explicit private Chromium dependency proof, outside the ordinary unit suite.
//! The reviewed container runner supplies Chromium; absence is a hard failure.
use o3_server::{native_page_worker::cdp::Payload, preview, store::State};
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
    std::fs::write(&launcher, b"#!/usr/bin/python3\nimport os,sys\nassert os.path.isfile('/run/.containerenv')\nfd=os.open('/out/owner-pipe/chromium.stderr',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)\nos.dup2(fd,2)\nos.close(fd)\nos.execv('/usr/bin/chromium',['chromium','--no-sandbox','--disable-dev-shm-usage',*sys.argv[1:]])\n").unwrap();
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
    let params = br#"{"expression":"'\ud800'","awaitPromise":true,"returnByValue":true}"#;
    let packet = browser
        .evaluate_exact("one", &one, params, Duration::from_secs(3))
        .await
        .unwrap();
    let Some(Payload::Result(value)) = packet.payload() else {
        panic!("Expected exact browser result")
    };
    assert!(serde_json::from_str::<Value>(value.json()).is_err());
    assert!(value.json().to_ascii_lowercase().contains("\\ud800"));
    assert!(browser
        .evaluate_exact(
            "one",
            &one,
            br#"{"expression":"1","contextId":123}"#,
            Duration::from_secs(1)
        )
        .await
        .is_err());
    let late = br#"{"expression":"globalThis.o3ExactPendingStarted=true;new Promise(resolve => setTimeout(() => {globalThis.o3ExactPendingSettled=true;resolve('\udfff')},200))","awaitPromise":true,"returnByValue":true}"#;
    let error = browser
        .evaluate_exact("one", &one, late, Duration::from_millis(20))
        .await
        .unwrap_err();
    assert_eq!(error["reason"], "deadline");
    let started = browser
        .evaluate_exact(
            "one",
            &one,
            br#"{"expression":"globalThis.o3ExactPendingStarted===true","returnByValue":true}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(remote_value(&started), json!(true));
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let settled = browser.evaluate_exact("one", &one, br#"{"expression":"globalThis.o3ExactPendingSettled===true","returnByValue":true}"#, Duration::from_secs(3)).await.unwrap();
            if remote_value(&settled) == json!(true) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let before_refresh = browser
        .evaluate_exact(
            "two",
            &two,
            br#"{"expression":"1","returnByValue":true}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    preview::call(
        &state,
        "preview.refresh",
        &json!({"threadId":"two","tabId":two}),
    )
    .await
    .unwrap();
    let ordinary = browser
        .evaluate_exact(
            "two",
            &two,
            br#"{"expression":"'other target survives'","returnByValue":true}"#,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(ordinary.session(), before_refresh.session());
    assert_eq!(remote_value(&ordinary), json!("other target survives"));
    preview::shutdown(&state).await;
    assert_eq!(retained.0.load(Ordering::SeqCst), 0);
    assert!(browser
        .evaluate_exact("one", &one, params, Duration::from_secs(1))
        .await
        .is_err());
    println!(
        "{}",
        json!({"complete":true,"actualRustOwnerPipe":true,"exactArguments":true,"exactResult":true,"lateResponseDrained":true,"otherTargetSurvived":true,"sharedProfileDirectory":true,"contextOverrideRefused":true,"closedOwnerRefused":true,"profileRemovalRequests":retained.0.load(Ordering::SeqCst)})
    );
}
