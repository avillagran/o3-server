mod support;
use serde_json::json;
use t3_protocol::api::orchestration;

#[test]
fn shell_keeps_all_live_records_after_ten_thousand_newer_deleted_records() {
    let s = support::start(|_| {});
    let c = s.connect();
    {
        let mut db = s.state.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        for n in 0..20003 {
            let archived = n == 0;
            let deleted = n >= 10002;
            let t = json!({"id":format!("t{n}"),"projectId":"p","title":"Scale","createdAt":"2026-11-02T00:00:00Z","updatedAt":"2026-11-02T00:00:00Z","archivedAt":if archived {json!("2026-11-02T00:00:00Z")} else {json!(null)},"deletedAt":if deleted {json!("2026-11-02T00:00:00Z")} else {json!(null)}});
            tx.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES ('thread',?1,?1,?2)",
                [format!("t{n}"), t.to_string()],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let start = std::time::Instant::now();
    let shell = o3_server::events::shell(&s.state).unwrap();
    assert_eq!(shell["threads"].as_array().unwrap().len(), 10001);
    assert_eq!(shell["archivedThreads"].as_array().unwrap().len(), 1);
    let archive = orchestration::archived_shell(&c).unwrap();
    assert_eq!(archive["threads"][0]["id"], "t0");
    let live = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(live["threads"].as_array().unwrap().len(), 10001);
    eprintln!(
        "large-profile snapshots elapsed_ms={} {}",
        start.elapsed().as_millis(),
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("VmHWM:") || l.starts_with("VmRSS:"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
