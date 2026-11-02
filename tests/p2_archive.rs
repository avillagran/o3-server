mod support;
use serde_json::json;
use t3_protocol::api::orchestration;
#[test]
fn oversized_archive_returns_actionable_error_inside_reader_budget() {
    let s=support::start(|s|{
        let mut db=s.db.lock().unwrap();let tx=db.transaction().unwrap();
        let title="x".repeat(9000);
        for n in 0..1000 {let id=format!("a{n}");let t=json!({"id":id,"projectId":"p","title":title,"archivedAt":"2026-11-02T00:00:00Z","deletedAt":null});tx.execute("INSERT INTO records VALUES('thread',?1,?1,?2)",rusqlite::params![id,t.to_string()]).unwrap();}tx.commit().unwrap();
    });
    let err=orchestration::archived_shell(&s.connect()).unwrap_err().to_string();
    assert!(err.contains("archive exceeds"),"{err}");
}
