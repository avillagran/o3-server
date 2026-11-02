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

#[test]
fn long_latest_messages_fit_real_http_and_shell_readers() {
    let s = support::start(|_| {});
    let c = s.connect();
    {
        let mut db = s.state.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        for n in 0..375 {
            let id = format!("long{n}");
            let t = json!({"id":id,"projectId":"p","title":"Scale","createdAt":"2026-11-02T00:00:00Z","updatedAt":"2026-11-02T00:00:00Z","archivedAt":null,"deletedAt":null});
            tx.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES ('thread',?1,?1,?2)",
                [&id, &t.to_string()],
            )
            .unwrap();
            let m = json!({"id":id,"role":"user","createdAt":"2026-11-02T00:00:00Z","text":"界".repeat(32768),"updatedAt":"2026-11-02T00:00:00Z"});
            tx.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES ('messages',?1,?1,?2)",
                [&id, &m.to_string()],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(shell["threads"].as_array().unwrap().len(), 375);
    assert!(shell["threads"][0]["latestVisibleMessage"].is_null());
    let mut stream = support::subscribe(s.connect(), "orchestration.subscribeShell", json!({}));
    let frame = stream.next(std::time::Duration::from_secs(10));
    assert_eq!(frame["snapshot"]["threads"].as_array().unwrap().len(), 375);
    s.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.text',?1) WHERE kind='messages' AND id='long0'", ["x".repeat(9 * 1024 * 1024)]).unwrap();
    s.state.notify(); // Direct fixture writes must wake the WAL checkpointer.
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.metadata.update","commandId":"incremental","threadId":"long0","title":"Incremental proof"})).unwrap();
    let update = stream.next(std::time::Duration::from_secs(10));
    assert_eq!(update["kind"], "thread.updated");
    assert!(update["thread"]["latestVisibleMessage"].is_null());
    assert!(serde_json::to_vec(&update).unwrap().len() < 8192);
    eprintln!(
        "bounded shell bytes={}",
        serde_json::to_vec(&shell).unwrap().len()
    );
}

#[test]
fn archive_does_not_build_active_rows() {
    let s = support::start(|_| {});
    let c = s.connect();
    let db = s.state.db.lock().unwrap();
    db.execute(
        "INSERT INTO records(kind,id,thread_id,body) VALUES ('thread','active','active','{}')",
        [],
    )
    .unwrap();
    drop(db);
    let archive = orchestration::archived_shell(&c).unwrap();
    assert!(archive["threads"].as_array().unwrap().is_empty());
}
