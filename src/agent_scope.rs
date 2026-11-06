//! Native agent resource isolation and per-profile orphan cleanup.
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{process::Command, sync::OnceCell};

const AGENT_SLICE: &str = "app-t3code-agents.slice";
const PARENT_SLICE: &str = "app-t3code.slice";
const SCOPE_OPTIONS: [&str; 2] = ["--property=TimeoutStopSec=10s", "--property=Delegate=yes"];

pub fn owned_agent_slice(state_dir: &Path) -> String {
    let digest = Sha256::digest(state_dir.to_string_lossy().as_bytes());
    let hash: String = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("app-t3code-agents-{hash}.slice")
}

pub fn memory_limits(total: u64) -> (u64, u64) {
    const GIB: u64 = 1024 * 1024 * 1024;
    (
        total.saturating_sub(6 * GIB).max(total / 2),
        total
            .saturating_sub(4 * GIB)
            .max(total / 4 * 3 + total % 4 * 3 / 4),
    )
}

#[derive(Clone, Debug, Default)]
struct ScopeState {
    load: String,
    active: String,
    result: String,
    group: String,
    oom_kills: Option<u64>,
    populated: Option<bool>,
}

#[derive(Debug, PartialEq)]
enum Status {
    OomKilled,
    Gone,
    Running,
    Stopping,
}

fn classify(state: &ScopeState) -> Status {
    if state.result == "oom-kill" {
        Status::OomKilled
    } else if state.load != "loaded" || matches!(state.active.as_str(), "inactive" | "failed") {
        Status::Gone
    } else if state.active == "deactivating"
        || state.oom_kills.unwrap_or(0) > 0
        || state.populated == Some(false)
    {
        Status::Stopping
    } else {
        Status::Running
    }
}

fn values(text: &str) -> BTreeMap<&str, &str> {
    text.lines()
        .filter_map(|line| {
            line.split_once('=')
                .filter(|(key, _)| !key.is_empty())
                .map(|(key, value)| (key, value.trim()))
        })
        .collect()
}

fn counter(text: &str, key: &str) -> Option<u64> {
    text.split('\n').find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(' ')?;
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    })
}

#[derive(Clone)]
struct Backend {
    run: PathBuf,
    ctl: PathBuf,
    runtime: OsString,
    oom_policy: bool,
}

#[derive(Clone)]
struct Tracked {
    unit: String,
    shared: bool,
    oom_killed: bool,
}

struct ScopeOptions {
    owned: bool,
    continuing: bool,
    survives_restart: bool,
}

pub(crate) struct TerminalCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    pub adjust_oom: bool,
}
impl TerminalCommand {
    pub(crate) fn plain(program: &str, env: &[(String, String)]) -> Self {
        Self {
            program: PathBuf::from(program),
            args: vec![],
            env: env.to_vec(),
            adjust_oom: false,
        }
    }
}

/// One registry is shared by every provider instance in a server profile.
/// It is disabled for embedded hosts until explicitly enabled.
pub struct Registry {
    enabled: AtomicBool,
    owned_slice: String,
    backend: OnceCell<Option<Backend>>,
    tracked: Mutex<VecDeque<(String, Tracked)>>,
    cgroup_root: PathBuf,
    host: BTreeMap<OsString, OsString>,
}

