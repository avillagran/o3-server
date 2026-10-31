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
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    c
}

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
    // Repository-local config must not be able to run programs for read-only inspection.
    let mut full = vec!["-c", "core.fsmonitor=false", "-c", "core.quotepath=false"];
    full.extend_from_slice(args);
    Box::pin(run("git", cwd, &full, opts, env)).await
}

pub async fn run(
    program: &str,
    cwd: &Path,
    args: &[&str],
    opts: Opts,
    env: &[(&str, &Path)],
) -> Result<Out, RunError> {
    let mut command = base_command(program, cwd, args);
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    // Boxed: debug builds give big futures big frames, and sockets run on small stacks.
    let work = Box::pin(async {
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
    match tokio::time::timeout(opts.timeout, work).await {
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
