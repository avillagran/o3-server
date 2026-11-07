//! Pre-bind Tailscale enforcement with bounded argv-only commands and redacted errors.
use crate::{
    managed_policy::Policy,
    managed_tailscale::{self, ServeRoute, Target},
};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

const STATUS_TIMEOUT: Duration = Duration::from_millis(3500);
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Cli {
    program: PathBuf,
    environment: Vec<(String, String)>,
    status_timeout: Duration,
}
impl Default for Cli {
    fn default() -> Self {
        Self {
            program: if cfg!(windows) {
                "tailscale.exe"
            } else {
                "tailscale"
            }
            .into(),
            environment: Vec::new(),
            status_timeout: STATUS_TIMEOUT,
        }
    }
}
#[derive(Debug)]
enum Failure {
    Missing,
    Inactive,
    Timeout,
    Unavailable,
}
struct Group(u32);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
async fn read_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, Failure> {
    let mut output = Vec::new();
    reader
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|_| Failure::Unavailable)?;
    if output.len() as u64 > OUTPUT_LIMIT {
        return Err(Failure::Unavailable);
    }
    Ok(output)
}
fn inactive(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    [
        "not logged in",
        "logged out",
        "need login",
        "needs login",
        "doesn't appear to be running",
        "tailscaled.service not running",
        "tailscale is stopped",
        "tailscaled: the system cannot find the file specified",
    ]
    .iter()
    .any(|label| lower.contains(label))
        || lower
            .find("failed to connect to local tailscale")
            .is_some_and(|index| {
                [
                    "connection refused",
                    "no such file or directory",
                    "cannot find the file",
                ]
                .iter()
                .any(|label| lower[index..].contains(label))
            })
}
impl Cli {
    async fn run(&self, args: &[String], timeout: Duration) -> Result<String, Failure> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(args)
            .envs(self.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ENOTDIR)
            {
                Failure::Missing
            } else {
                Failure::Unavailable
            }
        })?;
        let group = Group(child.id().ok_or(Failure::Unavailable)?);
        let stdout = child.stdout.take().ok_or(Failure::Unavailable)?;
        let stderr = child.stderr.take().ok_or(Failure::Unavailable)?;
        let work = async {
            let (stdout, stderr, status) =
                tokio::join!(read_output(stdout), read_output(stderr), child.wait());
            (stdout, stderr, status)
        };
        let outcome = tokio::time::timeout(timeout, work).await;
        let result = match outcome {
            Ok((stdout, stderr, status)) => {
                let stdout = stdout?;
                let stderr = stderr?;
                if status.map_err(|_| Failure::Unavailable)?.success() {
                    Ok(String::from_utf8_lossy(&stdout).into_owned())
                } else if inactive(&String::from_utf8_lossy(&stderr)) {
                    Err(Failure::Inactive)
                } else {
                    Err(Failure::Unavailable)
                }
            }
            Err(_) => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(group.0 as i32), libc::SIGTERM);
                }
                if tokio::time::timeout(Duration::from_millis(500), child.wait())
                    .await
                    .is_err()
                {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(group.0 as i32), libc::SIGKILL);
                    }
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
                Err(Failure::Timeout)
            }
        };
        drop(group);
        result
    }
    async fn routes(&self, target: &Target) -> Result<Vec<ServeRoute>, Failure> {
        match self
            .run(
                &["serve".into(), "status".into(), "--json".into()],
                self.status_timeout,
            )
            .await
        {
            Ok(raw) => {
                managed_tailscale::parse_serve_routes(&raw, target).ok_or(Failure::Unavailable)
            }
            Err(Failure::Missing | Failure::Inactive) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}
fn refusal(policy: &Policy, detail: &str) -> String {
    let organization = policy
        .document
        .as_ref()
        .map(|document| document.organization.as_str())
        .unwrap_or("Your organization");
    format!("{organization}'s policy does not allow remote clients, {detail}.")
}
fn read_failure(policy: &Policy, error: Failure) -> String {
    refusal(
        policy,
        if matches!(error, Failure::Timeout) {
            "so the server does not start until Tailscale Serve can be checked, and Tailscale did not answer; open Tailscale and finish setting it up, or remove it"
        } else {
            "so the server does not start until Tailscale Serve can be checked"
        },
    )
}
fn former_name(route: &ServeRoute, current: Option<&str>) -> bool {
    route.owner.is_none()
        && route
            .host
            .as_deref()
            .zip(current)
            .is_some_and(|(host, current)| host.to_lowercase() != current)
}
fn describe(route: &ServeRoute, current: Option<&str>) -> String {
    let port = ryu_js::Buffer::new()
        .format_finite(route.serve_port)
        .to_owned();
    let location = format!(
        "port {port}{} here",
        route
            .mount
            .as_ref()
            .map(|mount| format!(" at {mount}"))
            .unwrap_or_default()
    );
    let Some(off) = managed_tailscale::off_args(route) else {
        return format!("{location}; stop the \"tailscale serve\" command that runs it");
    };
    if former_name(route, current) {
        return format!("{location} under {}, a former name of this machine, which \"tailscale serve ... off\" cannot reach; run \"tailscale serve reset\" and set up your other routes again",route.host.as_deref().unwrap());
    }
    let command = format!("\"tailscale {}\"", off.join(" "));
    if current.is_none() && route.owner.is_none() && route.host.is_some() {
        return format!("{location} under {}; if that is this machine's current name, turn it off with {command}",route.host.as_deref().unwrap());
    }
    match &route.ambiguous {Some(target)=>format!("{location}, and its target {target} might be another service on this port; check it, then turn it off with {command}"),None=>format!("{location}; turn it off with {command}")}
}
/// Inspect and close only the Tailscale routes the managed policy disallows.
/// This must finish before the server binds its real listener.
pub async fn enforce(policy: &Policy, target: &Target) -> Result<(), String> {
    close_forwarding(policy, target, &Cli::default()).await
}
/// The real listener must bind only after this check returns successfully.
pub(crate) async fn close_forwarding(
    policy: &Policy,
    target: &Target,
    cli: &Cli,
) -> Result<(), String> {
    if policy.allows_remote() {
        return Ok(());
    }
    let probe=tokio::net::TcpListener::bind((target.host.as_str(),target.port)).await.map_err(|_|refusal(policy,&format!("so the server does not start while it cannot bind {}:{}; if another app holds that port, the server would take over its Tailscale Serve routes",target.host,target.port)))?;
    drop(probe);
    let found = cli
        .routes(target)
        .await
        .map_err(|error| read_failure(policy, error))?;
    if found.is_empty() {
        return Ok(());
    }
    let current = if found
        .iter()
        .any(|route| route.mount.is_some() && route.owner.is_none())
    {
        cli.run(&["status".into(), "--json".into()], cli.status_timeout)
            .await
            .ok()
            .and_then(|raw| managed_tailscale::parse_status(&raw))
            .and_then(|status| status["magicDnsName"].as_str().map(str::to_lowercase))
    } else {
        None
    };
    for route in found.iter().filter(|route| {
        route.owner.is_none()
            && route.ambiguous.is_none()
            && (route.mount.is_none()
                || (current.is_some() && !former_name(route, current.as_deref())))
    }) {
        if let Some(args) = managed_tailscale::off_args(route) {
            let _ = cli.run(&args, SERVE_TIMEOUT).await;
        }
    }
    let left = cli
        .routes(target)
        .await
        .map_err(|error| read_failure(policy, error))?;
    if left.is_empty() {
        Ok(())
    } else {
        Err(refusal(
            policy,
            &format!(
                "so the server does not start while Tailscale Serve forwards {}",
                left.iter()
                    .map(|route| describe(route, current.as_deref()))
                    .collect::<Vec<_>>()
                    .join(", and ")
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::os::unix::fs::PermissionsExt;
    static TEST_PORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    fn fixture(state: Value) -> (std::path::PathBuf, Cli) {
        crate::test_sandbox::assert_isolated();
        let dir = std::env::temp_dir().join(format!(
            "tailscale-guard-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&dir).unwrap();
        let program = dir.join("tailscale");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/tailscale.py"),
            &program,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("fixture.json");
        std::fs::write(&path, state.to_string()).unwrap();
        (
            path.clone(),
            Cli {
                program,
                environment: vec![("O3_TAILSCALE_FIXTURE".into(), path.to_str().unwrap().into())],
                status_timeout: Duration::from_secs(2),
            },
        )
    }
    fn policy() -> Policy {
        Policy::parse(std::path::Path::new("/fixture/policy.json"),r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"clients":["local-browser"]}}"#).unwrap()
    }
    fn target() -> Target {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        Target {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
        }
    }
    fn calls(path: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(path.with_extension("calls"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    #[tokio::test]
    async fn closes_only_unambiguous_owned_routes_and_rechecks_before_binding() {
        let _ports = TEST_PORTS.lock().await;
        let target = target();
        let proxy = format!("127.0.0.1:{}", target.port);
        let (path, cli) = fixture(
            json!({"serve":{"TCP":{"443":{"TCPForward":proxy},"8443":{"TCPForward":"127.0.0.1:1"}},"Web":{"node.tail.example:443":{"Handlers":{"/ours":{"Proxy":proxy},"/other":{"Proxy":"127.0.0.1:1"}}}},"Services":{"svc:other":{"TCP":{"443":{"TCPForward":"127.0.0.1:1"}}}}},"status":{"Self":{"DNSName":"node.tail.example."}}}),
        );
        close_forwarding(&policy(), &target, &cli).await.unwrap();
        assert_eq!(
            calls(&path),
            vec![
                json!(["serve", "status", "--json"]),
                json!(["status", "--json"]),
                json!(["serve", "--tcp=443", "off"]),
                json!(["serve", "--https=443", "--set-path=/ours", "off"]),
                json!(["serve", "status", "--json"])
            ]
        );
        let stored: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(stored["serve"]["TCP"].get("8443").is_some());
        assert!(stored["serve"]["Web"]["node.tail.example:443"]["Handlers"]
            .get("/other")
            .is_some());
        assert!(stored["serve"]["Services"].get("svc:other").is_some());
    }
    #[tokio::test]
    async fn refuses_shared_foreground_ambiguous_and_former_name_routes_without_mutating_them() {
        let _ports = TEST_PORTS.lock().await;
        let target = target();
        let proxy = format!("127.0.0.1:{}", target.port);
        for serve in [
            json!({"Services":{"svc:sample":{"TCP":{"443":{"TCPForward":proxy}}}}}),
            json!({"Foreground":{"session":{"TCP":{"443":{"TCPForward":proxy}}}}}),
            json!({"TCP":{"443":{"TCPForward":format!("localhost:{}",target.port)}}}),
            json!({"Web":{"old.tail.example:443":{"Handlers":{"/ours":{"Proxy":proxy}}}}}),
        ] {
            let (path, cli) =
                fixture(json!({"serve":serve,"status":{"Self":{"DNSName":"node.tail.example."}}}));
            let before = std::fs::read(&path).unwrap();
            let error = close_forwarding(&policy(), &target, &cli)
                .await
                .unwrap_err();
            assert!(error.contains("Fixture Corp"));
            assert!(error.contains("still") || error.contains("while Tailscale Serve forwards"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!calls(&path).iter().any(|args| args
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "off")));
        }
    }
    #[tokio::test]
    async fn unavailable_status_and_surviving_routes_fail_closed_without_exposing_stderr() {
        let _ports = TEST_PORTS.lock().await;
        let target = target();
        for state in [
            json!({"raw":"{malformed"}),
            json!({"exit":1,"stderr":"permission denied tskey-private-fixture"}),
            json!({"serve":{"TCP":{"443":{"TCPForward":format!("127.0.0.1:{}",target.port)}}},"offPersists":true}),
        ] {
            let (_, cli) = fixture(state);
            let error = close_forwarding(&policy(), &target, &cli)
                .await
                .unwrap_err();
            assert!(error.contains("Fixture Corp"));
            assert!(!error.contains("tskey-private-fixture"));
        }
        for stderr in [
            "not logged in",
            "tailscale is stopped",
            "failed to connect to local tailscale: connection refused",
        ] {
            let (_, cli) = fixture(json!({"exit":1,"stderr":stderr}));
            close_forwarding(&policy(), &target, &cli).await.unwrap();
        }
    }
    #[tokio::test]
    async fn occupied_port_is_refused_before_any_cli_action_and_missing_cli_is_allowed() {
        let _ports = TEST_PORTS.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied = Target {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
        };
        let (path, cli) = fixture(json!({"serve":null}));
        let error = close_forwarding(&policy(), &occupied, &cli)
            .await
            .unwrap_err();
        assert!(error.contains("another app holds that port"));
        assert!(!path.with_extension("calls").exists());
        drop(listener);
        let target = target();
        let missing = Cli {
            program: path.with_file_name("missing-cli"),
            ..cli.clone()
        };
        close_forwarding(&policy(), &target, &missing)
            .await
            .unwrap();
        std::fs::set_permissions(&cli.program, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(close_forwarding(&policy(), &target, &cli).await.is_err());
    }
    #[tokio::test]
    async fn timed_out_cli_and_its_detached_descendant_are_stopped() {
        let _ports = TEST_PORTS.lock().await;
        let target = target();
        let (path, mut cli) = fixture(json!({"sleep":true,"fork":true}));
        cli.status_timeout = Duration::from_millis(100);
        let error = close_forwarding(&policy(), &target, &cli)
            .await
            .unwrap_err();
        assert!(error.contains("Tailscale did not answer"));
        let pid = std::fs::read_to_string(path.with_extension("descendant")).unwrap();
        let stat = std::path::PathBuf::from(format!("/proc/{}/stat", pid.trim()));
        for _ in 0..20 {
            if std::fs::read_to_string(&stat)
                .map_or(true, |stat| stat.split_whitespace().nth(2) == Some("Z"))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("Synthetic descendant still running");
    }
    #[tokio::test]
    #[ignore = "Official nightly in a private policy/Tailscale fixture container only"]
    async fn official_prebind_tailscale_decisions_match_synthetic_runtime() {
        use std::os::unix::process::CommandExt;
        let _ports = TEST_PORTS.lock().await;
        assert_eq!(
            std::env::var("O3_POLICY_FIXED_PATH_CONTAINER").as_deref(),
            Ok("1")
        );
        assert!(std::path::Path::new("/run/.containerenv").is_file());
        std::fs::write("/etc/t3code/policy.json",r#"{"schemaVersion":1,"organization":"Fixture Corp","limits":{"clients":["local-browser"]}}"#).unwrap();
        let binary = std::env::var("T3_REFERENCE_BIN").unwrap();
        let version = std::process::Command::new(&binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).contains("20261106.4262"));
        for scenario in [
            "remove-owned",
            "service-held",
            "ambiguous-held",
            "off-survives",
            "malformed",
            "inactive",
            "permission",
        ] {
            let target = target();
            let proxy = format!("127.0.0.1:{}", target.port);
            let state = match scenario {
                "remove-owned" => {
                    json!({"serve":{"Web":{"node.tail.example:443":{"Handlers":{"/ours":{"Proxy":proxy},"/other":{"Proxy":"127.0.0.1:1"}}}}},"status":{"Self":{"DNSName":"node.tail.example."}}})
                }
                "service-held" => {
                    json!({"serve":{"Services":{"svc:sample":{"TCP":{"443":{"TCPForward":proxy}}}}}})
                }
                "ambiguous-held" => {
                    json!({"serve":{"TCP":{"443":{"TCPForward":format!("localhost:{}",target.port)}}}})
                }
                "off-survives" => {
                    json!({"serve":{"TCP":{"443":{"TCPForward":proxy}}},"offPersists":true})
                }
                "malformed" => json!({"raw":"{malformed"}),
                "inactive" => json!({"exit":1,"stderr":"tailscale is stopped"}),
                "permission" => {
                    json!({"exit":1,"stderr":"permission denied tskey-private-fixture"})
                }
                _ => unreachable!(),
            };
            let (_, ours_cli) = fixture(state.clone());
            let ours = close_forwarding(&policy(), &target, &ours_cli).await;
            let (path, native_cli) = fixture(state);
            let directory = path.parent().unwrap();
            let profile = directory.join("profile");
            let log = directory.join("native.log");
            let output = std::fs::File::create(&log).unwrap();
            let executable_path =
                format!("{}:{}", directory.display(), std::env::var("PATH").unwrap());
            let mut child = std::process::Command::new(&binary)
                .env("T3CODE_HOME", profile)
                .env("O3_TAILSCALE_FIXTURE", &path)
                .env("PATH", executable_path)
                .args([
                    "serve",
                    "--host",
                    &target.host,
                    "--port",
                    &target.port.to_string(),
                ])
                .stdin(Stdio::null())
                .stdout(output.try_clone().unwrap())
                .stderr(output)
                .process_group(0)
                .spawn()
                .unwrap();
            let group = Group(child.id());
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let mut listened = false;
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                if std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], target.port)),
                    Duration::from_millis(50),
                )
                .is_ok()
                {
                    listened = true;
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Native startup timed out: {scenario}"
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            assert_eq!(
                listened,
                ours.is_ok(),
                "native pre-bind decision: {scenario}"
            );
            if let Err(error) = ours {
                let log = std::fs::read_to_string(&log).unwrap();
                assert!(
                    log.contains(&error),
                    "Native refusal differs for {scenario}: expected {error}"
                );
                assert!(!log.contains("tskey-private-fixture"));
            }
            drop(group);
            let _ = child.wait();
            assert_eq!(
                calls(&path),
                calls(&std::path::PathBuf::from(ours_cli.environment[0].1.clone())),
                "native selective calls: {scenario}"
            );
            assert!(native_cli.program.is_file());
        }
        println!("Official native pre-bind decisions and selective CLI calls verified for seven scenarios; no inference");
    }
}
