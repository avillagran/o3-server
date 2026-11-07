//! Read the first launcher-envelope line without exposing its contents in diagnostics.
use crate::desktop_bootstrap::Envelope;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Stat,
    Open,
    Read,
    Decode,
}

#[derive(Debug)]
pub struct Error {
    pub fd: i32,
    pub stage: Stage,
}
impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match self.stage {
            Stage::Stat => "stat bootstrap file descriptor",
            Stage::Open => "open bootstrap input stream for file descriptor",
            Stage::Read => "read bootstrap envelope from file descriptor",
            Stage::Decode => "decode bootstrap envelope from file descriptor",
        };
        write!(out, "Failed to {action} {}.", self.fd)
    }
}
impl std::error::Error for Error {}

/// The descriptor is trusted launcher input. There is no arbitrary envelope-size limit.
#[cfg(unix)]
pub async fn read(fd: i32, timeout: Duration) -> Result<Option<Envelope>, Error> {
    tokio::task::spawn_blocking(move || read_blocking(fd, timeout))
        .await
        .map_err(|_| Error {
            fd,
            stage: Stage::Read,
        })?
}

#[cfg(unix)]
fn unavailable(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EBADF | libc::ENOENT))
}

#[cfg(unix)]
pub(crate) fn close_on_exec(fd: i32) -> std::io::Result<()> {
    // SAFETY: these fcntl operations only query/update flags on the supplied descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Inherited launcher descriptors must stay in this process, including when a
    // regular-file bootstrap is reopened without advancing the original offset.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn read_blocking(fd: i32, timeout: Duration) -> Result<Option<Envelope>, Error> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let error = |stage| Error { fd, stage };
    // Node rejects negative descriptors before calling fstat (ERR_OUT_OF_RANGE).
    if fd < 0 {
        return Err(error(Stage::Stat));
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat only initializes the supplied stat buffer; invalid descriptors are rejected.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
        let cause = std::io::Error::last_os_error();
        return if unavailable(&cause) {
            Ok(None)
        } else {
            Err(error(Stage::Stat))
        };
    }
    close_on_exec(fd).map_err(|_| error(Stage::Open))?;
    let directory = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    let stream = match std::fs::File::open(format!("{directory}/{fd}")) {
        Ok(file) => file,
        Err(cause)
            if matches!(
                cause.raw_os_error(),
                Some(libc::ENXIO | libc::EINVAL | libc::EPERM | libc::EACCES)
            ) =>
        {
            // SAFETY: native direct-stream fallback takes ownership of this launcher descriptor.
            unsafe { std::fs::File::from_raw_fd(fd) }
        }
        Err(_) => return Err(error(Stage::Open)),
    };
    let deadline = std::time::Instant::now() + timeout;
    let mut line = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let mut poll = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let wait = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        // SAFETY: one initialized pollfd is valid for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, wait) };
        if ready == 0 {
            return Ok(None);
        }
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error(Stage::Read));
        }
        let mut chunk = [0u8; 4096];
        // SAFETY: stream owns a valid descriptor and chunk is writable for its full length.
        let count =
            unsafe { libc::read(stream.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len()) };
        if count < 0 {
            let cause = std::io::Error::last_os_error();
            if cause.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return if unavailable(&cause) {
                Ok(None)
            } else {
                Err(error(Stage::Read))
            };
        }
        if count == 0 {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let chunk = &chunk[..count as usize];
        if let Some(end) = chunk.iter().position(|byte| matches!(byte, b'\r' | b'\n')) {
            line.extend_from_slice(&chunk[..end]);
            break;
        }
        line.extend_from_slice(chunk);
    }
    // Node's UTF-8 stream replaces malformed raw bytes before JSON decoding.
    let value =
        serde_json::from_str(&String::from_utf8_lossy(&line)).map_err(|_| error(Stage::Decode))?;
    Envelope::decode(value)
        .map(Some)
        .map_err(|_| error(Stage::Decode))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[tokio::test]
    async fn original_bootstrap_file_stays_open_but_is_not_inherited_by_subprocesses() {
        use std::os::unix::fs::MetadataExt;
        crate::test_sandbox::assert_isolated();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!(
            "o3-bootstrap-private-{:032x}",
            u128::from_ne_bytes(nonce)
        ));
        std::fs::write(&path,serde_json::json!({"mode":"desktop","noBrowser":true,"port":3773,"host":"127.0.0.1","desktopBootstrapToken":"synthetic-private-seed","tailscaleServeEnabled":false,"tailscaleServePort":443}).to_string()).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let fd = file.as_raw_fd();
        let metadata = file.metadata().unwrap();
        // SAFETY: this is our private fixture's descriptor; simulate the inherited launcher state.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) },
            0
        );
        let probe = || {
            std::process::Command::new("python3").args(["-c", "import os,sys\ntry:\n s=os.fstat(int(sys.argv[1]))\nexcept OSError as error:\n sys.exit(0 if error.errno==9 else 2)\nsys.exit(1 if (s.st_dev,s.st_ino)==(int(sys.argv[2]),int(sys.argv[3])) else 0)"])
            .args([fd.to_string(),metadata.dev().to_string(),metadata.ino().to_string()]).status().unwrap().code()
        };
        assert_eq!(
            probe(),
            Some(1),
            "fixture did not reproduce inherited input"
        );
        assert!(read(fd, Duration::from_secs(1)).await.unwrap().is_some());
        assert!(
            file.metadata().is_ok(),
            "launcher descriptor must remain open in the server"
        );
        assert_eq!(
            probe(),
            Some(0),
            "subprocess inherited private launcher input"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn native_descriptor_reader_oracle() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/desktop-bootstrap-4288.json"
        ))
        .unwrap();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let directory =
            std::env::temp_dir().join(format!("o3-bootstrap-{:032x}", u128::from_ne_bytes(nonce)));
        std::fs::create_dir(&directory).unwrap();
        for case in fixture["fdCases"].as_array().unwrap() {
            let label = case["label"].as_str().unwrap();
            let result = if let Some(fd) = case["fd"].as_i64() {
                read(fd as i32, Duration::from_millis(100)).await
            } else if label == "closed" {
                let path = directory.join(label);
                std::fs::write(&path, []).unwrap();
                let file = std::fs::File::open(&path).unwrap();
                let fd = file.as_raw_fd();
                drop(file);
                let result = read(fd, Duration::from_millis(100)).await;
                std::fs::remove_file(path).unwrap();
                result
            } else if label == "timeout" {
                let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
                // The native direct-socket fallback owns and closes this descriptor.
                read(socket.into_raw_fd(), Duration::from_millis(25)).await
            } else {
                let bytes: Vec<u8> = serde_json::from_value(case["bytes"].clone()).unwrap();
                let path = directory.join(label);
                std::fs::write(&path, bytes).unwrap();
                let file = std::fs::File::open(&path).unwrap();
                let result = read(file.as_raw_fd(), Duration::from_millis(100)).await;
                // Reopened file descriptors leave the launcher's original descriptor valid.
                assert!(file.metadata().is_ok());
                std::fs::remove_file(path).unwrap();
                result
            };
            let actual = match result {
                Ok(Some(envelope)) => {
                    let names = [
                        "mode",
                        "noBrowser",
                        "port",
                        "host",
                        "desktopBootstrapToken",
                        "tailscaleServeEnabled",
                        "tailscaleServePort",
                    ];
                    let mut value = serde_json::Map::new();
                    for name in names {
                        value.insert(name.into(), envelope.value(name).unwrap().clone());
                    }
                    json!({"value":value})
                }
                Ok(None) => json!({"none":true}),
                Err(error) => {
                    let tag = match error.stage {
                        Stage::Stat => "BootstrapFdStatError",
                        Stage::Open => "BootstrapInputStreamOpenError",
                        Stage::Read => "BootstrapEnvelopeReadError",
                        Stage::Decode => "BootstrapEnvelopeDecodeError",
                    };
                    json!({"error":tag})
                }
            };
            assert_eq!(actual, case["expected"], "{label}");
        }
        std::fs::remove_dir(directory).unwrap();
    }
}
