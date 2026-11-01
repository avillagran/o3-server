mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
struct Stream(Arc<tokio::sync::Notify>);
impl Provider for Stream {
    fn execute<'a>(
        &'a self,
        _: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        _: tokio::sync::mpsc::Receiver<ProviderControl>,
        _: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            out.send(ProviderEvent::Started).await.unwrap();
            for t in ["first\n\n", "second"] {
                out.send(ProviderEvent::TextDelta(t.into())).await.unwrap();
            }
            self.0.notified().await;
            out.send(ProviderEvent::Completed).await.unwrap();
            Ok(())
        })
    }
}
#[test]
fn real_settings_reader_controls_paragraph_and_turn_publication() {
    for mode in ["paragraph", "turn"] {
        let release = Arc::new(tokio::sync::Notify::new());
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(Stream(release.clone())))
        });
        let c = s.connect();
        c.rpc(
            "server.updateSettings",
            t3_protocol::native_settings::patch(
                &json!({"key":"responseStreamingMode","value":mode}),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Streaming","modelSelection":{"instanceId":"test","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"m","threadId":"t","messageId":"m","text":"stream","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let projection = || {
            c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let p = projection();
            let assistant = p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "assistant");
            if mode == "paragraph" {
                if assistant.is_some_and(|m| m["text"] == "first\n\n") {
                    break;
                }
            } else if p["runs"][0]["status"] == "running" {
                std::thread::sleep(Duration::from_millis(100));
                assert!(projection()["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|m| m["role"] != "assistant"));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        release.notify_one();
        while projection()["runs"][0]["status"] != "completed" {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(projection()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "first\n\nsecond"));
    }
}
