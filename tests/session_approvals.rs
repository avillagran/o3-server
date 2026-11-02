mod support;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use t3_protocol::api::orchestration::{self, Command, MessageDispatch, ThreadCreate};
fn wait(c: &t3_protocol::Connection, f: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let p = orchestration::thread_projection(c, "t").unwrap();
        if f(&p) {
            return p;
        }
        assert!(Instant::now() < end, "{p}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn reader_accept_for_session_survives_actual_idle_reap_and_retains_operation_scope() {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/input/session-approval.py");
    let provider = Arc::new(
        o3_server::claude::ClaudeProvider::new(binary).with_idle_timeout(Duration::from_millis(50)),
    );
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(provider));
    let c = s.connect();
    let workspace = s.dir.join("repo");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Sessions","workspaceRoot":workspace})).unwrap();
    orchestration::dispatch(
        &c,
        &Command::ThreadCreate(ThreadCreate {
            command_id: "create".into(),
            thread_id: "t".into(),
            project_id: "p".into(),
            title: "Session".into(),
            model_selection: json!({"instanceId":"claudeAgent","model":"test"}),
            runtime_mode: "approval-required".into(),
            interaction_mode: "default".into(),
            branch: None,
            worktree_path: None,
            created_by: "user".into(),
            creation_source: "web".into(),
            imported_native_thread: None,
        }),
    )
    .unwrap();
    for (i, text) in [(0, "echo safe"), (1, "echo safe"), (2, "echo changed")] {
        orchestration::dispatch(
            &c,
            &Command::MessageDispatch(MessageDispatch::new(
                &format!("send{i}"),
                "t",
                &format!("m{i}"),
                text,
                "web",
            )),
        )
        .unwrap();
        if i != 1 {
            let p = wait(&c, |p| {
                !t3_protocol::approvals::pending(p)
                    .as_array()
                    .unwrap()
                    .is_empty()
            });
            let pending = t3_protocol::approvals::pending(&p);
            let cmd=Command::approve(&format!("approve{i}"),&json!({"thread":"t","runtimeRequestId":pending[0]["requestId"],"decision":if i==0{"acceptForSession"}else{"accept"}}),&p).unwrap();
            orchestration::dispatch(&c, &cmd).unwrap();
        }
        let p = wait(&c, |p| {
            p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "completed")
        });
        assert_eq!(t3_protocol::approvals::pending(&p), json!([]));
        while !s.state.active.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(workspace.join("starts"))
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        let end = Instant::now() + Duration::from_secs(5);
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            assert!(Instant::now() < end, "idle child not reaped");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("starts"))
            .unwrap()
            .lines()
            .count(),
        3
    );
}

#[test]
fn stop_cancels_approval_blocked_native_run_and_request() {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/input/session-approval.py");
    let provider = Arc::new(
        o3_server::claude::ClaudeProvider::new(binary).with_idle_timeout(Duration::from_millis(50)),
    );
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(provider));
    let c = s.connect();
    let workspace = s.dir.join("repo");
    std::fs::create_dir_all(&workspace).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Sessions","workspaceRoot":workspace})).unwrap();
    orchestration::dispatch(
        &c,
        &Command::ThreadCreate(ThreadCreate {
            command_id: "create".into(),
            thread_id: "t".into(),
            project_id: "p".into(),
            title: "Session".into(),
            model_selection: json!({"instanceId":"claudeAgent","model":"test"}),
            runtime_mode: "approval-required".into(),
            interaction_mode: "default".into(),
            branch: None,
            worktree_path: None,
            created_by: "user".into(),
            creation_source: "web".into(),
            imported_native_thread: None,
        }),
    )
    .unwrap();

    orchestration::dispatch(
        &c,
        &Command::MessageDispatch(MessageDispatch::new("send", "t", "m", "echo safe", "web")),
    )
    .unwrap();
    let p = wait(&c, |p| {
        !t3_protocol::approvals::pending(p)
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert_eq!(p["runs"][0]["status"], "waiting");
    assert_eq!(
        t3_protocol::approvals::pending(&p)[0]["responseCapability"],
        "live"
    );
    c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"thread.stop","commandId":"stop","threadId":"t"}),
    )
    .unwrap();
    let p = wait(&c, |p| p["runs"][0]["status"] == "interrupted");
    assert_eq!(t3_protocol::approvals::pending(&p), json!([]));
    assert!(p["runtimeRequests"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["status"] != "pending"));
    assert!(p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "run_interrupt_result"));
}
