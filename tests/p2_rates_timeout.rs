//! A slow price source must not outlive the client's read timeout. Own binary: it sets the URL.
mod support;
use std::time::{Duration, Instant};
use t3_protocol::api::diagnostics;

#[test]
fn slow_rate_source_answers_before_the_client_gives_up() {
    let s = support::start(|_| {});
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::env::set_var(
        "O3_USAGE_RATES_URL",
        format!("http://{}/rates", listener.local_addr().unwrap()),
    );
    let (a, b) = (s.connect(), s.connect());
    let started = Instant::now();
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| diagnostics::refresh_usage_rates(&a));
        std::thread::sleep(Duration::from_millis(200));
        let second = scope.spawn(|| diagnostics::refresh_usage_rates(&b));
        (first.join().unwrap(), second.join().unwrap())
    });
    std::env::remove_var("O3_USAGE_RATES_URL");
    drop(listener);
    assert_ne!(first.unwrap()["status"], "fresh");
    assert_ne!(second.unwrap()["status"], "fresh");
    assert!(started.elapsed() < Duration::from_secs(11));
}
