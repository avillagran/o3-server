//! Argv-only subprocess runner for git: no shell, no stdin, bounded time and output.
//! Repository mutations are serialized per repository (git common dir).
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::OwnedMutexGuard,
};

pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(180);
pub const OUTPUT_CAP: usize = 4 * 1024 * 1024;
const STDERR_CAP: usize = 256 * 1024;

#[derive(Debug)]
pub struct Out {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// Stdout hit the cap; the process was stopped and `stdout` is a prefix.
    pub truncated: bool,
}
impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}
#[derive(Debug)]
pub enum RunError {
    Spawn(String),
    Timeout(u64),
}

#[derive(Clone, Copy)]
pub struct Opts {
    /// Zero has no deadline; cancellation still stops the child process group.
    pub timeout: Duration,
    pub cap: usize,
    /// When true a capped stdout is an `Out { truncated: true }`, otherwise a failure.
    pub allow_truncate: bool,
}
impl Opts {
    pub const READ: Opts = Opts {
        timeout: READ_TIMEOUT,
        cap: OUTPUT_CAP,
        allow_truncate: false,
    };
    pub const WRITE: Opts = Opts {
        timeout: WRITE_TIMEOUT,
        cap: OUTPUT_CAP,
        allow_truncate: false,
    };
    pub const NETWORK: Opts = Opts {
        timeout: NETWORK_TIMEOUT,
        cap: OUTPUT_CAP,
        allow_truncate: false,
    };
}

async fn read_capped<R: AsyncRead + Unpin>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => return (out, false),
            Ok(n) => {
                let room = cap.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
                if n > room {
                    return (out, true);
                }
            }
        }
    }
}

fn base_command(program: &str, cwd: &Path, args: &[&str]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    c.process_group(0);
    c.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    c
}

/// Prepended to every git invocation. Repository-local config must not run programs during
/// read-only inspection, and user config must not change parsed output: diff.relative drops
/// changes outside a subdirectory workspace, suppressBlankEmpty strips blank context lines,
/// showSignature and colour inject text (upstream f2527c2c0, 35af84977, 581544212, 50106efdd).
pub(crate) const SAFE_CONFIG: [&str; 14] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.quotepath=false",
    "-c",
    "diff.relative=false",
    "-c",
    "diff.suppressBlankEmpty=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "color.ui=false",
    "-c",
    "color.diff=false",
];

/// Runs `git <args>` in `cwd`. `args` are passed as argv, never through a shell.
pub async fn git(cwd: &Path, args: &[&str], opts: Opts) -> Result<Out, RunError> {
    Box::pin(git_env(cwd, args, opts, &[])).await
}

/// `git` with extra environment variables (for example a temporary `GIT_INDEX_FILE`).
pub async fn git_env(
    cwd: &Path,
    args: &[&str],
    opts: Opts,
    env: &[(&str, &Path)],
) -> Result<Out, RunError> {
    let mut full = SAFE_CONFIG.to_vec();
    full.extend_from_slice(args);
    Box::pin(run("git", cwd, &full, opts, env)).await
}

// Kill hooks and transport descendants too when a caller cancels the future.
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

pub async fn run(
    program: &str,
    cwd: &Path,
    args: &[&str],
    opts: Opts,
    env: &[(&str, &Path)],
) -> Result<Out, RunError> {
    run_inner(program, cwd, args, opts, env, false, None).await
}

