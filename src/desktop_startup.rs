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
    pub browser: Option<BrowserDescriptors>,
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

impl Startup {
    /// Read trusted launcher startup input exactly once before starting providers.
    ///
    /// # Safety
    /// The caller must exclusively own any inherited browser descriptors named
    /// by the envelope and transfer their ownership to this startup operation.
    pub async unsafe fn read(args: &[String]) -> io::Result<Self> {
        for name in ["--bootstrap-fd", "--mode"] {
            if args.iter().any(|arg| arg == name) && option(args, name).is_none() {
                return Err(io::Error::other(format!("Missing value for {name}")));
            }
        }
        let fd = option(args, "--bootstrap-fd")
            .map(str::to_owned)
            .or_else(|| std::env::var("T3CODE_BOOTSTRAP_FD").ok());
        let bootstrap_fd = fd.as_deref().and_then(|fd| fd.parse::<i32>().ok());
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
        // SAFETY: startup runs once, before providers or profile discovery; these
        // dedicated descriptors are transferred by the trusted launcher envelope.
        let browser = unsafe { BrowserDescriptors::adopt(envelope.as_ref(), bootstrap_fd) }?;
        Ok(Self {
            desktop,
            envelope,
            browser,
        })
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

/// Dedicated launcher channels, owned before any provider subprocess can inherit them.
pub struct BrowserDescriptors {
    input: std::os::fd::OwnedFd,
    control: std::os::fd::OwnedFd,
}
impl BrowserDescriptors {
    /// The caller must exclusively own both inherited descriptor numbers.
    unsafe fn adopt(
        envelope: Option<&Envelope>,
        bootstrap: Option<i32>,
    ) -> io::Result<Option<Self>> {
        use std::os::fd::FromRawFd;
        let field = |name| {
            envelope
                .and_then(|e| e.value(name))
                .and_then(serde_json::Value::as_f64)
        };
        let (input, control) = match (field("desktopBrowserFd"), field("desktopBrowserControlFd")) {
            (None, None) => return Ok(None),
            (Some(input), Some(control)) => (input, control),
            _ => {
                return Err(io::Error::other(
                    "Desktop browser requires both dedicated channels",
                ))
            }
        };
        if [input, control]
            .iter()
            .any(|fd| !fd.is_finite() || fd.fract() != 0.0 || *fd > i32::MAX as f64)
        {
            return Err(io::Error::other(
                "Invalid desktop browser descriptor number",
            ));
        }
        let input = input as i32;
        let control = control as i32;
        if input < 3
            || control < 3
            || input == control
            || Some(input) == bootstrap
            || Some(control) == bootstrap
        {
            return Err(io::Error::other(
                "Desktop browser descriptors must be distinct dedicated channels",
            ));
        }
        for fd in [input, control] {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: fstat initializes only the supplied stack buffer.
            if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let mode = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
            if mode != libc::S_IFIFO && mode != libc::S_IFSOCK {
                return Err(io::Error::other(
                    "Desktop browser descriptor is not a pipe or socket",
                ));
            }
            crate::desktop_bootstrap_fd::close_on_exec(fd)?;
        }
        // SAFETY: validated dedicated descriptors are exclusively transferred by startup.
        Ok(Some(Self {
            input: unsafe { std::os::fd::OwnedFd::from_raw_fd(input) },
            control: unsafe { std::os::fd::OwnedFd::from_raw_fd(control) },
        }))
    }
    /// Register the channels only in the final server runtime.
    pub fn into_channel(self) -> io::Result<crate::desktop_browser_channel::Channel> {
        crate::desktop_browser_channel::Channel::from_owned_descriptors(self.input, self.control)
    }
}
