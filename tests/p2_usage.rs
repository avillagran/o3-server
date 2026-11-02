mod support;
use serde_json::json;
use t3_protocol::{api::diagnostics, usage_summary};

#[test]
fn ledger_usage_reaches_production_aggregate() {
    let s = support::start(|s| {
        let db = s.db.lock().unwrap();
        for (id, driver, input, cached, output, reasoning, cost) in [
            ("c", "claudeAgent", 100, 20, 30, 0, Some(0.25)),
            ("x", "codex", 100, 20, 30, 10, None),
        ] {
            db.execute("INSERT INTO records VALUES(?, ?, ?, ?)", rusqlite::params!["providerTurns", id, "t", json!({"id":id,"threadId":"t","driver":driver,"model":"fixture","providerThreadId":id,"startedAt":"2026-11-02T01:00:00Z","tokenUsage":{"inputTokens":input,"cachedInputTokens":cached,"outputTokens":output,"reasoningOutputTokens":reasoning,"cost":cost,"updatedAt":"2026-11-02T01:01:00Z"}}).to_string()]).unwrap();
        }
    });
    let c = s.connect();
    let query = diagnostics::UsageQuery {
        since_day: "2026-11-01".into(),
        until_day: "2026-11-01".into(),
        time_zone: "America/Santiago".into(),
        ..Default::default()
    };
    let raw = diagnostics::usage_summary(&c, &query).unwrap();
    let day = "2026-11-01".parse().unwrap();
    let a = usage_summary::aggregate(&raw, day, day);
    assert_eq!(a["tokens"], 280, "{raw}");
    assert_eq!(a["cost"], 0.25);
    assert_eq!(a["unpricedRecords"], 1);
    assert_eq!(a["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn long_history_scan_has_bounded_aggregation_and_reports_cost() {
    let s = support::start(|s| {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        for n in 0..50_000 {
            tx.execute("INSERT INTO records VALUES('providerTurns',?,'t',?)", rusqlite::params![n.to_string(), json!({"id":n,"driver":"codex","model":"fixture","providerThreadId":"one-session","startedAt":"2026-11-02T01:00:00Z","tokenUsage":{"inputTokens":100,"outputTokens":50}}).to_string()]).unwrap();
        }
        tx.commit().unwrap();
    });
    let c = s.connect();
    let query = diagnostics::UsageQuery {
        since_day: "2026-11-02".into(),
        until_day: "2026-11-02".into(),
        time_zone: "UTC".into(),
        ..Default::default()
    };
    let at = std::time::Instant::now();
    let raw = diagnostics::usage_summary(&c, &query).unwrap();
    eprintln!(
        "50000-turn usage: {:?}, scan={}ms, response={} bytes",
        at.elapsed(),
        raw["scanDurationMs"],
        raw.to_string().len()
    );
    assert_eq!(raw["buckets"].as_array().unwrap().len(), 1);
    assert_eq!(raw["buckets"][0]["records"], 50000);
    assert_eq!(raw["buckets"][0]["sessions"], 1);
    assert!(at.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn narrow_query_ignores_history_outside_exact_local_day() {
    let s = support::start(|s| {
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        for n in 0..100_001 {
            tx.execute(
                r#"INSERT INTO records VALUES('providerTurns',?,'t','{"driver":"codex","startedAt":"2026-11-01T12:00:00Z","tokenUsage":{"inputTokens":1}}')"#,
                [n.to_string()],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    });
    let q = diagnostics::UsageQuery {
        since_day: "2026-11-02".into(),
        until_day: "2026-11-02".into(),
        time_zone: "UTC".into(),
        ..Default::default()
    };
    let now = std::time::Instant::now();
    let raw = diagnostics::usage_summary(&s.connect(), &q).unwrap();
    assert!(raw["buckets"].as_array().unwrap().is_empty());
    assert!(now.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn cached_token_rates_reach_real_reader_savings() {
    let s = support::start(|s| {
        std::fs::write(s.dir.join("usage-rates.json"), json!({"document":{"fixture":{"input_cost_per_token":0.01,"output_cost_per_token":0.02,"cache_read_input_token_cost":0.001}}}).to_string()).unwrap();
        s.db.lock().unwrap().execute("INSERT INTO records VALUES('providerTurns','cached','t',?)", [json!({"driver":"codex","model":"fixture","startedAt":"2026-11-02T00:00:00Z","tokenUsage":{"inputTokens":100,"cachedInputTokens":20,"outputTokens":0}}).to_string()]).unwrap();
    });
    let q = diagnostics::UsageQuery {
        since_day: "2026-11-02".into(),
        until_day: "2026-11-02".into(),
        time_zone: "UTC".into(),
        ..Default::default()
    };
    let raw = diagnostics::usage_summary(&s.connect(), &q).unwrap();
    let day = "2026-11-02".parse().unwrap();
    let result = usage_summary::aggregate(&raw, day, day);
    assert!(
        (result["cacheSavings"].as_f64().unwrap() - 0.18).abs() < 1e-10,
        "{result}"
    );
}