/// External installers get a fixed interpreter search path and no ambient runtime injection.
pub async fn run_installer(
    program: &str,
    cwd: &Path,
    args: &[&str],
    opts: Opts,
) -> Result<Out, RunError> {
    run_inner(program, cwd, args, opts, &[], true, None).await
}
pub(crate) async fn git_controlled(
    cwd: &Path,
    args: &[&str],
    opts: Opts,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<Out, RunError> {
    let args = SAFE_CONFIG
        .iter()
        .copied()
        .chain(args.iter().copied())
        .collect::<Vec<_>>();
    run_inner("git", cwd, &args, opts, &[], false, Some(cancel)).await
}
async fn run_inner(
    program: &str,
    cwd: &Path,
    args: &[&str],
    opts: Opts,
    env: &[(&str, &Path)],
    installer: bool,
    mut cancel: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<Out, RunError> {
    let mut command = base_command(program, cwd, args);
    if installer {
        let home = std::env::var_os("HOME");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C");
        if let Some(home) = home {
            command.env("HOME", home);
        }
    }
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let _group = ProcessGroup(child.id().expect("spawned child"));
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    // Boxed: debug builds give big futures big frames, and sockets run on small stacks.
    let mut work = Box::pin(async {
        let ((out, cut), (err, _)) = tokio::join!(
            read_capped(stdout, opts.cap),
            read_capped(stderr, STDERR_CAP)
        );
        if cut {
            let _ = child.start_kill();
        }
        let status = child.wait().await.ok();
        (out, cut, err, status)
    });
    // Checkout and submodule preparation follow upstream's cancellable, unbounded
    // lifetime. Zero opts out of a deadline while retaining process-group cleanup.
    let deadline = async {
        if opts.timeout.is_zero() {
            std::future::pending::<()>().await;
        } else {
            tokio::time::sleep(opts.timeout).await;
        }
    };
    let cancelled = async {
        if let Some(cancel) = cancel.as_mut() {
            while !*cancel.borrow() {
                if cancel.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        } else {
            std::future::pending::<()>().await;
        }
    };
    let mut was_cancelled = false;
    let outcome = tokio::select! {result=&mut work=>Ok(result),_=deadline=>Err(()),_=cancelled=>{was_cancelled=true;Err(())}};
    drop(work);
    if outcome.is_err() {
        unsafe {
            libc::kill(-(_group.0 as i32), libc::SIGKILL);
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    match outcome {
        Ok((stdout, cut, err, status)) => Ok(Out {
            code: if cut {
                if opts.allow_truncate {
                    Some(0)
                } else {
                    None
                }
            } else {
                status.and_then(|s| s.code())
            },
            stdout,
            stderr: String::from_utf8_lossy(&err).trim().to_owned(),
            truncated: cut,
        }),
        // `kill_on_drop` stops the child when `work` is dropped.
        Err(_) if was_cancelled => Err(RunError::Spawn("Workspace preparation cancelled".into())),
        Err(_) => Err(RunError::Timeout(opts.timeout.as_millis() as u64)),
    }
}

type Locks = Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;
fn locks() -> &'static Locks {
    static LOCKS: OnceLock<Locks> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

/// Serializes mutations of the repository that contains `cwd` (all of its worktrees).
pub async fn lock_repo(cwd: &Path) -> OwnedMutexGuard<()> {
    let key = match git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Opts::READ,
    )
    .await
    {
        Ok(o) if o.ok() => PathBuf::from(o.text().trim()),
        _ => std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf()),
    };
    let mutex = {
        let mut map = locks().lock().unwrap();
        // Idle entries are dropped so the table stays proportional to active repositories.
        map.retain(|k, m| *k == key || Arc::strong_count(m) > 1);
        map.entry(key).or_default().clone()
    };
    mutex.lock_owned().await
}

/// Cleanup must skip, rather than wait for, an in-flight repository mutation.
pub(crate) fn try_lock_common_dir(key: PathBuf) -> Option<OwnedMutexGuard<()>> {
    let mutex = {
        let mut map = locks().lock().unwrap();
        map.retain(|k, m| *k == key || Arc::strong_count(m) > 1);
        map.entry(key).or_default().clone()
    };
    mutex.try_lock_owned().ok()
}

#[cfg(test)]
mod diff_relative_tests {
    #[tokio::test]
    async fn user_git_config_does_not_change_parsed_output() {
        let root = std::env::temp_dir().join(format!(
            "o3-relative-{}",
            crate::store::random_id().unwrap()
        ));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let sh = |dir: &std::path::Path, args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap()
                .success());
        };
        sh(&root, &["init", "-q"]);
        std::fs::write(root.join("out.txt"), "a\n").unwrap();
        std::fs::write(sub.join("in.txt"), "a\n").unwrap();
        sh(&root, &["add", "."]);
        sh(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "i",
            ],
        );
        sh(&root, &["config", "diff.relative", "true"]);
        std::fs::write(root.join("out.txt"), "b\n").unwrap();
        let out = super::git(&sub, &["diff", "--name-only", "HEAD"], super::Opts::READ)
            .await
            .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(out.text().contains("out.txt"), "{}", out.text());
    }
}
