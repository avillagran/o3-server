#[path = "support/sandbox.rs"]
mod test_sandbox;
use o3_server::proc::{self, Opts, RunError};
use std::{path::Path, time::Duration};
fn opts(ms: u64) -> Opts {
    Opts {
        timeout: Duration::from_millis(ms),
        cap: 1024,
        allow_truncate: false,
    }
}
#[tokio::test]
async fn input_is_bounded_closed_and_not_reported_in_failures() {
    test_sandbox::assert_isolated();
    let out = proc::run_with_input(
        "/usr/bin/python3",
        Path::new("/"),
        &[
            "-c",
            "import sys; d=sys.stdin.buffer.read(); assert d==b'private-input'; print('received')",
        ],
        opts(2000),
        &[],
        b"private-input",
    )
    .await
    .unwrap();
    assert!(out.ok());
    assert_eq!(out.text().trim(), "received");
    let error = proc::run_with_input(
        "/nonexistent-fixture-program",
        Path::new("/"),
        &[],
        opts(2000),
        &[],
        b"private-input",
    )
    .await
    .unwrap_err();
    assert_eq!(error.spawn_kind, Some(std::io::ErrorKind::NotFound));
    assert!(!format!("{error:?}").contains("private-input"));
    let error = proc::run_with_input(
        "/usr/bin/python3",
        Path::new("/"),
        &["-c", "raise Exception('must not execute')"],
        opts(2000),
        &[],
        &vec![b'x'; 65537],
    )
    .await
    .unwrap_err();
    assert!(format!("{error:?}").contains("exceeds limit"));
}
#[tokio::test]
async fn blocked_input_and_closed_stdin_remain_bounded() {
    test_sandbox::assert_isolated();
    let error = proc::run_with_input(
        "/usr/bin/python3",
        Path::new("/"),
        &["-c", "import time; time.sleep(30)"],
        opts(50),
        &[],
        &vec![b'x'; 65536],
    )
    .await
    .unwrap_err();
    assert!(matches!(error.error, RunError::Timeout(_)));
    let error = proc::run_with_input(
        "/usr/bin/python3",
        Path::new("/"),
        &["-c", "import os,time; os.close(0); time.sleep(30)"],
        opts(2000),
        &[],
        &vec![b'x'; 65536],
    )
    .await
    .unwrap_err();
    // A 64 KiB kernel pipe may accept the entire bounded input before the
    // child closes it; either EPIPE or the deadline is the correct outcome.
    assert!(matches!(
        error.error,
        RunError::Spawn(_) | RunError::Timeout(_)
    ));
    assert!(!format!("{error:?}").contains(&"x".repeat(20)));
}

#[tokio::test]
async fn timeout_and_future_cancellation_stop_the_owned_process_group() {
    test_sandbox::assert_isolated();
    let directory = std::env::temp_dir().join(format!(
        "o3-input-process-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&directory).unwrap();
    // Existence is the readiness signal, so publish only the closed, complete
    // PID file. The private staging name remains in the same owned fixture.
    let code = "import os,sys,time\nwith open(sys.argv[1]+'.pending','x') as ready:\n ready.write(str(os.getpid()))\nos.rename(sys.argv[1]+'.pending',sys.argv[1])\ntime.sleep(30)";
    let pid_path = directory.join("timeout.pid");
    let error = proc::run_with_input(
        "/usr/bin/python3",
        &directory,
        &["-c", code, pid_path.to_str().unwrap()],
        opts(200),
        &[],
        &vec![b'x'; 65536],
    )
    .await
    .unwrap_err();
    assert!(matches!(error.error, RunError::Timeout(_)));
    let pid: i32 = std::fs::read_to_string(pid_path).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "timeout child was not reaped"
    );
    let pid_path = directory.join("cancel.pid");
    let child_path = pid_path.clone();
    let cwd = directory.clone();
    let task = tokio::spawn(async move {
        proc::run_with_input(
            "/usr/bin/python3",
            &cwd,
            &["-c", code, child_path.to_str().unwrap()],
            opts(30000),
            &[],
            &vec![b'x'; 65536],
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !pid_path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(pid_path).unwrap().parse().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled child still exists");
    // Exact owned files and their containing fixture directory remain for container teardown.
}

#[tokio::test]
async fn pipe_buffers_preserve_output_caps_and_null_stdin() {
    test_sandbox::assert_isolated();
    for allow_truncate in [false, true] {
        let out=proc::run("/usr/bin/python3",Path::new("/"),&["-c","import os,sys; assert sys.stdin.buffer.read()==b''; os.write(1,b'x'*20000); os.write(2,b'fixture-stderr')"],Opts{timeout:Duration::from_secs(2),cap:17000,allow_truncate},&[]).await.unwrap();
        assert_eq!(out.stdout, vec![b'x'; 17000]);
        assert_eq!(out.stderr, "fixture-stderr");
        assert!(out.truncated);
        assert_eq!(out.ok(), allow_truncate);
    }
}
