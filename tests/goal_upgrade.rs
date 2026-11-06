use o3_server::store::State;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "upgrade continuation timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn record(root: &Path, kind: &str, id: &str) -> Value {
    let db = rusqlite::Connection::open_with_flags(
        root.join("userdata/o3.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let body: String = db
        .query_row(
            "SELECT body FROM records WHERE kind=? AND id=?",
            [kind, id],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&body).unwrap()
}
fn spawn(root: &Path) -> Server {
    Server(
        Command::new(env!("CARGO_BIN_EXE_o3-server"))
            .args(["serve", "--no-bridge", "--port", "0", "--base-dir"])
            .arg(root)
            .env("HOME", root)
            .env("CODEX_HOME", root.join("native-home"))
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
            .unwrap(),
    )
}

#[test]
fn server_restart_resumes_a_legacy_active_goal_without_replacing_the_goal_or_app_run() {
    let root = std::env::temp_dir().join(format!(
        "o3-goal-upgrade-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(root.join("bin")).unwrap();
    // Copy the peer into this private fixture instead of linking into a real tree.
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goal-peer.py"),
        root.join("bin/codex"),
    )
    .unwrap();
    let state = State::open(&root.join("userdata")).unwrap();
    {
        let db = state.db.lock().unwrap();
        let put = |kind: &str, id: &str, body: Value| {
            db.execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,'t',?3)",
                rusqlite::params![kind, id, body.to_string()],
            )
            .unwrap();
        };
        put(
            "thread",
            "t",
            json!({"id":"t","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"goal-fixture"},"runtimeMode":"full-access","worktreePath":root,"activeProviderThreadId":"p","codexNativeThreadId":"native-root","deletedAt":null}),
        );
        put(
            "providerThreads",
            "p",
            json!({"id":"p","appThreadId":"t","driver":"codex","providerInstanceId":"codex","status":"active","nativeThreadRef":{"driver":"codex","nativeId":"native-root","strength":"strong"}}),
        );
        put(
            "runs",
            "r",
            json!({"id":"r","threadId":"t","status":"running","ordinal":1,"providerInstanceId":"codex","providerThreadId":"p"}),
        );
    }
    drop(state);
    let goal = json!({"objective":"Preserve work across this update","status":"active","tokensUsed":73,"timeUsedSeconds":17,"tokenBudget":12345});
    std::fs::write(
        root.join("resume-goal.json"),
        json!({"goal":goal,"initialTurns":1}).to_string(),
    )
    .unwrap();
    let mut first = spawn(&root);
    wait(|| {
        record(&root, "providerThreads", "p")["goal"]["status"] == "active"
            && record(&root, "runs", "r")["status"] == "running"
    });
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    let previous_binding = record(&root, "providerThreads", "p")["updatedAt"].clone();
    std::fs::write(
        root.join("resume-goal.json"),
        json!({"goal":goal,"initialTurns":2}).to_string(),
    )
    .unwrap();
    let _second = spawn(&root);
    wait(|| {
        record(&root, "providerThreads", "p")["updatedAt"] != previous_binding
            && record(&root, "providerThreads", "p")["goal"]["status"] == "active"
            && record(&root, "runs", "r")["status"] == "running"
    });
    let observed = record(&root, "providerThreads", "p")["goal"].clone();
    assert_eq!(observed["objective"], goal["objective"]);
    assert_eq!(observed["tokenBudget"], 12345);
    assert_eq!(observed["tokensUsed"], 73);
    std::fs::write(root.join("next"), "").unwrap();
    wait(|| record(&root, "runs", "r")["status"] == "completed");
    assert_eq!(
        record(&root, "providerThreads", "p")["goal"]["status"],
        "complete"
    );
    let db = rusqlite::Connection::open_with_flags(
        root.join("userdata/o3.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM records WHERE kind='runs'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let trace = std::fs::read_to_string(root.join("goal-trace.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        trace
            .iter()
            .filter(|entry| entry["method"] == "turn/start")
            .count(),
        2
    );
    assert!(
        !trace.iter().any(|entry| entry["method"]
            .as_str()
            .is_some_and(|method| method.starts_with("thread/goal/"))),
        "upgrade must not replace or reset the persisted native goal"
    );
}
