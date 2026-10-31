//! A small PTY layer over libc: `openpty` plus a session-leader child with its own process group.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    process::Stdio,
};
use tokio::io::unix::AsyncFd;

pub struct Pty {
    pub master: AsyncFd<OwnedFd>,
    pub child: tokio::process::Child,
    pub pid: i32,
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn set_flags(fd: RawFd, cloexec: bool, nonblock: bool) -> io::Result<()> {
    unsafe {
        if cloexec {
            let f = cvt(libc::fcntl(fd, libc::F_GETFD))?;
            cvt(libc::fcntl(fd, libc::F_SETFD, f | libc::FD_CLOEXEC))?;
        }
        if nonblock {
            let f = cvt(libc::fcntl(fd, libc::F_GETFL))?;
            cvt(libc::fcntl(fd, libc::F_SETFL, f | libc::O_NONBLOCK))?;
        }
    }
    Ok(())
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Starts `program` on a fresh PTY as session leader; its pid is also its process-group id.
pub fn spawn(
    program: &str,
    cwd: &Path,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
) -> io::Result<Pty> {
    let (mut m, mut s) = (0, 0);
    let ws = winsize(cols, rows);
    cvt(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null(), &ws) })?;
    let master = unsafe { OwnedFd::from_raw_fd(m) };
    let slave = unsafe { OwnedFd::from_raw_fd(s) };
    set_flags(m, true, true)?;
    set_flags(s, true, false)?;
    let mut command = tokio::process::Command::new(program);
    command
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave))
        .kill_on_drop(true);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child already reaped"))? as i32;
    // `OwnedFd` owns its descriptor for the registration's lifetime, which is what the lint asks for.
    #[allow(deprecated)]
    let master = AsyncFd::new(master)?;
    Ok(Pty { master, child, pid })
}

pub fn resize(master: &AsyncFd<OwnedFd>, cols: u16, rows: u16) -> io::Result<()> {
    let ws = winsize(cols, rows);
    cvt(unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) }).map(|_| ())
}

/// One non-blocking read; `Ok(0)` means the other side hung up (EIO on Linux counts as EOF).
pub fn read(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    if n >= 0 {
        return Ok(n as usize);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EIO) {
        Ok(0)
    } else {
        Err(e)
    }
}

pub fn write(master: &AsyncFd<OwnedFd>, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(master.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
    if n >= 0 {
        Ok(n as usize)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Writes all of `data`, waiting for the PTY to drain, but never longer than `limit`.
pub async fn write_all(
    master: &AsyncFd<OwnedFd>,
    mut data: &[u8],
    limit: std::time::Duration,
) -> io::Result<()> {
    let deadline = tokio::time::Instant::now() + limit;
    while !data.is_empty() {
        let mut guard = tokio::time::timeout_at(deadline, master.writable())
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        match guard.try_io(|fd| write(fd, data)) {
            Ok(Ok(n)) => data = &data[n..],
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => {}
        }
    }
    Ok(())
}

/// Signals the whole process group led by `pgid`.
pub fn signal_group(pgid: i32, signal: libc::c_int) {
    if pgid > 1 {
        unsafe {
            libc::kill(-pgid, signal);
        }
    }
}
