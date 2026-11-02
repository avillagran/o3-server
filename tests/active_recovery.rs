use o3_server::store::State;
mod events {
    use serde_json::Value;
    pub fn put(
        db: &rusqlite::Connection,
        k: &str,
        id: &str,
        t: &str,
        v: &Value,
    ) -> rusqlite::Result<usize> {
        db.execute(
            "INSERT OR REPLACE INTO records(kind,id,thread_id,body) VALUES(?,?,?,?)",
            rusqlite::params![k, id, t, v.to_string()],
        )
    }
    pub fn get(db: &rusqlite::Connection, k: &str, id: &str) -> Result<Value, String> {
        let s: String = db
            .query_row(
                "SELECT body FROM records WHERE kind=? AND id=?",
                [k, id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&s).map_err(|e| e.to_string())
    }
}
use serde_json::json;
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl std::ops::Deref for Server {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "recovery timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}
#[test]
fn sigkill_active_native_sessions_continue_after_restart() {
    for instance in ["claudeAgent", "codex"] {
        let root = std::env::temp_dir().join(format!(
            "o3-kill-{}",
            o3_server::store::random_id().unwrap()
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        for name in ["claude", "codex"] {
            std::os::unix::fs::symlink(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/recovery-peer.py"
                ),
                root.join("bin").join(name),
            )
            .unwrap();
        }
        let state = State::open(&root.join("userdata")).unwrap();
        let mut thread = json!({"id":"t","providerInstanceId":instance,"modelSelection":{"instanceId":instance,"model":"haiku"},"runtimeMode":"full-access","worktreePath":root});
        thread[if instance == "codex" {
            "codexNativeThreadId"
        } else {
            "activeProviderThreadId"
        }] = json!("native");
        let db = state.db.lock().unwrap();
        events::put(&db, "thread", "t", "t", &thread).unwrap();
        events::put(
            &db,
            "runs",
            "r",
            "t",
            &json!({"id":"r","threadId":"t","status":"running"}),
        )
        .unwrap();
        events::put(
            &db,
            "runtimeRequests",
            "old",
            "t",
            &json!({"id":"old","status":"pending"}),
        )
        .unwrap();
        drop(db);
        drop(state);
        let spawn = || {
            Command::new(env!("CARGO_BIN_EXE_o3-server"))
                .args(["--port", "0"])
                .env("T3CODE_HOME", &root)
                .env("HOME", &root)
                .env("CODEX_HOME", &root)
                .env("RECOVERY_MARKER", root.join("active"))
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        root.join("bin").display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut child = Server(spawn());
        wait(|| root.join("active").exists());
        child.kill().unwrap();
        child.wait().unwrap();
        let mut child = Server(spawn());
        let state = State::open(&root.join("userdata")).unwrap();
        wait(|| {
            events::get(&state.db.lock().unwrap(), "runs", "r").unwrap()["status"] == "completed"
        });
        let db = state.db.lock().unwrap();
        let old = events::get(&db, "runtimeRequests", "old").unwrap();
        assert_eq!(old["status"], "cancelled");
        assert!(old["resolutionReason"]
            .as_str()
            .unwrap()
            .contains("server-restarted"));
        assert!(db
            .query_row(
                "SELECT json_extract(body,'$.recoveryReason') FROM run_internal WHERE id='r'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap()
            .contains("server restarted"));
        child.kill().unwrap();
        child.wait().unwrap();
        drop(db);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
