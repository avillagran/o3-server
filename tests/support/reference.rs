//! Credential-free official headless reference, restricted to the container runner.
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct Reference {
    child: Child,
    raw: super::support::RawClient,
}
impl Drop for Reference {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

impl Reference {
    pub fn raw_exit(&self, tag: &str, payload: serde_json::Value) -> serde_json::Value {
        self.raw.exit(tag, payload)
    }
}

pub fn start(root: &Path) -> (t3_protocol::Connection, Reference) {
    use std::os::unix::process::CommandExt;
    assert_eq!(
        std::env::var("O3_HEADLESS_CONTAINER").as_deref(),
        Ok("1"),
        "Use tools/test-headless-reference"
    );
    let binary = std::env::var("T3_REFERENCE_BIN").expect("T3_REFERENCE_BIN");
    let profile = root.join("reference");
    let mut version = Command::new(&binary);
    let version = version.arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("20261106.4262"),
        "The proof is pinned to nightly 4262"
    );
    let auth = Command::new(&binary)
        .env("T3CODE_HOME", &profile)
        .args([
            "auth",
            "session",
            "issue",
            "--subject",
            "headless-reference",
            "--ttl",
            "1h",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(
        auth.status.success(),
        "Reference auth failed: {}",
        String::from_utf8_lossy(&auth.stderr)
    );
    let token = String::from_utf8(auth.stdout).unwrap().trim().to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let log = std::fs::File::create(root.join("reference.log")).unwrap();
    let process = Command::new(binary)
        .env("T3CODE_HOME", &profile)
        .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .process_group(0)
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let mut process = Reference {
        child: process,
        raw: super::support::RawClient::bearer(&base, &token),
    };
    let deadline = Instant::now() + Duration::from_secs(90);
    while t3_protocol::fetch_descriptor(&base).is_err() {
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "Reference exited; inspect reference.log"
        );
        assert!(Instant::now() < deadline, "Reference did not listen");
        std::thread::sleep(Duration::from_millis(100));
    }
    super::support::signals::shield_client_thread_from_sigchld();
    let client = t3_protocol::Connection::new(
        &base,
        None,
        t3_protocol::Credential::bearer(&token).unwrap(),
    )
    .unwrap();
    (client, process)
}
