//! Exercise launcher credentials through the actual binary and authenticated HTTP boundary.
#[path = "support/sandbox.rs"]
mod sandbox;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
impl Server {
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "owned process did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn stop(&mut self) {
        // SAFETY: this PID is the directly owned child, retained until it is reaped.
        assert_eq!(unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(Instant::now() < deadline, "owned server did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn directory() -> PathBuf {
    sandbox::assert_isolated();
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).unwrap();
    let path = std::env::temp_dir().join(format!(
        "desktop-startup-{:032x}",
        u128::from_ne_bytes(bytes)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
fn envelope(home: &Path) -> Value {
    json!({"mode":"desktop","noBrowser":true,"port":3773,"host":"127.0.0.1","desktopBootstrapToken":"private-launcher-seed","tailscaleServeEnabled":false,"tailscaleServePort":443,"t3Home":home})
}
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_o3-server"));
    command
        .args(["serve", "--no-bridge", "--port", "0"])
        .env("PATH", "/usr/bin:/bin")
        .env_remove("T3CODE_HOME")
        .env_remove("T3CODE_HOST")
        .env_remove("T3CODE_PORT")
        .env_remove("T3CODE_MODE")
        .env_remove("T3CODE_BOOTSTRAP_FD")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}
fn start(mut command: Command, value: &Value) -> Server {
    command.args(["--bootstrap-fd", "0"]).stdin(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{value}").unwrap();
    drop(stdin);
    Server(child)
}
fn ready(server: &mut Server, home: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(bytes) = std::fs::read(home.join("userdata/server-runtime.json")) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                return value;
            }
        }
        if let Some(status) = server.0.try_wait().unwrap() {
            let mut error = String::new();
            if let Some(mut stderr) = server.0.stderr.take() {
                stderr.read_to_string(&mut error).unwrap();
            }
            panic!("owned server exited {status}: {error}");
        }
        assert!(
            Instant::now() < deadline,
            "owned server did not become ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn request(runtime: &Value, path: &str, body: Option<&str>) -> (u16, Value) {
    let mut socket =
        std::net::TcpStream::connect(("127.0.0.1", runtime["port"].as_u64().unwrap() as u16))
            .unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let data = body.unwrap_or("");
    let method = if body.is_some() { "POST" } else { "GET" };
    write!(socket,"{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{data}",data.len()).unwrap();
    let mut text = String::new();
    socket.read_to_string(&mut text).unwrap();
    let (headers, body) = text.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}

#[test]
fn inherited_envelope_authenticates_in_desktop_and_explicit_web_modes() {
    for web in [false, true] {
        let root = directory();
        let home = root.join("bootstrap-home");
        let explicit = root.join("explicit-home");
        let value = envelope(&home);
        let mut command = command();
        if web {
            command.args(["--mode", "web", "--base-dir"]).arg(&explicit);
        }
        let selected = if web { &explicit } else { &home };
        let mut server = start(command, &value);
        let runtime = ready(&mut server, selected);
        let (status, descriptor) = request(&runtime, "/api/auth/session", None);
        assert_eq!(status, 200);
        assert_eq!(
            descriptor["auth"]["policy"],
            if web {
                "loopback-browser"
            } else {
                "desktop-managed-local"
            }
        );
        let (status, first) = request(
            &runtime,
            "/oauth/token",
            Some("subject_token=private-launcher-seed"),
        );
        assert_eq!(status, 200);
        let (status, second) = request(
            &runtime,
            "/oauth/token",
            Some("subject_token=private-launcher-seed"),
        );
        assert_eq!(status, 200);
        assert_ne!(first["access_token"], second["access_token"]);
        assert_eq!(
            request(&runtime, "/oauth/token", Some("subject_token=wrong")).0,
            401
        );
        if web {
            assert!(!home.exists());
        }
        server.stop();
        assert!(!selected.join("userdata/server-runtime.json").exists());
    }
}

#[test]
fn unsupported_launcher_channels_fail_before_creating_profile() {
    let root = directory();
    let home = root.join("uncreated-home");
    let mut value = envelope(&home);
    value["desktopBrowserFd"] = json!(10);
    let mut server = start(command(), &value);
    let status = server.wait();
    assert!(!status.success());
    let mut error = String::new();
    server
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(error.contains("desktopBrowserFd"));
    assert!(!error.contains("private-launcher-seed"));
    assert!(!home.exists());
}

#[test]
fn incomplete_bootstrap_options_fail_without_profile_effects() {
    for flag in ["--bootstrap-fd", "--mode"] {
        let root = directory();
        let home = root.join("uncreated-home");
        let mut command = command();
        command
            .arg("--base-dir")
            .arg(&home)
            .arg(flag)
            .stdin(Stdio::null());
        let mut server = Server(command.spawn().unwrap());
        assert!(!server.wait().success());
        let mut error = String::new();
        server
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        assert!(error.contains(flag));
        assert!(!home.exists());
    }
}

#[test]
fn a_closed_launcher_log_pipe_does_not_end_the_owned_server() {
    let root = directory();
    let home = root.join("home");
    let mut command = command();
    command.args(["--bootstrap-fd", "0"]).stdin(Stdio::piped());
    let mut server = Server(command.spawn().unwrap());
    // Close logging before providing the envelope, so startup cannot win this race.
    drop(server.0.stderr.take());
    let mut input = server.0.stdin.take().unwrap();
    writeln!(input, "{}", envelope(&home)).unwrap();
    drop(input);
    let runtime = ready(&mut server, &home);
    let (status, session) = request(&runtime, "/api/auth/session", None);
    assert_eq!(status, 200);
    assert_eq!(session["auth"]["policy"], "desktop-managed-local");
    assert!(server.0.try_wait().unwrap().is_none());
    server.stop();
}

#[cfg(target_os = "linux")]
#[test]
fn parent_replacement_stops_the_launcher_owned_server_gracefully() {
    use std::os::fd::{AsRawFd, FromRawFd};
    struct OwnedPid(std::os::fd::OwnedFd);
    impl Drop for OwnedPid {
        fn drop(&mut self) {
            // SAFETY: a pidfd identifies this exact private fixture process, even after exit.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.0.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
    }
    let root = directory();
    let mut launcher = Server(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/input/desktop-bootstrap-launcher.py"
            ))
            .arg(env!("CARGO_BIN_EXE_o3-server"))
            .arg(&root)
            .env("PATH", "/usr/bin:/bin")
            .env_remove("T3CODE_HOME")
            .env_remove("T3CODE_MODE")
            .env_remove("T3CODE_PORT")
            .env_remove("T3CODE_HOST")
            .env_remove("T3CODE_BOOTSTRAP_FD")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(root.join("child-pid")) {
            break text.parse::<i32>().unwrap();
        }
        assert!(
            launcher.0.try_wait().unwrap().is_none(),
            "private launcher exited early"
        );
        assert!(
            Instant::now() < deadline,
            "private launcher did not report readiness"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: open a stable process handle for the fixture's reported live child.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(fd >= 0, "pidfd_open: {}", std::io::Error::last_os_error());
    // SAFETY: the syscall returned a new descriptor owned by this guard.
    let owned = OwnedPid(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) });
    // SIGTERM is directed at the directly owned launcher, which deliberately has no handler.
    assert_eq!(
        unsafe { libc::kill(launcher.0.id() as i32, libc::SIGTERM) },
        0
    );
    assert!(!launcher.0.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    let runtime = root.join("home/userdata/server-runtime.json");
    while runtime.exists() {
        assert!(
            Instant::now() < deadline,
            "server did not observe its changed parent"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut poll = libc::pollfd {
        fd: owned.0.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll receives one initialized process descriptor; wait for actual process exit.
    assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1);
}
