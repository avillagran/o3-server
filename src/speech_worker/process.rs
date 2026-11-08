//! A private process group whose leader stays unreaped until its final fence.
use std::{io, os::unix::process::CommandExt, process::Stdio, time::Duration};
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    Confirmed { graceful: bool },
    Unknown,
}

pub struct OwnedProcess {
    child: Child,
    group: i32,
    wait_started: bool,
}

impl OwnedProcess {
    /// The caller supplies only an admitted executable and sealed arguments.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // A new session prevents unrelated processes from joining this group.
        unsafe {
            command.as_std_mut().pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 1)
            .ok_or_else(|| io::Error::other("Invalid owned speech process identity"))?;
        Ok(Self {
            child,
            group,
            wait_started: false,
        })
    }

    pub fn id(&self) -> u32 {
        self.group as u32
    }

    pub fn take_pipes(
        &mut self,
    ) -> io::Result<(tokio::process::ChildStdin, tokio::process::ChildStdout)> {
        let input = self
            .child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Speech input unavailable"))?;
        let output = self
            .child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Speech output unavailable"))?;
        Ok((input, output))
    }

    fn signal(&self, signal: i32) -> bool {
        // ESRCH is also a confirmed fence: no member of this unreaped group exists.
        (unsafe { libc::kill(-self.group, signal) == 0 })
            || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    /// `grace` is an independently observed EOF/close acknowledgement, never a
    /// wait/try_wait on the leader. The terminal group fence precedes its reap.
    pub async fn close(
        self,
        grace: impl std::future::Future<Output = bool>,
        deadline: Duration,
    ) -> Cleanup {
        self.close_with(grace, deadline, |group, signal| {
            (unsafe { libc::kill(-group, signal) == 0 })
                || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        })
        .await
    }

    async fn close_with(
        mut self,
        grace: impl std::future::Future<Output = bool>,
        deadline: Duration,
        mut signal: impl FnMut(i32, i32) -> bool,
    ) -> Cleanup {
        let terminated = signal(self.group, libc::SIGTERM);
        let graceful = terminated && tokio::time::timeout(deadline, grace).await.unwrap_or(false);
        let fenced = signal(self.group, libc::SIGKILL);
        // Once wait is polled, the PID could be reused. Drop must never signal it.
        self.wait_started = true;
        match tokio::time::timeout(deadline, self.child.wait()).await {
            Ok(Ok(status)) if fenced => Cleanup::Confirmed {
                graceful: graceful && status.success(),
            },
            _ => Cleanup::Unknown,
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !self.wait_started {
            // Tokio will arrange leader reaping after this synchronous fence.
            self.signal(libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_group_fence_receipt_stays_unknown_even_after_leader_reap() {
        crate::test_sandbox::assert_isolated();
        let node = std::env::var("O3_ADMITTED_NODE").unwrap();
        let mut command = Command::new(node);
        command.args(["-e", "setInterval(()=>{},1000)"]).env_clear();
        let process = OwnedProcess::spawn(&mut command).unwrap();
        let group = process.group;
        let result = process
            .close_with(async { false }, Duration::from_secs(2), |pid, signal| {
                assert_eq!(pid, group);
                // Explicit fault injection: perform real owned cleanup, but withhold
                // its receipt, as an OS/control failure would. Never claim ready.
                let _ = unsafe { libc::kill(-pid, signal) };
                false
            })
            .await;
        assert_eq!(result, Cleanup::Unknown);
        assert_eq!(unsafe { libc::kill(group, 0) }, -1);
    }
}
