//! A per-command Linux subreaper. Ownership follows the process tree, including
//! double-fork/setsid/env-clear daemons; closing the owner's pipe ends the scope.+//! The post-fork supervisor uses only stack memory and libc, never Rust locks.
use std::{
    ffi::CString,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, process::ExitStatusExt},
    },
    path::Path,
    process::ExitStatus,
};

pub struct Child {
    control: Option<OwnedFd>,
    result: tokio::sync::oneshot::Receiver<io::Result<ExitStatus>>,
    status: Option<ExitStatus>,
    finished: tokio::sync::watch::Receiver<bool>,
}
impl Child {
    pub(crate) fn completion(&self) -> tokio::sync::watch::Receiver<bool> {
        self.finished.clone()
    }
    pub(crate) fn stop(&mut self) {
        self.control.take();
    }
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let status = (&mut self.result)
            .await
            .map_err(|_| io::Error::other("Process scope waiter ended"))??;
        self.status = Some(status);
        Ok(status)
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}
fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::other("NUL in process input"))
}

/// Execute on `slave`, retaining no server descriptors in the supervisor. The
/// shell's pid is returned separately from the supervisor (for PTY job control).
pub(crate) fn spawn(
    program: &Path,
    args: &[std::ffi::OsString],
    cwd: &Path,
    env: &[(String, String)],
    slave: RawFd,
    adjust_oom: bool,
) -> io::Result<(Child, i32)> {
    let program = cstring(program.as_os_str().as_bytes())?;
    let cwd = cstring(cwd.as_os_str().as_bytes())?;
    let env = env
        .iter()
        .map(|(k, v)| cstring(format!("{k}={v}").as_bytes()))
        .collect::<io::Result<Vec<_>>>()?;
    let mut envp = env.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
    envp.push(std::ptr::null());
    let args = args
        .iter()
        .map(|arg| cstring(arg.as_bytes()))
        .collect::<io::Result<Vec<_>>>()?;
    let mut argv = std::iter::once(program.as_ptr())
        .chain(args.iter().map(|arg| arg.as_ptr()))
        .collect::<Vec<_>>();
    argv.push(std::ptr::null());
    let (control_read, control_write) = pipe()?;
    let (status_read, status_write) = pipe()?;
    let (error_read, error_write) = pipe()?;
    // Duplicate before fork so remapping cannot clobber a source descriptor.
    let sources = [
        slave,
        control_read.as_raw_fd(),
        status_write.as_raw_fd(),
        error_write.as_raw_fd(),
    ];
    let mut held = Vec::new();
    for fd in sources {
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
        if copy < 0 {
            return Err(io::Error::last_os_error());
        }
        held.push(unsafe { OwnedFd::from_raw_fd(copy) });
    }
    let fds = held.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>();
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            supervise(
                &fds,
                &program,
                &cwd,
                &argv,
                &envp,
                limit.rlim_cur,
                adjust_oom,
            );
        }
    }
    drop(held);
    drop(control_read);
    drop(status_write);
    drop(error_write);
    let mut status_file = std::fs::File::from(status_read);
    let mut child_pid = [0; 4];
    let admitted = status_file.read_exact(&mut child_pid);
    let mut error_file = std::fs::File::from(error_read);
    let mut errno = [0; 4];
    let error = match error_file.read(&mut errno) {
        Ok(0) => admitted.map(|_| ()),
        Ok(4) => Err(io::Error::from_raw_os_error(i32::from_ne_bytes(errno))),
        Ok(_) => Err(io::Error::other("Invalid process scope admission")),
        Err(e) => Err(e),
    };
    if let Err(e) = error {
        drop(control_write);
        unsafe {
            while libc::waitpid(pid, std::ptr::null_mut(), 0) < 0
                && *libc::__errno_location() == libc::EINTR
            {}
        }
        return Err(e);
    }
    let (send, result) = tokio::sync::oneshot::channel();
    let (finished, completion) = tokio::sync::watch::channel(false);
    // Own reaping independently of the async consumer, even if its future is dropped.
    let waiter = std::thread::Builder::new()
        .name("process-scope".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let mut status = [0; 4];
            let result = status_file
                .read_exact(&mut status)
                .map(|_| ExitStatus::from_raw(i32::from_ne_bytes(status)));
            unsafe {
                while libc::waitpid(pid, std::ptr::null_mut(), 0) < 0
                    && *libc::__errno_location() == libc::EINTR
                {}
            }
            if result.is_ok() {
                finished.send_replace(true);
            }
            let _ = send.send(result);
        });
    if let Err(error) = waiter {
        drop(control_write);
        unsafe {
            while libc::waitpid(pid, std::ptr::null_mut(), 0) < 0
                && *libc::__errno_location() == libc::EINTR
            {}
        }
        return Err(error);
    }
    Ok((
        Child {
            control: Some(control_write),
            result,
            status: None,
            finished: completion,
        },
        i32::from_ne_bytes(child_pid),
    ))
}

