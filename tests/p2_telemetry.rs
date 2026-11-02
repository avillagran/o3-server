mod support;
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};
use t3_protocol::api::diagnostics;

struct Busy(Child);
impl Drop for Busy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn history_cpu_includes_busy_provider_children() {
    let s = support::start(|_| {});
    let c = s.connect();
    // A deterministic child stands in for a busy provider. The in-process server
    // has this test process as its root, so this is part of its process tree.
    let _busy = Busy(
        Command::new("sh")
            .args(["-c", include_str!("fixtures/telemetry-busy.sh")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    diagnostics::telemetry_history(&c, 60_000, 60_000).unwrap();
    // Keep a real RPC subscription open so the on-demand sampler stays active.
    let _stream = support::subscribe(
        c.clone(),
        "server.getStorageCleanupReport",
        serde_json::json!({}),
    );
    std::thread::sleep(Duration::from_secs(12));
    let h = diagnostics::telemetry_history(&c, 60_000, 60_000).unwrap();
    let cpu = h["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["avgCpuPercent"].as_f64())
        .fold(0.0, f64::max);
    assert!(
        cpu > 5.0,
        "the client CPU graph must include child CPU: {h}"
    );
}
