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
            closing: false,
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
