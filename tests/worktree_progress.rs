mod common;
mod support;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
#[test]
fn real_setup_reader_progress_cancel_and_rollback() {
    let s = support::start(|_| {});
    let root = s.dir.join("origin");
    std::fs::create_dir_all(&root).unwrap();
    common::repo(&root);
    common::write(&root.join("file"), "data");
    common::commit_all(&root, "seed");
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Local","workspaceRoot":root,"scripts":[]})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"setsid sh -c 'echo $$ > escaped-pid; echo setup-ready; sleep 120' & wait","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let conn = s.connect();
    let done = Arc::new(AtomicBool::new(false));
    let stop = done.clone();
    let reader = std::thread::spawn(move || {
        t3_protocol::api::streams::subscribe_worktree_setup(
            &conn,
            "t",
            || stop.load(Ordering::Acquire),
            || false,
            |v| {
                if let Some(v) = v {
                    tx.send(v.clone()).unwrap();
                }
                Ok(())
            },
        )
    });
    let launch = s.connect();
    let task = std::thread::spawn(move || {
        launch.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Local","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}}))
    });
    let end = std::time::Instant::now() + Duration::from_secs(12);
    let path = loop {
        assert!(
            std::time::Instant::now() < end,
            "missing real setup progress"
        );
        let v = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        if v["stages"].as_array().is_some_and(|a| {
            a.iter()
                .any(|v| v["tail"].to_string().contains("setup-ready"))
        }) {
            assert_eq!(v["phase"], "running");
            break v["worktreePath"].as_str().unwrap().to_owned();
        }
    };
    assert_eq!(
        t3_protocol::api::worktree_setup::cancel(&c, "t").unwrap()["cancelled"],
        true
    );
    let pid: i32 = std::fs::read_to_string(std::path::Path::new(&path).join("escaped-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(4);
    while !task.is_finished() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bounded = task.is_finished();
    let live =
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "));
    // Cleanup only this probe's isolated descendant even on the failing baseline.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    assert!(task.join().unwrap().is_err());
    assert!(
        bounded,
        "setup cancellation blocked on escaped descendant pipes"
    );
    assert!(
        !std::path::Path::new(&path).exists(),
        "cancelled worktree remains"
    );
    loop {
        let v = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if v["phase"] == "cancelled" {
            break;
        }
    }
    assert!(!live, "escaped setup descendant survived cancellation");
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu2","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"echo finished > setup-result","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
    c.rpc("orchestration.launchThread",json!({"commandId":"retry","threadId":"t","projectId":"p","title":"Local","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}})).unwrap();
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&path).join("setup-result")).unwrap(),
        "finished\n"
    );
    done.store(true, Ordering::Release);
    reader.join().unwrap().unwrap();
}

#[test]
#[ignore = "subprocess worker for the restart regression"]
fn setup_restart_worker() {
    let dir = std::path::PathBuf::from(std::env::var_os("O3_RESTART_TEST_DIR").unwrap());
    let s = support::start_at(dir, |_| {});
    let root = s.dir.join("origin");
    std::fs::create_dir_all(&root).unwrap();
    common::repo(&root);
    common::write(&root.join("file"), "data");
    common::commit_all(&root, "seed");
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"pc","projectId":"p","title":"Local","workspaceRoot":root,"scripts":[]})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"echo invocation >> invocations; setsid sh -c 'echo $$ > escaped-pid; sleep 120' & wait","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
    let _ = c.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"restart","projectId":"p","title":"Local","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}}));
}

#[test]
fn restart_stops_orphan_reports_failure_and_reruns_setup() {
    let dir = std::env::temp_dir().join(format!("o3-setup-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut worker = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "setup_restart_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("O3_RESTART_TEST_DIR", &dir)
        .spawn()
        .unwrap();
    let path = dir.join("worktrees").join(
        "restart"
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    );
    assert!(support::wait_for(Duration::from_secs(10), || path
        .join("escaped-pid")
        .exists()));
    let pid: i32 = std::fs::read_to_string(path.join("escaped-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    worker.kill().unwrap();
    worker.wait().unwrap();
    let s = support::start_at(dir, |_| {});
    let mut stream = support::subscribe(
        s.connect(),
        "subscribeWorktreeSetup",
        json!({"threadId":"restart"}),
    );
    let snapshot = stream.next(Duration::from_secs(3));
    let live =
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "));
    // Baseline cleanup is scoped to this test's own session.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    assert!(!live, "escaped setup survived restart");
    assert_eq!(snapshot["phase"], "failed", "{snapshot}");
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu2","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"echo invocation >> invocations; echo recovered > setup-result","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
    c.rpc("orchestration.launchThread",json!({"commandId":"retry","threadId":"restart","projectId":"p","title":"Local","runtimeMode":"full-access","interactionMode":"default","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}})).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("invocations"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(path.join("setup-result")).unwrap(),
        "recovered\n"
    );
}
