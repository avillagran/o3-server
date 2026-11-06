//! Private launcher bootstrap intake, before profile discovery or persistent state creation.
use crate::desktop_bootstrap::Envelope;
use std::{io, time::Duration};

/// A launcher may close its log pipe while remaining alive; this must not end its server.
pub fn log_stderr(message: impl std::fmt::Display) -> io::Result<()> {
    use io::Write;
    match writeln!(io::stderr().lock(), "{message}") {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

pub fn auth_policy(host: Option<&str>, desktop: bool) -> crate::managed_limits::AuthPolicy {
    let web = crate::managed_clients::web_auth_policy(host);
    if desktop && matches!(web, crate::managed_limits::AuthPolicy::LocalBrowser) {
        crate::managed_limits::AuthPolicy::Desktop
    } else {
        web
    }
}

pub struct Startup {
    pub desktop: bool,
    pub envelope: Option<Envelope>,
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

impl Startup {
    pub async fn read(args: &[String]) -> io::Result<Self> {
        for name in ["--bootstrap-fd", "--mode"] {
            if args.iter().any(|arg| arg == name) && option(args, name).is_none() {
                return Err(io::Error::other(format!("Missing value for {name}")));
            }
        }
        let fd = option(args, "--bootstrap-fd")
            .map(str::to_owned)
            .or_else(|| std::env::var("T3CODE_BOOTSTRAP_FD").ok());
        let envelope = if let Some(fd) = fd {
            let fd = fd
                .parse::<i32>()
                .map_err(|_| io::Error::other("Invalid bootstrap file descriptor"))?;
            crate::desktop_bootstrap_fd::read(fd, crate::desktop_bootstrap::READ_TIMEOUT)
                .await
                .map_err(io::Error::other)?
        } else {
            None
        };
        let mode = option(args, "--mode")
            .map(str::to_owned)
            .or_else(|| std::env::var("T3CODE_MODE").ok())
            .or_else(|| envelope.as_ref().map(|_| "desktop".into()))
            .unwrap_or_else(|| "web".into());
        let desktop = match mode.as_str() {
            "desktop" => true,
            "web" => false,
            _ => return Err(io::Error::other("Invalid server mode")),
        };
        if let Some(envelope) = &envelope {
            // These channels require dedicated consumers before real desktop launchers may use them.
            for name in [
                "desktopTelemetryFd",
                "desktopTelemetryControlFd",
                "desktopBrowserFd",
                "desktopBrowserControlFd",
                "resourceMonitorPath",
                "otlpTracesUrl",
                "otlpMetricsUrl",
                "otlpLogsUrl",
            ] {
                if envelope.value(name).is_some() {
                    return Err(io::Error::other(format!(
                        "Desktop bootstrap consumer is not implemented: {name}"
                    )));
                }
            }
            if envelope
                .value("tailscaleServeEnabled")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return Err(io::Error::other(
                    "Desktop bootstrap Tailscale Serve is not implemented",
                ));
            }
        }
        Ok(Self { desktop, envelope })
    }
    pub fn parent_watch_enabled(&self, parent: i32) -> bool {
        self.desktop && parent > 1 && self.envelope.is_some()
    }
}

pub fn parent_id() -> i32 {
    // SAFETY: getppid has no arguments or memory side effects.
    unsafe { libc::getppid() }
}

/// Any parent change, including adoption by a subreaper, ends a launcher-owned server.
pub async fn wait_for_parent_change(parent: i32, enabled: bool) {
    if !enabled {
        std::future::pending::<()>().await;
        return;
    }
    loop {
        if parent_id() != parent {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
