//! Provider supervision releases preview focus before background cleanup completes.
mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::json;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Peer {
    finish: tokio::sync::Notify,
    reap: tokio::sync::Notify,
    tail: AtomicBool,
}
impl Provider for Peer {
    fn capabilities(&self, instance: &str) -> Option<serde_json::Value> {
        (instance == "scripted").then(|| json!({}))
    }
    fn execute<'a>(
        &'a self,
        _input: RunInput,
        output: tokio::sync::mpsc::Sender<ProviderEvent>,
        _control: tokio::sync::mpsc::Receiver<ProviderControl>,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            output
                .send(ProviderEvent::Started)
                .await
                .map_err(|_| "closed")?;
            self.finish.notified().await;
            self.tail.store(true, Ordering::Release);
            output
                .send(ProviderEvent::Completed)
                .await
                .map_err(|_| "closed")?;
            self.reap.notified().await;
            Ok(())
        })
    }
}
#[test]
fn thinking_counts_and_terminal_event_releases_focus_before_provider_reaping() {
    let peer = Arc::new(Peer::default());
    let inject = peer.clone();
    let server = support::start(move |s| *s.provider.lock().unwrap() = Some(inject));
    let activity = server.state.streams.p2.preview.activity();
    let mut changes = activity.subscribe();
    let client = server.connect();
    client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Activity","modelSelection":{"instanceId":"scripted","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"think","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let next = |changes: &mut tokio::sync::mpsc::UnboundedReceiver<(String, bool)>| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(event) = changes.try_recv() {
                break event;
            }
            assert!(Instant::now() < deadline, "activity edge missing");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    assert_eq!(next(&mut changes), ("t".into(), true));
    assert!(activity.active("t"));
    assert!(!peer.tail.load(Ordering::Acquire));
    peer.finish.notify_one();
    assert_eq!(next(&mut changes), ("t".into(), false));
    assert!(peer.tail.load(Ordering::Acquire));
    assert!(!activity.active("t"));
    peer.reap.notify_one();
}
