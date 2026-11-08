//! Cleanup acknowledgement is injected; profile plans are recorded and never executed.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
#[derive(Default)]
struct Recording(AtomicUsize);
impl crate::cleanup::Removal for Recording {
    fn execute(&self, _plan: &crate::cleanup::RemovalPlan) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn unconfirmed_profile_owner_survives_retry_and_caller_cancellation() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "browser-unconfirmed-{}",
        crate::store::random_id().unwrap()
    ));
    let path = directory(&root, "default").unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("retained"), b"owned fixture").unwrap();
    let browser = Browsers::default();
    let recorder = std::sync::Arc::new(Recording::default());
    *browser.removal.0.lock().unwrap() = recorder.clone();
    let (ack, done) = tokio::sync::watch::channel(false);
    let (tx, _requests) = mpsc::channel(8);
    browser.tabs.lock().unwrap().insert(
        ("thread".into(), "tab".into()),
        Tab {
            exact_tx: mpsc::channel(1).0,
            exact_wake: Default::default(),
            keyboard: Default::default(),
            closing: false,
            activity: Default::default(),
            control: None,
            origin: Origin::Local,
            desktop_channel: None,
            navigation: None,
            events: None,
            process_done: done.clone(),
            session: None,
            target: None,
            profile: "default".into(),
            tx,
            done,
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
        },
    );
    assert!(browser.clear(&root, "default").await.is_err());
    assert_eq!(recorder.0.load(Ordering::SeqCst), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), browser.clear(&root, "default"))
            .await
            .is_err(),
        "unconfirmed retry cannot reach profile removal"
    );
    assert_eq!(recorder.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        browser.connection("thread", "tab").await,
        Some((Origin::Local, false))
    );
    assert!(browser
        .command("thread", "tab", "Runtime.evaluate", json!({}))
        .await
        .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(30),
        browser.close("thread", Some("tab"))
    )
    .await
    .is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), browser.shutdown())
            .await
            .is_err()
    );
    assert_eq!(
        browser.tabs.lock().unwrap().len(),
        1,
        "all cancellation paths retain the receipt"
    );
    ack.send_replace(true);
    browser.clear(&root, "default").await.unwrap();
    assert_eq!(recorder.0.load(Ordering::SeqCst), 1);
    assert!(
        path.join("retained").exists(),
        "recording executor retains the entire fixture tree"
    );
}

#[tokio::test]
async fn process_acknowledgement_cannot_clear_a_profile_before_owned_observation_work_drains() {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "browser-work-receipt-{}",
        crate::store::random_id().unwrap()
    ));
    let path = directory(&root, "default").unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("retained"), b"owned fixture").unwrap();
    let browser = Browsers::default();
    let recorder = std::sync::Arc::new(Recording::default());
    *browser.removal.0.lock().unwrap() = recorder.clone();
    let control = crate::preview_control::runtime::Control::new(Some("provider:owned".into()));
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let observed = control.observe(move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(Value::Null)
    });
    tokio::time::timeout(Duration::from_secs(2), start)
        .await
        .unwrap()
        .unwrap();
    let (_process, process_done) = tokio::sync::watch::channel(true);
    let (_page, done) = tokio::sync::watch::channel(false);
    let (tx, _requests) = mpsc::channel(8);
    browser.tabs.lock().unwrap().insert(
        ("thread".into(), "tab".into()),
        Tab {
            exact_tx: mpsc::channel(1).0,
            exact_wake: Default::default(),
            keyboard: Default::default(),
            closing: false,
            activity: Default::default(),
            control: Some(control.clone()),
            origin: Origin::Local,
            desktop_channel: None,
            navigation: None,
            events: None,
            process_done,
            session: None,
            target: None,
            profile: "default".into(),
            tx,
            done,
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
        },
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), browser.clear(&root, "default"))
            .await
            .is_err()
    );
    assert_eq!(recorder.0.load(Ordering::SeqCst), 0);
    assert_eq!(browser.tabs.lock().unwrap().len(), 1);
    assert!(
        !control.is_finished(),
        "process-only acknowledgement cannot discard an owned callback"
    );
    release.send(()).unwrap();
    observed.wait().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), browser.clear(&root, "default"))
        .await
        .unwrap()
        .unwrap();
    assert!(control.is_finished());
    assert!(browser.tabs.lock().unwrap().is_empty());
    assert_eq!(recorder.0.load(Ordering::SeqCst), 1);
    assert!(
        path.join("retained").exists(),
        "explicit recording removal never deletes the fixture tree"
    );
}
