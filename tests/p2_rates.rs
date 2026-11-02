mod support;
use std::io::{Read, Write};
use t3_protocol::api::diagnostics;

#[test]
fn refresh_rates_persists_real_http_document_and_reports_stale_on_failure() {
    let s = support::start(|_| {});
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rates", listener.local_addr().unwrap());
    std::env::set_var("O3_USAGE_RATES_URL", &url);
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        let body = r#"{"sample-model":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000005},"sample_spec":{"input_cost_per_token":"bad"}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let c = s.connect();
    let rates = diagnostics::refresh_usage_rates(&c).unwrap();
    assert_eq!(rates["status"], "fresh", "{rates}");
    assert_eq!(rates["knownModels"], 1);
    peer.join().unwrap();
    let query = diagnostics::UsageQuery {
        since_day: "2026-11-02".into(),
        until_day: "2026-11-02".into(),
        time_zone: "UTC".into(),
        ..Default::default()
    };
    let summary = diagnostics::usage_summary(&c, &query).unwrap();
    assert_eq!(summary["pricing"]["knownModels"], 1);
    assert!(summary["pricing"]["fetchedAt"].is_string());
    let failed = diagnostics::refresh_usage_rates(&c).unwrap();
    assert_eq!(failed["status"], "stale");
    assert_eq!(failed["knownModels"], 1);
    assert_eq!(failed["fetchedAt"], rates["fetchedAt"]);
    assert!(s.dir.join("usage-rates.json").is_file());
    std::env::remove_var("O3_USAGE_RATES_URL");
    let state = o3_server::store::State::open(&s.dir).unwrap();
    assert_eq!(o3_server::usage_rates::pricing(&state)["knownModels"], 1);
}