unsafe fn report_error() -> ! {
    let error = *libc::__errno_location();
    libc::write(5, (&error as *const i32).cast(), 4);
    libc::_exit(127)
}

unsafe fn supervise(
    fds: &[i32],
    program: &CString,
    cwd: &CString,
    argv: &[*const libc::c_char],
    envp: &[*const libc::c_char],
    limit: libc::rlim_t,
    adjust_oom: bool,
) -> ! {
    for target in 0..=2 {
        if libc::dup2(fds[0], target) < 0 {
            libc::_exit(127);
        }
    }
    for (source, target) in [(fds[1], 3), (fds[2], 4), (fds[3], 5)] {
        if libc::dup2(source, target) < 0 {
            libc::_exit(127);
        }
    }
    libc::fcntl(5, libc::F_SETFD, libc::FD_CLOEXEC);
    if libc::syscall(libc::SYS_close_range, 6u32, u32::MAX, 0u32) < 0 {
        for fd in 6..limit {
            libc::close(fd as i32);
        }
    }
    // Survive a launcher's signal to the owner's entire process group, so EOF
    // still lets us kill and reap the separate PTY session.
    if libc::setsid() < 0 || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) < 0 {
        report_error();
    }
    // The runtime's blocked/handled signals must not survive into the shell.
    let mut mask: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut mask);
    libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
    for signal in [
        libc::SIGCHLD,
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGPIPE,
    ] {
        libc::signal(signal, libc::SIG_DFL);
    }
    libc::sigaddset(&mut mask, libc::SIGCHLD);
    libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
    let child_events = libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
    if child_events < 0 {
        report_error();
    }
    let root = libc::fork();
    if root < 0 {
        report_error();
    }
    if root == 0 {
        libc::close(3);
        libc::close(4);
        libc::close(child_events);
        libc::sigemptyset(&mut mask);
        libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
        if libc::setsid() < 0
            || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0
            || libc::chdir(cwd.as_ptr()) < 0
        {
            report_error();
        }
        if adjust_oom {
            crate::agent_scope::raise_oom_score();
        }
        libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        report_error();
    }
    libc::write(4, (&root as *const i32).cast(), 4);
    libc::close(0);
    libc::close(1);
    libc::close(2);
    libc::close(5);
    let mut root_status = libc::SIGKILL;
    let mut stopping = false;
    loop {
        let mut status = 0;
        loop {
            let reaped = libc::waitpid(-1, &mut status, libc::WNOHANG);
            if reaped == root {
                root_status = status;
                stopping = true;
            }
            if reaped <= 0 {
                break;
            }
        }
        if stopping {
            // Each killed parent reparents its descendants to this subreaper.
            // Pids cannot be reused until our waitpid reaps them.
            kill_children();
            let reaped = libc::waitpid(-1, &mut status, libc::WNOHANG);
            if reaped == root {
                root_status = status;
            }
            if reaped > 0 {
                continue;
            }
            if reaped < 0 && *libc::__errno_location() == libc::ECHILD {
                break;
            }
        }
        let mut poll = [
            libc::pollfd {
                fd: if stopping { -1 } else { 3 },
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
            libc::pollfd {
                fd: child_events,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Sleep until the owner closes or a child exits; no idle polling.
        libc::poll(poll.as_mut_ptr(), 2, -1);
        if poll[0].revents != 0 {
            stopping = true;
        }
        if poll[1].revents != 0 {
            let mut signal = [0u8; 128];
            libc::read(child_events, signal.as_mut_ptr().cast(), signal.len());
        }
    }
    libc::write(4, (&root_status as *const i32).cast(), 4);
    libc::_exit(0)
}

unsafe fn kill_children() {
    // /proc/thread-self resolves the only thread in this forked supervisor.
    let fd = libc::open(
        c"/proc/thread-self/children".as_ptr(),
        libc::O_RDONLY | libc::O_CLOEXEC,
    );
    if fd < 0 {
        return;
    }
    let mut bytes = [0u8; 4096];
    let mut pid = 0i32;
    loop {
        let n = libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len());
        if n <= 0 {
            break;
        }
        for &c in &bytes[..n as usize] {
            if c.is_ascii_digit() {
                pid = pid.saturating_mul(10).saturating_add((c - b'0') as i32);
            } else {
                if pid > 1 {
                    libc::kill(pid, libc::SIGKILL);
                }
                pid = 0;
            }
        }
    }
    if pid > 1 {
        libc::kill(pid, libc::SIGKILL);
    }
    libc::close(fd);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    async fn until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn scope_reaps_daemons_after_cancel_owner_loss_and_shell_exit() {
        // Run the same owning Rust process in a subprocess, then SIGKILL it.
        // Only its pipe lifetime is available to the surviving subreaper.
        if let Some(dir) = std::env::var_os("O3_SCOPE_TEST_OWNER") {
            let dir = std::path::PathBuf::from(dir);
            let env = std::env::vars().collect::<Vec<_>>();
            let p = crate::pty::spawn_with_scope("/bin/sh", &dir, &env, 80, 24, true).unwrap();
            crate::pty::write_all(&p.master, b"python3 daemon.py\n", Duration::from_secs(1))
                .await
                .unwrap();
            std::future::pending::<()>().await;
            drop(p);
            return;
        }
        for mode in [
            "cancel",
            "owner-loss",
            "exit",
            "owner-sigkill",
            "owner-groupkill",
        ] {
            let dir = std::env::temp_dir()
                .join(format!("o3-scope-{}", crate::store::random_id().unwrap()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("daemon.py"),
                r#"import os, time
if os.fork():
    os._exit(0)
os.setsid()
if os.fork():
    os._exit(0)
os.environ.clear()
for fd in (0,1,2):
    os.close(fd)
open('daemon.pid','w').write(str(os.getpid()))
while True: time.sleep(1)
"#,
            )
            .unwrap();
            if matches!(mode, "owner-sigkill" | "owner-groupkill") {
                use std::os::unix::process::CommandExt;
                struct Owner(std::process::Child);
                impl Drop for Owner {
                    fn drop(&mut self) {
                        let _ = self.0.kill();
                        let _ = self.0.wait();
                    }
                }
                let mut owner=Owner(std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact","process_scope::tests::scope_reaps_daemons_after_cancel_owner_loss_and_shell_exit","--nocapture"])
                    .env("O3_SCOPE_TEST_OWNER",&dir).process_group(0).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap());
                until(|| {
                    std::fs::read_to_string(dir.join("daemon.pid"))
                        .is_ok_and(|s| s.parse::<i32>().is_ok())
                })
                .await;
                let daemon = std::fs::read_to_string(dir.join("daemon.pid"))
                    .unwrap()
                    .parse::<i32>()
                    .unwrap();
                if mode == "owner-groupkill" {
                    assert_eq!(
                        unsafe { libc::kill(-(owner.0.id() as i32), libc::SIGKILL) },
                        0
                    );
                } else {
                    owner.0.kill().unwrap();
                }
                owner.0.wait().unwrap();
                until(|| !Path::new(&format!("/proc/{daemon}")).exists()).await;
                std::fs::remove_dir_all(dir).unwrap();
                continue;
            }
            let env = std::env::vars().collect::<Vec<_>>();
            let mut p = crate::pty::spawn_with_scope("/bin/sh", &dir, &env, 80, 24, true).unwrap();
            crate::pty::write_all(
                &p.master,
                b"python3 daemon.py; echo ready\n",
                Duration::from_secs(1),
            )
            .await
            .unwrap();
            until(|| {
                dir.join("daemon.pid").exists()
                    && std::fs::read_to_string(dir.join("daemon.pid"))
                        .is_ok_and(|s| s.parse::<i32>().is_ok())
            })
            .await;
            let daemon = std::fs::read_to_string(dir.join("daemon.pid"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            assert!(Path::new(&format!("/proc/{daemon}")).exists());
            match mode {
                "cancel" => {
                    assert!(p.child.stop_scope());
                    assert!(tokio::time::timeout(Duration::from_secs(5), p.child.wait())
                        .await
                        .unwrap()
                        .is_ok());
                }
                "owner-loss" => drop(p),
                _ => {
                    crate::pty::write_all(&p.master, b"exit 7\n", Duration::from_secs(1))
                        .await
                        .unwrap();
                    assert_eq!(
                        tokio::time::timeout(Duration::from_secs(5), p.child.wait())
                            .await
                            .unwrap()
                            .unwrap()
                            .code(),
                        Some(7)
                    );
                }
            }
            until(|| !Path::new(&format!("/proc/{daemon}")).exists()).await;
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn scope_reports_exec_and_cwd_errors_without_a_live_child() {
        let env = std::env::vars().collect::<Vec<_>>();
        assert!(crate::pty::spawn_with_scope(
            "/bin/sh",
            Path::new("/o3-does-not-exist"),
            &env,
            80,
            24,
            true
        )
        .is_err());
        assert!(crate::pty::spawn_with_scope(
            "/o3-does-not-exist",
            Path::new("/"),
            &env,
            80,
            24,
            true
        )
        .is_err());
    }
}