impl Registry {
    pub fn new(state_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            owned_slice: owned_agent_slice(state_dir),
            backend: OnceCell::new(),
            tracked: Mutex::new(VecDeque::new()),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            host: std::env::vars_os().collect(),
        })
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub(crate) async fn initialize(&self) {
        self.backend().await;
    }

    fn scoped_command(
        &self,
        backend: &Backend,
        unit: &str,
        program: &Path,
        args: impl IntoIterator<Item = OsString>,
        options: ScopeOptions,
    ) -> Command {
        let mut command = Command::new(&backend.run);
        command
            .args(["--user", "--scope", "--quiet"])
            .arg(format!(
                "--slice={}",
                if options.survives_restart {
                    AGENT_SLICE
                } else {
                    &self.owned_slice
                }
            ))
            .arg(format!("--unit={unit}"))
            .args(SCOPE_OPTIONS);
        if backend.oom_policy {
            command.arg(format!(
                "--property=OOMPolicy={}",
                if options.continuing {
                    "continue"
                } else {
                    "stop"
                }
            ));
        }
        if !options.owned {
            command.arg("--collect");
        }
        command.arg("--").arg(program).args(args);
        command
    }

    /// Terminal callers initialize the backend before taking synchronous PTY locks.
    /// The full sanitized environment is explicit, rather than inherited again.
    pub(crate) fn terminal_command(
        &self,
        program: &str,
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<TerminalCommand, String> {
        let mut plan = TerminalCommand::plain(program, env);
        if !cfg!(target_os = "linux") || !self.enabled.load(Ordering::Acquire) {
            return Ok(plan);
        }
        let environment = env
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();
        let cwd = if cwd.is_absolute() {
            cwd.to_owned()
        } else {
            std::env::current_dir()
                .map_err(|error| error.to_string())?
                .join(cwd)
        };
        let Some(resolved) = resolve(Path::new(program), &cwd, &environment) else {
            return Ok(plan);
        };
        plan.program = resolved;
        plan.adjust_oom = true;
        if let Some(backend) = self.backend.get().and_then(Option::as_ref) {
            let unit = format!("t3code-terminal-{}.scope", &crate::store::random_id()?[..8]);
            let command = self.scoped_command(
                backend,
                &unit,
                &plan.program,
                vec![],
                ScopeOptions {
                    owned: false,
                    continuing: false,
                    survives_restart: true,
                },
            );
            plan.program = PathBuf::from(command.as_std().get_program());
            plan.args = command.as_std().get_args().map(ToOwned::to_owned).collect();
            if environment
                .get(std::ffi::OsStr::new("XDG_RUNTIME_DIR"))
                .is_none_or(|value| value.is_empty())
            {
                plan.env.retain(|(key, _)| key != "XDG_RUNTIME_DIR");
                plan.env.push((
                    "XDG_RUNTIME_DIR".into(),
                    backend.runtime.to_string_lossy().into_owned(),
                ));
            }
        }
        // Unlike native sessions, terminal scopes may outlive a server restart.
        // They are collecting scopes, without a lease that stops them on drop.
        Ok(plan)
    }

    async fn backend(&self) -> Option<&Backend> {
        if !cfg!(target_os = "linux") || !self.enabled.load(Ordering::Acquire) {
            return None;
        }
        self.backend.get_or_init(|| self.probe()).await.as_ref()
    }

    async fn run(program: &Path, args: &[&str], timeout: Duration) -> Option<std::process::Output> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        tokio::time::timeout(timeout, command.output())
            .await
            .ok()?
            .ok()
    }

    async fn ctl(backend: &Backend, args: &[&str]) -> Option<std::process::Output> {
        Self::run(&backend.ctl, args, Duration::from_secs(5)).await
    }

    fn group_file(&self, group: &str, file: &str) -> Option<PathBuf> {
        if group.is_empty() {
            return None;
        }
        let relative = Path::new(group.trim_start_matches('/'));
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return None;
        }
        Some(self.cgroup_root.join(relative).join(file))
    }

    async fn read_group(&self, group: &str, file: &str) -> String {
        match self.group_file(group, file) {
            Some(path) => tokio::fs::read_to_string(path).await.unwrap_or_default(),
            None => String::new(),
        }
    }

    async fn probe(&self) -> Option<Backend> {
        let runtime = self
            .host
            .get(std::ffi::OsStr::new("XDG_RUNTIME_DIR"))
            .filter(|value| !value.is_empty())?
            .clone();
        let cwd = std::env::current_dir().ok()?;
        let run = resolve(Path::new("systemd-run"), &cwd, &self.host)?;
        let ctl = resolve(Path::new("systemctl"), &cwd, &self.host)?;
        let mut backend = Backend {
            run,
            ctl,
            runtime,
            oom_policy: true,
        };
        if let Some(shown) = Self::ctl(
            &backend,
            &[
                "--user",
                "show",
                &self.owned_slice,
                "--property=ActiveState,ControlGroup",
            ],
        )
        .await
        {
            let text = String::from_utf8_lossy(&shown.stdout);
            let properties = values(&text);
            if properties.get("ActiveState") == Some(&"active") {
                let group = properties.get("ControlGroup").copied().unwrap_or("");
                if counter(&self.read_group(group, "cgroup.events").await, "populated") != Some(0) {
                    eprintln!(
                        "Stopping agents left by a previous server: {}",
                        self.owned_slice
                    );
                    let stopped = Self::run(
                        &backend.ctl,
                        &["--user", "stop", &self.owned_slice],
                        Duration::from_secs(30),
                    )
                    .await;
                    if !stopped.is_some_and(|output| output.status.success()) {
                        eprintln!(
                            "Could not stop agents left by a previous server: {}",
                            self.owned_slice
                        );
                    }
                }
            }
        }
        let probe = |oom_policy| {
            let mut args = vec![
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=app-t3code-agents.slice",
            ];
            args.extend(SCOPE_OPTIONS);
            if oom_policy {
                args.push("--property=OOMPolicy=stop");
            }
            args.extend(["--", "true"]);
            args
        };
        if !Self::run(&backend.run, &probe(true), Duration::from_secs(5))
            .await
            .is_some_and(|output| output.status.success())
        {
            backend.oom_policy = false;
            if !Self::run(&backend.run, &probe(false), Duration::from_secs(5))
                .await
                .is_some_and(|output| output.status.success())
            {
                eprintln!("Agent scopes are off: no systemd user manager");
                return None;
            }
        }
        #[cfg(target_os = "linux")]
        let total = unsafe {
            let mut info: libc::sysinfo = std::mem::zeroed();
            if libc::sysinfo(&mut info) == 0 {
                (u128::from(info.totalram) * u128::from(info.mem_unit)).min(u128::from(u64::MAX))
                    as u64
            } else {
                0
            }
        };
        #[cfg(not(target_os = "linux"))]
        let total = 0;
        let (high, max) = memory_limits(total);
        let high = format!("MemoryHigh={high}");
        let max = format!("MemoryMax={max}");
        for args in [
            vec![
                "--user",
                "set-property",
                "--runtime",
                AGENT_SLICE,
                &high,
                &max,
            ],
            vec![
                "--user",
                "set-property",
                "--runtime",
                PARENT_SLICE,
                "CPUWeight=10",
                "IOWeight=10",
                "TasksMax=25%",
            ],
        ] {
            if !Self::ctl(&backend, &args)
                .await
                .is_some_and(|output| output.status.success())
            {
                eprintln!("Could not set resource limits on agents slice {}", args[3]);
            }
        }
        Some(backend)
    }

    async fn read_scope(&self, backend: &Backend, unit: &str) -> Option<ScopeState> {
        let output = Self::ctl(
            backend,
            &[
                "--user",
                "show",
                unit,
                "--property=LoadState,ActiveState,Result,ControlGroup",
            ],
        )
        .await?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let properties = values(&text);
        let property = |key| properties.get(key).copied().unwrap_or("").to_owned();
        let group = property("ControlGroup");
        let memory = self.read_group(&group, "memory.events").await;
        let events = self.read_group(&group, "cgroup.events").await;
        Some(ScopeState {
            load: property("LoadState"),
            active: property("ActiveState"),
            result: property("Result"),
            group,
            oom_kills: counter(&memory, "oom_kill"),
            populated: counter(&events, "populated").map(|value| value == 1),
        })
    }

    fn forget(self: &Arc<Self>, backend: &Backend, unit: String) {
        let registry = self.clone();
        let backend = backend.clone();
        tokio::spawn(async move {
            for _ in 0..120 {
                let Some(state) = registry.read_scope(&backend, &unit).await else {
                    return;
                };
                if state.active == "failed" {
                    Self::ctl(&backend, &["--user", "reset-failed", &unit]).await;
                    return;
                }
                if state.active != "deactivating" {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    async fn track(self: &Arc<Self>, backend: &Backend, thread: &str, tracked: Option<Tracked>) {
        let (previous, oldest) = {
            let mut entries = self.tracked.lock().unwrap();
            let previous = entries
                .iter()
                .position(|(key, _)| key == thread)
                .and_then(|index| entries.remove(index));
            if let Some(tracked) = tracked {
                entries.push_back((thread.to_owned(), tracked));
            }
            let oldest = (entries.len() > 500)
                .then(|| entries.front().cloned())
                .flatten();
            (previous, oldest)
        };
        if let Some((_, previous)) = previous {
            self.forget(backend, previous.unit);
        }
        if let Some((_, oldest)) = oldest {
            let state = self.read_scope(backend, &oldest.unit).await;
            let status = state.as_ref().map(classify).unwrap_or(Status::Running);
            let removed = {
                let mut entries = self.tracked.lock().unwrap();
                if entries
                    .front()
                    .is_some_and(|(_, tracked)| tracked.unit == oldest.unit)
                {
                    let first = entries.pop_front().unwrap();
                    if matches!(status, Status::Running | Status::Stopping) {
                        entries.push_back(first);
                        false
                    } else {
                        true
                    }
                } else {
                    false
                }
            };
            if removed {
                self.forget(backend, oldest.unit);
            }
        }
    }

    pub async fn clear(self: &Arc<Self>, thread: &str) {
        if !self
            .tracked
            .lock()
            .unwrap()
            .iter()
            .any(|(key, _)| key == thread)
        {
            return;
        }
        if let Some(backend) = self.backend().await {
            self.track(backend, thread, None).await;
        }
    }

    pub async fn oom_killed(self: &Arc<Self>, thread: &str) -> bool {
        let tracked = self
            .tracked
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key == thread)
            .map(|(_, tracked)| tracked.clone());
        let Some(tracked) = tracked else {
            return false;
        };
        if tracked.oom_killed {
            return true;
        }
        let Some(backend) = self.backend().await else {
            return false;
        };
        for _ in 0..20 {
            let Some(state) = self.read_scope(backend, &tracked.unit).await else {
                return false;
            };
            match classify(&state) {
                Status::Running => return false,
                Status::Stopping => tokio::time::sleep(Duration::from_millis(100)).await,
                status => {
                    self.forget(backend, tracked.unit.clone());
                    if status == Status::Gone {
                        return false;
                    }
                    if let Some((_, current)) = self
                        .tracked
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .find(|(key, current)| key == thread && current.unit == tracked.unit)
                    {
                        current.oom_killed = true;
                    }
                    return true;
                }
            }
        }
        false
    }

    pub async fn stop(&self, unit: &str) {
        if !unit.starts_with("t3code-") || !unit.ends_with(".scope") {
            return;
        }
        let Some(backend) = self.backend().await else {
            return;
        };
        let Some(state) = self.read_scope(backend, unit).await else {
            return;
        };
        if state.load != "loaded"
            || state.active != "active"
            || state.result == "oom-kill"
            || state.populated == Some(false)
            || (backend.oom_policy && state.oom_kills.is_none_or(|count| count > 0))
        {
            return;
        }
        if !Self::ctl(backend, &["--user", "stop", "--no-block", unit])
            .await
            .is_some_and(|output| output.status.success())
        {
            eprintln!("Could not stop an exited agent's scope: {unit}");
        }
    }

    pub async fn kill(&self, thread: &str) -> bool {
        let tracked = self
            .tracked
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key == thread)
            .map(|(_, tracked)| tracked.clone());
        let Some(tracked) = tracked.filter(|tracked| !tracked.shared) else {
            return false;
        };
        let Some(backend) = self.backend().await else {
            return false;
        };
        let Some(state) = self.read_scope(backend, &tracked.unit).await else {
            return false;
        };
        if state.active != "active" {
            return false;
        }
        if let Some(path) = self.group_file(&state.group, "cgroup.kill") {
            // Opening an existing kernel control file must never create a regular file.
            use tokio::io::AsyncWriteExt;
            if let Ok(mut file) = tokio::fs::OpenOptions::new().write(true).open(path).await {
                if file.write_all(b"1").await.is_ok() && file.flush().await.is_ok() {
                    return true;
                }
            }
        }
        Self::ctl(
            backend,
            &["--user", "kill", "--signal=SIGKILL", &tracked.unit],
        )
        .await
        .is_some_and(|output| output.status.success())
    }

    /// Wrap before configuring stdio, process groups or kill-on-drop. Commands inherit
    /// the server environment, with the command's explicit additions and removals applied.
    pub(crate) async fn wrap(
        self: &Arc<Self>,
        command: Command,
        name: &str,
        thread: Option<&str>,
        shared: bool,
        survives_restart: bool,
    ) -> Result<(Command, Option<Lease>), String> {
        if !cfg!(target_os = "linux") || !self.enabled.load(Ordering::Acquire) {
            return Ok((command, None));
        }
        let source = command.as_std();
        let changes: Vec<_> = source
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect();
        let cwd = match source.get_current_dir() {
            Some(cwd) if cwd.is_absolute() => cwd.to_owned(),
            Some(cwd) => std::env::current_dir()
                .map_err(|error| error.to_string())?
                .join(cwd),
            None => std::env::current_dir().map_err(|error| error.to_string())?,
        };
        let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        for (key, value) in &changes {
            if let Some(value) = value {
                environment.insert(key.clone(), value.clone());
            } else {
                environment.remove(key);
            }
        }
        let program = Path::new(source.get_program());
        let Some(program) = resolve(program, &cwd, &environment) else {
            return Ok((command, None));
        };
        let mut lease = None;
        let mut wrapped = if let Some(backend) = self.backend().await {
            let unit = format!("t3code-{name}-{}.scope", &crate::store::random_id()?[..8]);
            let continuing = shared && backend.oom_policy;
            let owned = thread.is_some() && !continuing;
            if let Some(thread) = thread {
                self.track(
                    backend,
                    thread,
                    owned.then(|| Tracked {
                        unit: unit.clone(),
                        shared,
                        oom_killed: false,
                    }),
                )
                .await;
            }
            let wrapped = self.scoped_command(
                backend,
                &unit,
                &program,
                source.get_args().map(ToOwned::to_owned),
                ScopeOptions {
                    owned,
                    continuing,
                    survives_restart,
                },
            );
            lease = Some(Lease {
                registry: self.clone(),
                unit,
            });
            wrapped
        } else {
            let mut wrapped = Command::new(program);
            wrapped.args(source.get_args());
            wrapped
        };
        for (key, value) in changes {
            if let Some(value) = value {
                wrapped.env(key, value);
            } else {
                wrapped.env_remove(key);
            }
        }
        if let Some(backend) = self.backend.get().and_then(Option::as_ref) {
            if environment
                .get(std::ffi::OsStr::new("XDG_RUNTIME_DIR"))
                .is_none_or(|value| value.is_empty())
            {
                wrapped.env("XDG_RUNTIME_DIR", &backend.runtime);
            }
        }
        wrapped.current_dir(cwd);
        #[cfg(target_os = "linux")]
        unsafe {
            // Only async-signal-safe syscalls run after fork. Failure is deliberately
            // ignored, matching the upstream wrapper on restricted kernels.
            wrapped.pre_exec(|| {
                raise_oom_score();
                Ok(())
            });
        }
        Ok((wrapped, lease))
    }
}

/// Only async-signal-safe syscalls; callers run this in the child before exec.
pub(crate) unsafe fn raise_oom_score() {
    #[cfg(target_os = "linux")]
    {
        let fd = libc::open(
            c"/proc/self/oom_score_adj".as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC,
        );
        if fd >= 0 {
            libc::write(fd, b"800\n".as_ptr().cast(), 4);
            libc::close(fd);
        }
    }
}

fn resolve(
    program: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let candidates: Vec<_> = if program.components().count() > 1 || program.is_absolute() {
        vec![cwd.join(program)]
    } else {
        std::env::split_paths(
            environment
                .get(std::ffi::OsStr::new("PATH"))
                .map(OsString::as_os_str)
                .unwrap_or_default(),
        )
        .map(|directory| cwd.join(directory).join(program))
        .collect()
    };
    candidates.into_iter().find(|path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

pub(crate) struct Lease {
    registry: Arc<Registry>,
    unit: String,
}
impl Lease {
    pub(crate) async fn stop(&mut self) {
        self.registry.stop(&self.unit).await;
        self.unit.clear();
    }
    pub(crate) fn attach(&self, thread: &str) {
        let previous = {
            let mut tracked = self.registry.tracked.lock().unwrap();
            tracked
                .iter()
                .position(|(key, current)| key == thread && current.unit != self.unit)
                .and_then(|index| tracked.remove(index))
        };
        if let Some((_, previous)) = previous {
            if let Some(backend) = self.registry.backend.get().and_then(Option::as_ref) {
                self.registry.forget(backend, previous.unit);
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.unit.is_empty() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let registry = self.registry.clone();
            let unit = self.unit.clone();
            runtime.spawn(async move {
                registry.stop(&unit).await;
            });
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    struct Fixture {
        root: PathBuf,
        registry: Arc<Registry>,
    }
    impl Fixture {
        fn new() -> Self {
            assert!(
                Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists(),
                "Process isolation fixtures must run in tools/test-rust-sandbox"
            );
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!(
                "o3-agent-scope-{}",
                crate::store::random_id().unwrap()
            ));
            std::fs::create_dir_all(root.join("cgroups")).unwrap();
            for name in ["systemctl", "systemd-run"] {
                std::fs::write(
                    root.join(name),
                    include_str!("../tests/fixtures/agent-scope-peer.py"),
                )
                .unwrap();
                std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            std::fs::write(root.join("units.json"), "{}").unwrap();
            let mut registry = Registry::new(&root.join("userdata"));
            let inner = Arc::get_mut(&mut registry).unwrap();
            inner.host = BTreeMap::from([
                (OsString::from("PATH"), root.as_os_str().to_owned()),
                (
                    OsString::from("XDG_RUNTIME_DIR"),
                    root.join("runtime").into_os_string(),
                ),
            ]);
            inner.cgroup_root = root.join("cgroups");
            registry.enable();
            Self { root, registry }
        }
        fn calls(&self) -> Vec<Value> {
            std::fs::read_to_string(self.root.join("calls.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
        fn units(&self, units: Value) {
            let temporary = self
                .root
                .join(format!("units-{}", crate::store::random_id().unwrap()));
            std::fs::write(&temporary, units.to_string()).unwrap();
            std::fs::rename(temporary, self.root.join("units.json")).unwrap();
        }
        fn state(&self, unit: &str, result: &str, active: &str, group: &str) {
            self.units(json!({unit:{"LoadState":"loaded","ActiveState":active,"Result":result,"ControlGroup":group}}));
        }
        fn group(&self, name: &str, memory: &str, events: &str) -> PathBuf {
            let group = self.root.join("cgroups").join(name);
            std::fs::create_dir_all(&group).unwrap();
            std::fs::write(group.join("memory.events"), memory).unwrap();
            std::fs::write(group.join("cgroup.events"), events).unwrap();
            group
        }
        async fn launch(&self, thread: &str, shared: bool) -> (Command, Option<Lease>) {
            self.registry
                .wrap(
                    Command::new("/usr/bin/python3"),
                    "fixture",
                    Some(thread),
                    shared,
                    false,
                )
                .await
                .unwrap()
        }
    }

    #[test]
    fn nightly_process_helpers_match_executed_source() {
        let cases: Value =
            serde_json::from_str(include_str!("../tests/fixtures/agent-scope-source.json"))
                .unwrap();
        for case in cases["slices"].as_array().unwrap() {
            assert_eq!(
                owned_agent_slice(Path::new(case["input"].as_str().unwrap())),
                case["expected"]
            );
        }
        for case in cases["limits"].as_array().unwrap() {
            let (high, max) = memory_limits(case["input"].as_u64().unwrap());
            assert_eq!(json!({"high":high,"max":max}), case["expected"]);
        }
        for case in cases["states"].as_array().unwrap() {
            let input = &case["input"];
            let state = ScopeState {
                load: input["loadState"].as_str().unwrap().into(),
                active: input["activeState"].as_str().unwrap().into(),
                result: input["result"].as_str().unwrap().into(),
                oom_kills: input["oomKills"].as_u64(),
                populated: input["populated"].as_bool(),
                ..Default::default()
            };
            let actual = match classify(&state) {
                Status::OomKilled => "oom-killed",
                Status::Gone => "gone",
                Status::Running => "running",
                Status::Stopping => "stopping",
            };
            assert_eq!(actual, case["expected"], "{input}");
        }
    }

    #[tokio::test]
    async fn scoped_launch_keeps_arguments_environment_cwd_stdio_and_raises_only_child_oom_score() {
        let f = Fixture::new();
        let before = std::fs::read_to_string("/proc/self/oom_score_adj").unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import json,os,sys;print(json.dumps({'args':sys.argv[1:],'cwd':os.getcwd(),'env':os.environ['SCOPE_CHECK'],'runtime':os.environ['XDG_RUNTIME_DIR'],'oom':open('/proc/self/oom_score_adj').read().strip()}))", "a' $()\n🚀"])
            .env("SCOPE_CHECK", "private-override").env_remove("XDG_RUNTIME_DIR").current_dir(&f.root);
        let (mut command, lease) = f
            .registry
            .wrap(command, "fixture", Some("owner"), false, false)
            .await
            .unwrap();
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result,
            json!({"args":["a' $()\n🚀"],"cwd":f.root,"env":"private-override","runtime":f.root.join("runtime"),"oom":"800"})
        );
        assert_eq!(
            std::fs::read_to_string("/proc/self/oom_score_adj").unwrap(),
            before
        );
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&format!("--slice={}", f.registry.owned_slice)));
        assert!(!args.contains(&"--collect".to_owned()));
        assert!(args.contains(&"--property=OOMPolicy=stop".to_owned()));
        let calls = f.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["args"][1] == "set-property")
                .count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["tool"] == "systemd-run")
                .count(),
            2
        );
        drop(lease);
    }

    #[tokio::test]
    async fn fallback_without_user_manager_keeps_native_environment_and_oom_adjustment() {
        let f = Fixture::new();
        std::fs::write(f.root.join("probe-disabled"), "").unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c","import os;print(os.environ['SCOPE_CHECK'],open('/proc/self/oom_score_adj').read().strip())"]).env("SCOPE_CHECK","fallback");
        let (mut command, lease) = f
            .registry
            .wrap(command, "fixture", Some("owner"), false, false)
            .await
            .unwrap();
        assert!(lease.is_none());
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"fallback 800\n");
        assert_eq!(
            f.calls()
                .iter()
                .filter(|call| call["tool"] == "systemd-run")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn old_systemd_shared_process_is_tracked_for_oom_and_never_killed_for_one_thread() {
        let f = Fixture::new();
        std::fs::write(f.root.join("probe-no-oom"), "").unwrap();
        let (command, lease) = f.launch("owner", true).await;
        assert!(!command
            .as_std()
            .get_args()
            .any(|arg| arg.to_string_lossy().starts_with("--property=OOMPolicy=")));
        let unit = lease.as_ref().unwrap().unit.clone();
        f.state(&unit, "oom-kill", "failed", "");
        assert!(!f.registry.kill("owner").await);
        assert!(f.registry.oom_killed("owner").await);
        assert_eq!(
            f.calls()
                .iter()
                .filter(|call| call["tool"] == "systemd-run")
                .count(),
            2
        );
        assert!(!f.calls().iter().any(|call| call["args"][1] == "kill"));
    }

    #[tokio::test]
    async fn continuing_shared_scope_clears_prior_ownership_and_can_survive_restart() {
        let f = Fixture::new();
        let (_, old) = f.launch("owner", false).await;
        let (command, lease) = f
            .registry
            .wrap(
                Command::new("/usr/bin/true"),
                "fixture",
                Some("owner"),
                true,
                true,
            )
            .await
            .unwrap();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--property=OOMPolicy=continue".into()));
        assert!(args.contains(&"--collect".into()));
        assert!(args.contains(&format!("--slice={AGENT_SLICE}")));
        assert!(!f.registry.kill("owner").await);
        assert!(!f.registry.oom_killed("owner").await);
        drop((old, lease));
    }

    #[tokio::test]
    async fn owned_kill_uses_existing_private_cgroup_control_then_systemctl_fallback() {
        let f = Fixture::new();
        let (_, lease) = f.launch("owner", false).await;
        let unit = &lease.as_ref().unwrap().unit;
        let group = f.group("owned", "oom_kill 0\n", "populated 1\n");
        std::fs::write(group.join("cgroup.kill"), "").unwrap();
        f.state(unit, "success", "active", "/owned");
        assert!(f.registry.kill("owner").await);
        assert_eq!(
            std::fs::read_to_string(group.join("cgroup.kill")).unwrap(),
            "1"
        );
        std::fs::remove_file(group.join("cgroup.kill")).unwrap();
        assert!(f.registry.kill("owner").await);
        assert!(!group.join("cgroup.kill").exists());
        assert!(f
            .calls()
            .iter()
            .any(|call| call["args"] == json!(["--user", "kill", "--signal=SIGKILL", unit])));
        assert!(f
            .registry
            .group_file("/../../host", "cgroup.kill")
            .is_none());
        assert!(f.registry.group_file("/", "cgroup.kill").is_none());
    }

    #[tokio::test]
    async fn exited_scope_stop_preserves_oom_diagnosis_and_rejects_unowned_unit_names() {
        let f = Fixture::new();
        let (_, lease) = f.launch("owner", false).await;
        let unit = &lease.as_ref().unwrap().unit;
        f.state(unit, "success", "active", "/owned");
        f.registry.stop(unit).await;
        f.group("owned", "oom_kill 1\n", "populated 1\n");
        f.registry.stop(unit).await;
        f.group("owned", "oom_kill 0\n", "populated 0\n");
        f.registry.stop(unit).await;
        f.state(unit, "oom-kill", "active", "/owned");
        f.registry.stop(unit).await;
        assert!(!f.calls().iter().any(|call| call["args"][1] == "stop"));
        f.state(unit, "success", "active", "/owned");
        f.group("owned", "oom_kill 0\n", "populated 1\n");
        f.registry.stop(unit).await;
        f.registry.stop("another-application.service").await;
        assert_eq!(
            f.calls()
                .iter()
                .filter(|call| call["args"][1] == "stop")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn oom_result_waits_for_systemd_is_sticky_and_is_cleared_by_new_session() {
        let f = Fixture::new();
        let (_, lease) = f.launch("owner", false).await;
        let unit = lease.as_ref().unwrap().unit.clone();
        f.group("owned", "oom_kill 1\n", "populated 0\n");
        f.state(&unit, "success", "deactivating", "/owned");
        let registry = f.registry.clone();
        let pending = tokio::spawn(async move { registry.oom_killed("owner").await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!pending.is_finished());
        f.state(&unit, "oom-kill", "failed", "/owned");
        assert!(pending.await.unwrap());
        f.units(json!({}));
        assert!(f.registry.oom_killed("owner").await);
        f.registry.clear("owner").await;
        assert!(!f.registry.oom_killed("owner").await);
    }

    #[tokio::test]
    async fn restarting_only_stops_populated_agents_in_the_exact_profile_slice() {
        for populated in [0, 1] {
            let f = Fixture::new();
            f.group(
                "leftovers",
                "oom_kill 0\n",
                &format!("populated {populated}\n"),
            );
            f.state(&f.registry.owned_slice, "success", "active", "/leftovers");
            std::fs::write(f.root.join("fail-stop"), "").unwrap();
            let (_, lease) = f.launch("owner", false).await;
            let stopped: Vec<_> = f
                .calls()
                .into_iter()
                .filter(|call| call["args"][1] == "stop")
                .collect();
            assert_eq!(stopped.len(), populated);
            if populated == 1 {
                assert_eq!(
                    stopped[0]["args"],
                    json!(["--user", "stop", f.registry.owned_slice])
                );
            }
            drop(lease);
        }
    }

    #[tokio::test]
    async fn simultaneous_wraps_probe_once_and_old_lease_cannot_clear_new_scope() {
        let f = Fixture::new();
        let (first, second) = tokio::join!(f.launch("owner", false), f.launch("owner", false));
        assert_eq!(
            f.calls()
                .iter()
                .filter(|call| call["tool"] == "systemd-run")
                .count(),
            1
        );
        let (_, old) = first;
        let (_, current) = second;
        let old_unit = &old.as_ref().unwrap().unit;
        let current_unit = &current.as_ref().unwrap().unit;
        f.group("owned", "oom_kill 0\n", "populated 1\n");
        f.units(json!({old_unit:{"LoadState":"loaded","ActiveState":"active","Result":"success","ControlGroup":"/owned"},current_unit:{"LoadState":"loaded","ActiveState":"active","Result":"success","ControlGroup":"/owned"}}));
        drop(old);
        assert!(f.registry.kill("owner").await);
        assert!(f.calls().iter().any(
            |call| call["args"] == json!(["--user", "kill", "--signal=SIGKILL", current_unit])
        ));
    }

    #[tokio::test]
    async fn tracking_evicts_inactive_scopes_and_retains_live_or_stopping_ones() {
        let f = Fixture::new();
        f.registry.backend().await.unwrap();
        {
            let mut tracked = f.registry.tracked.lock().unwrap();
            for index in 0..500 {
                tracked.push_back((
                    format!("thread-{index}"),
                    Tracked {
                        unit: format!("t3code-fixture-{index}.scope"),
                        shared: false,
                        oom_killed: false,
                    },
                ));
            }
        }
        f.state("t3code-fixture-0.scope", "success", "active", "");
        let (_, first) = f.launch("new-1", false).await;
        assert_eq!(f.registry.tracked.lock().unwrap().len(), 501);
        assert!(f
            .registry
            .tracked
            .lock()
            .unwrap()
            .iter()
            .any(|(thread, _)| thread == "thread-0"));
        let (_, second) = f.launch("new-2", false).await;
        assert_eq!(f.registry.tracked.lock().unwrap().len(), 501);
        assert!(!f
            .registry
            .tracked
            .lock()
            .unwrap()
            .iter()
            .any(|(thread, _)| thread == "thread-1"));
        drop((first, second));
    }

    #[tokio::test]
    async fn embedded_default_and_missing_binary_never_probe_systemd_or_replace_the_command() {
        let registry = Registry::new(Path::new("/private/test/userdata"));
        let (command, lease) = registry
            .wrap(Command::new("/bin/true"), "fixture", None, false, false)
            .await
            .unwrap();
        assert_eq!(command.as_std().get_program(), "/bin/true");
        assert!(lease.is_none());
        assert!(registry.backend.get().is_none());
        let f = Fixture::new();
        let program = f.root.join("missing");
        let (command, lease) = f
            .registry
            .wrap(
                Command::new(&program),
                "fixture",
                Some("owner"),
                false,
                false,
            )
            .await
            .unwrap();
        assert_eq!(command.as_std().get_program(), program);
        assert!(lease.is_none());
        assert!(f.calls().is_empty());
    }

    #[tokio::test]
    async fn terminal_fallback_raises_oom_score_without_reintroducing_the_parent_environment() {
        let f = Fixture::new();
        std::fs::write(f.root.join("probe-disabled"), "").unwrap();
        f.registry.initialize().await;
        let env = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), f.root.to_string_lossy().into_owned()),
            ("PYTHON_BASIC_REPL".into(), "1".into()),
        ];
        let plan = f
            .registry
            .terminal_command("python3", &f.root, &env)
            .unwrap();
        assert!(plan.adjust_oom);
        assert!(plan.args.is_empty());
        assert_eq!(plan.env, env);
        let mut terminal = crate::pty::spawn_prepared(&plan, &f.root, 80, 24, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(format!("/proc/{}/oom_score_adj", terminal.pid))
                .unwrap()
                .trim(),
            "800"
        );
        let environment = std::fs::read(format!("/proc/{}/environ", terminal.pid)).unwrap();
        assert!(!environment
            .split(|byte| *byte == 0)
            .any(|entry| entry.starts_with(b"XDG_RUNTIME_DIR=")));
        crate::pty::signal_group(terminal.pid, libc::SIGKILL);
        assert!(!terminal.child.wait().await.unwrap().success());
    }

    #[tokio::test]
    async fn invalid_terminal_requests_do_not_initialize_resource_policy() {
        let f = Fixture::new();
        let state = Arc::new(crate::store::State::open(&f.root.join("state")).unwrap());
        state.agent_scopes.enable();
        assert!(crate::terminal::call(&state, "terminal.open", &json!({}))
            .await
            .is_err());
        assert!(crate::terminal::call(
            &state,
            "terminal.open",
            &json!({"threadId":"t","terminalId":"terminal","cwd":f.root.join("missing")})
        )
        .await
        .is_err());
        assert!(crate::terminal::open_stream(
            &state,
            "terminal.attach",
            &json!({"threadId":"t","terminalId":"terminal","cwd":f.root.join("missing")})
        )
        .await
        .is_err());
        assert!(state.agent_scopes.backend.get().is_none());
    }

    #[tokio::test]
    async fn configured_native_providers_execute_through_the_shared_registry_and_own_app_thread_ids(
    ) {
        use crate::provider::{Provider, ProviderEvent, RunInput};
        use std::os::unix::fs::PermissionsExt;
        for driver in ["codex", "claudeAgent"] {
            let f = Fixture::new();
            let peer = f.root.join("native-peer.py");
            std::fs::write(&peer, include_str!("../tests/fixtures/goal-peer.py")).unwrap();
            std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::write(f.root.join("settings.json"),json!({"providerInstances":{"fixture":{"driver":driver,"enabled":true,"config":{"binaryPath":peer,"homePath":f.root.join("native-home")}}}}).to_string()).unwrap();
            let provider = crate::provider::ConfiguredProviders::new(
                Arc::new(crate::provider::ScriptedProvider),
                f.root.clone(),
            )
            .with_agent_scopes(f.registry.clone());
            for app_thread in ["first-app", "second-app"] {
                let (output, mut events) = tokio::sync::mpsc::channel(64);
                let (_controls, control) = tokio::sync::mpsc::channel(4);
                let (_cancel, cancel) = tokio::sync::watch::channel(false);
                let input = RunInput {
                    native_mcp: None,
                    provider_instance_id: "fixture".into(),
                    native_session_id: None,
                    thread_id: format!("{app_thread}:binding-generation"),
                    run_id: format!("run-{app_thread}"),
                    cwd: f.root.clone(),
                    text: "ordinary prompt".into(),
                    model_selection: json!({"model":"goal-fixture","options":[],"o3AppThreadId":app_thread}),
                    runtime_mode: "full-access".into(),
                    interaction_mode: "default".into(),
                    attachment_root: f.root.clone(),
                    attachments: json!([]),
                    context: None,
                };
                let execution = provider.execute(input, output, control, cancel);
                let collect = async {
                    let mut completed = false;
                    let mut text = String::new();
                    while let Some(event) = events.recv().await {
                        match event {
                            ProviderEvent::Completed => completed = true,
                            ProviderEvent::TextDelta(delta) => text.push_str(&delta),
                            ProviderEvent::Failed(error) => panic!("{error}"),
                            _ => {}
                        }
                    }
                    (completed, text)
                };
                let (result, (completed, text)) =
                    tokio::time::timeout(Duration::from_secs(8), async {
                        tokio::join!(execution, collect)
                    })
                    .await
                    .unwrap();
                result.unwrap();
                assert!(completed, "{driver}: {text}");
                assert!(
                    text.contains(if driver == "codex" {
                        "FIRST"
                    } else {
                        "CLAUDE_GOAL_PROOF"
                    }),
                    "{driver}: {text}"
                );
            }
            let calls = f.calls();
            let launches: Vec<_> =
                calls
                    .iter()
                    .filter(|call| {
                        call["tool"] == "systemd-run"
                            && call["args"].as_array().unwrap().iter().any(|arg| {
                                arg.as_str().is_some_and(|arg| arg.starts_with("--unit="))
                            })
                    })
                    .collect();
            assert_eq!(
                launches.len(),
                if driver == "codex" { 1 } else { 2 },
                "{driver}: {calls:?}"
            );
            assert!(launches.iter().all(|call| call["oomScore"] == "800"
                && call["runtime"] == json!(f.root.join("runtime"))));
            if driver == "codex" {
                assert!(!f.registry.kill("first-app").await);
                assert!(!f.registry.kill("second-app").await);
            } else {
                let tracked = f.registry.tracked.lock().unwrap();
                assert!(tracked.iter().any(|(thread, _)| thread == "first-app"));
                assert!(tracked.iter().any(|(thread, _)| thread == "second-app"));
                assert!(!tracked
                    .iter()
                    .any(|(thread, _)| thread.contains("binding-generation")));
            }
            provider.dispose().await.unwrap();
        }
    }

    #[test]
    fn compatible_client_sees_the_confirmed_oom_instead_of_a_transport_eof() {
        use crate::checkpoint::recovery_test_support as support;
        use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
        struct FailedTransport;
        impl Provider for FailedTransport {
            fn execute<'a>(
                &'a self,
                _input: RunInput,
                output: tokio::sync::mpsc::Sender<ProviderEvent>,
                _control: tokio::sync::mpsc::Receiver<ProviderControl>,
                _cancel: tokio::sync::watch::Receiver<bool>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
            {
                Box::pin(async move {
                    output
                        .send(ProviderEvent::Started)
                        .await
                        .map_err(|_| "closed")?;
                    Err("Fixture transport EOF".into())
                })
            }
        }
        let f = Fixture::new();
        f.state("t3code-fixture-oom.scope", "oom-kill", "failed", "");
        let server = support::start(|state| {
            state.agent_scopes.enable();
            assert!(state
                .agent_scopes
                .backend
                .set(Some(Backend {
                    run: f.root.join("systemd-run"),
                    ctl: f.root.join("systemctl"),
                    runtime: f.root.join("runtime").into_os_string(),
                    oom_policy: true
                }))
                .is_ok());
            state.agent_scopes.tracked.lock().unwrap().push_back((
                "t".into(),
                Tracked {
                    unit: "t3code-fixture-oom.scope".into(),
                    shared: false,
                    oom_killed: false,
                },
            ));
            *state.provider.lock().unwrap() = Some(Arc::new(FailedTransport));
        });
        let client = server.connect();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"OOM fixture","workspaceRoot":f.root})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"t","projectId":"p","title":"OOM fixture","modelSelection":{"instanceId":"scripted","model":"fixture","options":[]},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"message","messageId":"message","threadId":"t","text":"Fixture EOF","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(8), || {
            let projection = client
                .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap();
            projection["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["status"] == "failed" && run["error"] == "Killed: out of memory.")
        }));
        let projection = client
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap();
        let card = projection["turnItems"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "error")
            .expect("confirmed OOM must publish a terminal failure card");
        assert_eq!(card["failure"]["code"], "oom_kill");
        assert_eq!(card["failure"]["message"], "Killed: out of memory.");
        assert_eq!(card["failure"]["class"], "unknown");
    }

    #[tokio::test]
    #[ignore = "real systemd and private cgroups; use tools/test-agent-systemd"]
    async fn native_systemd_reaps_detached_children_preserves_shared_processes_and_reports_oom() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        assert_eq!(
            std::env::var("O3_AGENT_SYSTEMD_CONTAINER").as_deref(),
            Ok("1")
        );
        assert!(Path::new("/run/.containerenv").exists());
        assert_eq!(
            std::fs::read_to_string("/proc/1/comm").unwrap().trim(),
            "systemd"
        );
        let root = std::env::temp_dir().join(format!(
            "o3-native-scopes-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let terminal_state = Arc::new(crate::store::State::open(&root).unwrap());
        let registry = terminal_state.agent_scopes.clone();
        registry.enable();
        let backend = registry
            .backend()
            .await
            .expect("Disposable systemd user manager required")
            .clone();

        async fn worker(
            registry: &Arc<Registry>,
            thread: &str,
            shared: bool,
            detached: bool,
            oom: bool,
        ) -> (tokio::process::Child, Lease, Value) {
            let code=format!("import os,sys,json,subprocess\npid=None\nif {detached}:\n p=subprocess.Popen(['/usr/bin/python3','-u','-c','import os,time;os.setsid();print(os.getpid(),flush=True);time.sleep(120)'],stdout=subprocess.PIPE,text=True)\n pid=int(p.stdout.readline())\nprint(json.dumps({{'pid':os.getpid(),'detached':pid,'oom':open('/proc/self/oom_score_adj').read().strip()}}),flush=True)\nsys.stdin.readline()\nif {oom}:\n blocks=[]\n while True: blocks.append(bytearray(2*1024*1024))\n",detached=if detached{"True"}else{"False"},oom=if oom{"True"}else{"False"});
            let mut command = Command::new("/usr/bin/python3");
            command.args(["-u", "-c", &code]);
            let (mut command, lease) = registry
                .wrap(command, "native-proof", Some(thread), shared, false)
                .await
                .unwrap();
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn().unwrap();
            let mut reader = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            assert!(!line.is_empty(), "Scoped worker did not start");
            let proof: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(proof["oom"], "800");
            (child, lease.unwrap(), proof)
        }
        async fn gone(pid: u64) {
            for _ in 0..50 {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("Owned child {pid} survived its scope");
        }
        async fn finish(child: &mut tokio::process::Child) {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"exit\n")
                .await
                .unwrap();
            assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success());
        }

        let (mut child, mut lease, proof) =
            worker(&registry, "stop-owned", false, true, false).await;
        let state = registry.read_scope(&backend, &lease.unit).await.unwrap();
        assert_eq!(classify(&state), Status::Running);
        assert!(state.group.contains(&registry.owned_slice));
        assert!(std::fs::read_to_string(format!(
            "/proc/{}/cgroup",
            proof["detached"].as_u64().unwrap()
        ))
        .unwrap()
        .contains(&lease.unit));
        finish(&mut child).await;
        lease.stop().await;
        gone(proof["detached"].as_u64().unwrap()).await;

        let (mut child, mut lease, proof) =
            worker(&registry, "kill-owned", false, true, false).await;
        assert!(registry.kill("kill-owned").await);
        assert!(!tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        gone(proof["detached"].as_u64().unwrap()).await;
        lease.stop().await;

        let (mut shared, mut shared_lease, _) =
            worker(&registry, "shared", true, false, false).await;
        assert!(!registry.kill("shared").await);
        assert!(shared.try_wait().unwrap().is_none());
        finish(&mut shared).await;
        shared_lease.stop().await;

        std::env::set_var("O3_SCOPE_PARENT_PRIVATE", "not-for-the-terminal");
        std::env::set_var("SHELL", "/usr/bin/python3");
        let fresh_terminal_state =
            Arc::new(crate::store::State::open(&root.join("terminal-api")).unwrap());
        fresh_terminal_state.agent_scopes.enable();
        assert!(fresh_terminal_state.agent_scopes.backend.get().is_none());
        let api_input = json!({"threadId":"terminal-api-thread","terminalId":"terminal-api","cwd":root,"cols":80,"rows":24,"env":{"XDG_RUNTIME_DIR":""}});
        let snapshot = crate::terminal::call(&fresh_terminal_state, "terminal.open", &api_input)
            .await
            .unwrap();
        assert_eq!(snapshot["status"], "running");
        let api_pid = snapshot["pid"].as_u64().unwrap();
        let api_group = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let group = std::fs::read_to_string(format!("/proc/{api_pid}/cgroup")).unwrap();
                if group.contains("t3code-terminal-") {
                    break group;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(api_group.contains(AGENT_SLICE));
        let env = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), "/tmp/home".into()),
            ("TERM".into(), "xterm".into()),
            ("PYTHON_BASIC_REPL".into(), "1".into()),
            ("PTY_SCOPE_ONLY".into(), "literal-$HOME".into()),
        ];
        let mut terminals = Vec::new();
        for scoped in [false, true] {
            let launch = registry
                .terminal_command("/usr/bin/python3", &root, &env)
                .unwrap();
            let terminal = crate::pty::spawn_prepared(&launch, &root, 80, 24, scoped).unwrap();
            let command="import os,json;print('__PTY_PROOF__'+json.dumps({'pid':os.getpid(),'pgrp':os.getpgrp(),'sid':os.getsid(0),'tty':os.isatty(0),'env':os.environ.get('PTY_SCOPE_ONLY'),'leaked':'O3_SCOPE_PARENT_PRIVATE' in os.environ,'oom':open('/proc/self/oom_score_adj').read().strip(),'group':open('/proc/self/cgroup').read().strip()}))\n";
            crate::pty::write_all(&terminal.master, command.as_bytes(), Duration::from_secs(3))
                .await
                .unwrap();
            let mut output = String::new();
            let proof = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let mut bytes = [0u8; 4096];
                    if let Ok(count) = crate::pty::read(&terminal.master, &mut bytes) {
                        output.push_str(&String::from_utf8_lossy(&bytes[..count]));
                    }
                    if let Some((_, json)) = output.split_once("__PTY_PROOF__{") {
                        if let Some(end) = json.find('}') {
                            break serde_json::from_str::<Value>(&format!("{{{}", &json[..=end]))
                                .unwrap();
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(proof["pid"], terminal.pid);
            assert_eq!(proof["pgrp"], terminal.pid);
            assert_eq!(proof["sid"], terminal.pid);
            assert_eq!(proof["tty"], true);
            assert_eq!(proof["env"], "literal-$HOME");
            assert_eq!(proof["leaked"], false);
            assert_eq!(proof["oom"], "800");
            let group = proof["group"].as_str().unwrap();
            assert!(
                group.contains(AGENT_SLICE)
                    && group.contains("t3code-terminal-")
                    && !group.contains(&registry.owned_slice)
            );
            terminals.push(terminal);
        }

        // The CDP descriptors must survive systemd-run as well as the normal
        // launch path. Keep the browser peer inside this private namespace.
        use std::os::unix::fs::PermissionsExt;
        let mut browser_state = crate::store::State::open(&root.join("browser-state")).unwrap();
        browser_state.agent_scopes = registry.clone();
        let peer = root.join("browser-cdp.py");
        std::fs::write(&peer, include_str!("../tests/fixtures/browser-cdp.py")).unwrap();
        std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
        let browser = &browser_state.streams.p2.preview.browser;
        *browser.fixture_binary.lock().unwrap() = Some(peer);
        browser
            .open(
                &browser_state,
                "browser-thread",
                "browser-tab",
                "default",
                None,
            )
            .await
            .unwrap();
        let profile = std::fs::read_dir(browser_state.dir.join("browser-profiles"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let browser_pid = std::fs::read_to_string(profile.join("fixture.pid"))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let group = std::fs::read_to_string(format!("/proc/{browser_pid}/cgroup")).unwrap();
        assert!(group.contains(&registry.owned_slice) && group.contains("t3code-preview-browser-"));
        assert_eq!(
            std::fs::read_to_string(format!("/proc/{browser_pid}/oom_score_adj"))
                .unwrap()
                .trim(),
            "800"
        );
        assert_eq!(
            browser
                .screenshot("browser-thread", "browser-tab")
                .await
                .unwrap(),
            [255, 216, 255, 217]
        );
        assert!(!registry.kill("browser-thread").await);
        browser
            .open(
                &browser_state,
                "another-browser-thread",
                "another-browser-tab",
                "default",
                None,
            )
            .await
            .unwrap();
        browser.close("browser-thread", Some("browser-tab")).await;
        assert!(Path::new(&format!("/proc/{browser_pid}")).exists());
        assert_eq!(
            browser
                .screenshot("another-browser-thread", "another-browser-tab")
                .await
                .unwrap(),
            [255, 216, 255, 217]
        );
        browser.shutdown().await;
        gone(browser_pid).await;

        let (mut victim, mut oom_lease, _) = worker(&registry, "oom", false, false, true).await;
        let limits = Registry::ctl(
            &backend,
            &[
                "--user",
                "set-property",
                "--runtime",
                &oom_lease.unit,
                "MemoryHigh=infinity",
                "MemoryMax=33554432",
                "MemorySwapMax=0",
            ],
        )
        .await
        .unwrap();
        assert!(
            limits.status.success(),
            "{}",
            String::from_utf8_lossy(&limits.stderr)
        );
        let oom_state = registry
            .read_scope(&backend, &oom_lease.unit)
            .await
            .unwrap();
        let kernel_max = registry.read_group(&oom_state.group, "memory.max").await;
        assert_eq!(
            kernel_max.trim(),
            "33554432",
            "The private scope must have a real kernel memory cap"
        );
        victim
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"allocate\n")
            .await
            .unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_secs(15), victim.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(
            registry.oom_killed("oom").await,
            "Kernel OOM did not become a systemd OOM result"
        );
        assert!(registry.oom_killed("oom").await);
        oom_lease.stop().await;

        let (mut orphan, mut orphan_lease, _) =
            worker(&registry, "orphan", false, false, false).await;
        let other = Registry::new(&root.join("other-profile"));
        other.enable();
        let (mut unrelated, mut unrelated_lease, _) =
            worker(&other, "unrelated", false, false, false).await;
        let restarted = Registry::new(&root);
        restarted.enable();
        restarted.backend().await.unwrap();
        assert!(!tokio::time::timeout(Duration::from_secs(5), orphan.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        assert!(unrelated.try_wait().unwrap().is_none());
        for terminal in &terminals {
            assert!(
                Path::new(&format!("/proc/{}", terminal.pid)).exists(),
                "Restart cleanup killed a terminal outside the owned profile slice"
            );
        }
        finish(&mut unrelated).await;
        orphan_lease.stop().await;
        unrelated_lease.stop().await;
        crate::terminal::call(&fresh_terminal_state, "terminal.close", &api_input)
            .await
            .unwrap();
        gone(api_pid).await;
        for mut terminal in terminals {
            crate::pty::write_all(&terminal.master, b"exit()\n", Duration::from_secs(3))
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(5), terminal.child.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success()
            );
            gone(terminal.pid as u64).await;
        }
    }
}
