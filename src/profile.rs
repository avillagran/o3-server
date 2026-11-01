//! Profile discovery and owned, atomic runtime registration.
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
pub fn base_dir(args: &[String]) -> io::Result<PathBuf> {
    let raw = args
        .windows(2)
        .find(|w| w[0] == "--base-dir")
        .map(|w| w[1].clone())
        .or_else(|| {
            std::env::var("T3CODE_HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
        });
    let path = raw
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".t3"));
    if !path.is_absolute() {
        return Err(io::Error::other("base directory must be absolute"));
    }
    Ok(path)
}
pub fn atomic_json(path: &Path, value: &Value) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    writeln!(f, "{value}")?;
    f.sync_all()?;
    fs::rename(tmp, path)
}
/// Commands must not leave o3 files in a profile a different live server owns.
pub fn refuse_foreign_live(dir: &Path) -> io::Result<()> {
    let Ok(bytes) = fs::read(dir.join("server-runtime.json")) else {
        return Ok(());
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(());
    };
    if v["serverVersion"]
        .as_str()
        .is_some_and(|version| version.starts_with("o3-"))
    {
        return Ok(());
    }
    refuse_live(dir)
}
pub fn refuse_live(dir: &Path) -> io::Result<()> {
    if let Ok(bytes) = fs::read(dir.join("server-runtime.json")) {
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(pid) = v["pid"].as_u64() {
                if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
                    if !stat
                        .rsplit_once(") ")
                        .is_some_and(|(_, s)| s.starts_with('Z'))
                    {
                        return Err(io::Error::other(format!(
                            "profile already owned by live PID {pid}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
pub struct RuntimeGuard(PathBuf);
impl RuntimeGuard {
    pub fn write(dir: &Path, addr: std::net::SocketAddr) -> io::Result<Self> {
        let host = if addr.ip().is_unspecified() {
            "127.0.0.1".to_owned()
        } else {
            addr.ip().to_string()
        };
        let origin = if host.contains(':') {
            format!("http://[{host}]:{}", addr.port())
        } else {
            format!("http://{host}:{}", addr.port())
        };
        let path = dir.join("server-runtime.json");
        atomic_json(
            &path,
            &json!({"version":1,"pid":std::process::id(),"host":host,"port":addr.port(),"origin":origin,"startedAt":crate::store::iso(crate::store::now_ms()),"serverVersion":crate::rpc::SERVER_VERSION}),
        )?;
        Ok(Self(path))
    }
}
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if fs::read(&self.0)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["pid"] == std::process::id())
        {
            let _ = fs::remove_file(&self.0);
        }
    }
}

/// An OS lock closes the check/register race between two o3 processes.
pub fn ownership(dir: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(dir)?;
    refuse_live(dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("o3-runtime.lock"))?;
    lock.try_lock()
        .map_err(|e| io::Error::other(format!("profile is locked: {e}")))?;
    refuse_live(dir)?;
    Ok(lock)
}
