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
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"echo setup-ready; sleep 30","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
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
        launch.rpc("orchestration.launchThread",json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Local","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}}))
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
    assert!(task.join().unwrap().is_err());
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
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"pu2","projectId":"p","scripts":[{"id":"setup","name":"Setup","command":"echo finished > setup-result","icon":"terminal","runOnWorktreeCreate":true,"async":false}]})).unwrap();
    c.rpc("orchestration.launchThread",json!({"commandId":"retry","threadId":"t","projectId":"p","title":"Local","modelSelection":{"instanceId":"scripted","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":"HEAD"}})).unwrap();
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&path).join("setup-result")).unwrap(),
        "finished\n"
    );
    done.store(true, Ordering::Release);
    reader.join().unwrap().unwrap();
}
