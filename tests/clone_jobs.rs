mod common;
mod support;
use serde_json::json;
use std::time::Duration;
use t3_protocol::api::project_clone::{self, CloneStart};
#[test]
fn tracked_local_clone_real_start_reader_and_connection() {
    let marker = std::env::temp_dir().join(format!("o3-clone-smudge-{}", std::process::id()));
    std::env::set_var("GIT_CONFIG_COUNT", "3");
    std::env::set_var("GIT_CONFIG_KEY_0", "filter.block.smudge");
    std::env::set_var(
        "GIT_CONFIG_VALUE_0",
        format!("echo $$ > {}; sleep 30; cat", marker.display()),
    );
    std::env::set_var("GIT_CONFIG_KEY_1", "filter.block.required");
    std::env::set_var("GIT_CONFIG_VALUE_1", "true");
    std::env::set_var("GIT_CONFIG_KEY_2", "filter.block.clean");
    std::env::set_var("GIT_CONFIG_VALUE_2", "cat");
    let s = support::start(|_| {});
    let c = s.connect();
    let config = t3_protocol::api::server::config(&c).unwrap();
    assert_eq!(
        config["environment"]["capabilities"]["projectCloneTracking"],
        true
    );
    let origin = s.dir.join("origin");
    std::fs::create_dir(&origin).unwrap();
    common::repo(&origin);
    common::write(&origin.join("file"), "data");
    common::commit_all(&origin, "seed");
    let destination = s.dir.join("clones/copy");
    let request = CloneStart {
        project_id: "clone".into(),
        title: "Copy".into(),
        created_at: "2026-11-02T00:00:00Z".into(),
        destination_path: destination.to_string_lossy().into(),
        provider: None,
        repository: None,
        remote_url: Some(origin.to_string_lossy().into()),
        protocol: None,
    };
    let mut unsupported = request.clone();
    unsupported.project_id = "unsupported".into();
    unsupported.provider = Some("gitlab".into());
    unsupported.remote_url = None;
    unsupported.repository = Some("fixture/repo".into());
    assert!(project_clone::start(&c, &unsupported).is_err());
    let result = project_clone::start(&c, &request).unwrap();
    assert_eq!(result["projectId"], "clone");
    let mut stream = support::subscribe(s.connect(), "subscribeProjectClones", json!({}));
    loop {
        let jobs = stream.next(Duration::from_secs(10));
        if jobs[0]["phase"] == "done" {
            break;
        }
        assert_eq!(jobs[0]["phase"], "running", "{jobs}");
    }
    assert_eq!(
        std::fs::read_to_string(destination.join("file")).unwrap(),
        "data"
    );
    common::write(&origin.join(".gitattributes"), "file filter=block\n");
    common::commit_all(&origin, "blocking checkout");
    let mut slow = request.clone();
    slow.project_id = "slow".into();
    slow.destination_path = s.dir.join("clones/slow").to_string_lossy().into();
    project_clone::start(&c, &slow).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || marker.exists()));
    let pid = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        c.rpc("projectClone.cancel", json!({"projectId":"slow"}))
            .unwrap()["applied"],
        true
    );
    loop {
        let jobs = stream.next(Duration::from_secs(10));
        if jobs
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["projectId"] == "slow" && j["phase"] == "cancelled")
        {
            break;
        }
    }
    assert!(!std::path::Path::new(&slow.destination_path).exists());
    assert!(support::wait_for(Duration::from_secs(3), || {
        !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists()
    }));
    let _ = std::fs::remove_file(marker);
    let mut bad = request;
    bad.project_id = "bad".into();
    bad.destination_path = s.dir.join("clones/bad").to_string_lossy().into();
    bad.remote_url = Some(s.dir.join("missing").to_string_lossy().into());
    project_clone::start(&c, &bad).unwrap();
    loop {
        let jobs = stream.next(Duration::from_secs(10));
        if jobs
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["projectId"] == "bad" && j["phase"] == "failed")
        {
            break;
        }
    }
    assert!(!std::path::Path::new(&bad.destination_path).exists());
}
