//! Owned nonblocking pipe/socket transport; no descriptor paths are reopened.
use super::Channel;
use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::io::{unix::AsyncFd, AsyncRead, AsyncWrite, ReadBuf};

struct Descriptor(AsyncFd<OwnedFd>);
impl Descriptor {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        crate::desktop_bootstrap_fd::close_on_exec(fd.as_raw_fd())?;
        // SAFETY: fcntl only reads/updates flags on this owned descriptor.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OwnedFd keeps the same open descriptor until AsyncFd drops it.
        // This wrapper never exposes, replaces or closes that descriptor early.
        unsafe { AsyncFd::register(fd) }
            .map(Self)
            .map_err(io::Error::from)
    }
}
impl AsyncRead for Descriptor {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut readiness = ready!(self.0.poll_read_ready(cx))?;
            let result = readiness.try_io(|fd| {
                let bytes = output.initialize_unfilled();
                loop {
                    // SAFETY: output owns a writable buffer, bounded by its remaining capacity.
                    let count = unsafe {
                        libc::read(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len())
                    };
                    if count >= 0 {
                        return Ok(count as usize);
                    }
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            });
            match result {
                Ok(Ok(count)) => {
                    output.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => continue,
            }
        }
    }
}
impl AsyncWrite for Descriptor {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut readiness = ready!(self.0.poll_write_ready(cx))?;
            let result = readiness.try_io(|fd| loop {
                // SAFETY: bytes is readable for its supplied length and fd is owned.
                let count =
                    unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
                if count >= 0 {
                    return Ok(count as usize);
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            });
            match result {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl Channel {
    /// Adopt two already-owned launcher pipe/socket descriptors. Both become
    /// nonblocking and close-on-exec; neither is reopened through a filesystem path.
    /// This is not startup activation: CDP/preview consumers are still required.
    pub fn from_owned_descriptors(input: OwnedFd, control: OwnedFd) -> io::Result<Self> {
        let input = Descriptor::new(input)?;
        let control = Descriptor::new(control)?;
        Ok(Self::start(input, control))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_browser::TabKey;
    use std::{os::unix::net::UnixStream, time::Duration};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn real_anonymous_pipes_support_partial_io_and_broken_control() {
        use std::os::fd::FromRawFd;
        fn pipe() -> (OwnedFd, OwnedFd) {
            let mut descriptors = [-1; 2];
            // SAFETY: pipe2 initializes exactly two descriptors on success.
            assert_eq!(
                unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
                0
            );
            // SAFETY: both descriptors were freshly created and ownership is unique.
            unsafe {
                (
                    OwnedFd::from_raw_fd(descriptors[0]),
                    OwnedFd::from_raw_fd(descriptors[1]),
                )
            }
        }
        let (input, launcher) = pipe();
        let (commands, control) = pipe();
        let mut launcher = Descriptor::new(launcher).unwrap();
        let commands = Descriptor::new(commands).unwrap();
        let channel = Channel::from_owned_descriptors(input, control).unwrap();
        for byte in b"{\"type\":\"attached\",\"threadId\":\"thread\",\"tabId\":\"tab\"}\n" {
            launcher.write_all(&[*byte]).await.unwrap();
        }
        let key = TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        };
        assert!(channel.await_attached(&key, Duration::from_secs(1)).await);
        drop(commands);
        tokio::time::timeout(Duration::from_secs(1), channel.turn("thread", false))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn real_owned_socket_pairs_exchange_events_commands_and_close() {
        let (launcher, input) = UnixStream::pair().unwrap();
        let (control, commands) = UnixStream::pair().unwrap();
        launcher.set_nonblocking(true).unwrap();
        commands.set_nonblocking(true).unwrap();
        let mut launcher = tokio::net::UnixStream::from_std(launcher).unwrap();
        let mut commands = tokio::net::UnixStream::from_std(commands).unwrap();
        let channel = Channel::from_owned_descriptors(input.into(), control.into()).unwrap();
        launcher
            .write_all(b"{\"type\":\"attached\",\"threadId\":\"thread\",\"tabId\":\"tab\"}\n")
            .await
            .unwrap();
        let key = TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        };
        assert!(channel.await_attached(&key, Duration::from_secs(1)).await);
        channel.turn("thread", true).await;
        let mut bytes = [0; 256];
        let count = commands.read(&mut bytes).await.unwrap();
        let command: serde_json::Value = serde_json::from_slice(&bytes[..count]).unwrap();
        assert_eq!(
            command,
            serde_json::json!({"type":"turn","threadId":"thread","active":true})
        );
        drop(channel);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), commands.read(&mut bytes))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}
