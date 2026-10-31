//! Common transcript operations shared by provider drivers.
pub(crate) use crate::claude_projection::save_item;
pub(crate) fn finish(
    db: &rusqlite::Connection,
    tid: &str,
    rid: &str,
    thread: &serde_json::Value,
    at: &str,
    status: &str,
) -> Result<(), String> {
    if thread["providerInstanceId"] == "codex" {
        crate::codex_projection::finish(db, tid, rid, thread, &serde_json::json!(status), at)
    } else {
        crate::claude_projection::finish(db, tid, rid, thread, at, status)
    }
}
