mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
struct Health(Arc<AtomicUsize>);
impl Provider for Health {
    fn config(&self) -> Vec<Value> {
        vec![
            json!({"instanceId":"codex","driver":"codex","installed":true,"enabled":true,"version":format!("v{}",self.0.load(Ordering::SeqCst))}),
        ]
    }
    fn refresh<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn execute<'a>(
        &'a self,
        _i: RunInput,
        _o: tokio::sync::mpsc::Sender<ProviderEvent>,
        _c: tokio::sync::mpsc::Receiver<ProviderControl>,
        _x: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
#[test]
fn profile_controls_real_health_probes_and_readers_observe_versions() {
    let polls = Arc::new(AtomicUsize::new(0));
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(Arc::new(Health(polls.clone()))));
    let c = s.connect();
    for (profile, ms) in [("performance", 60000), ("battery-saver", 900000)] {
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        let patch = t3_protocol::native_settings::patch(
            &json!({"key":"backgroundActivityProfile","value":profile}),
            Some(&config),
        )
        .unwrap();
        let saved = c.rpc("server.updateSettings", patch).unwrap();
        assert_eq!(saved["providerHealthRefreshInterval"], ms);
        assert!(t3_protocol::native_settings::snapshot(&saved)["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["key"] == "backgroundActivityProfile" && f["value"] == profile));
    }
    let set = |ms| {
        c.rpc("server.updateSettings",json!({"patch":{"backgroundActivity":{"profile":"custom","baseProfile":"balanced","overrides":{"providerHealthRefreshInterval":ms}}}})).unwrap()
    };
    set(1000);
    let end = Instant::now() + Duration::from_secs(4);
    while polls.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let view = t3_protocol::provider_instances::projection(&config["settings"], &config, true);
    assert_eq!(
        view["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "codex")
            .unwrap()["currentVersion"],
        "v1"
    );
    set(0);
    let before = polls.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(polls.load(Ordering::SeqCst), before);
    set(1000);
    let end = Instant::now() + Duration::from_secs(4);
    while polls.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    // A real activity builder pauses ongoing probes until the client clears low-power mode.
    let mut activity=t3_protocol::api::server::ClientActivity::from_json(&json!({"clientId":"reader","clientKind":"web","visible":true,"focused":true,"recentlyInteracted":true,"scopes":[{"type":"provider-status"}],"observedAt":"2026-11-01T00:00:00Z","lowPowerMode":"true","ttlMs":10000})).unwrap();
    t3_protocol::api::server::report_client_activity(&c, &activity).unwrap();
    let before = polls.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1400));
    assert_eq!(polls.load(Ordering::SeqCst), before);
    activity.low_power_mode = Some("false".into());
    t3_protocol::api::server::report_client_activity(&c, &activity).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while polls.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    }
}
