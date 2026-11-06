//! The desktop's service-install surface. Only invoked by the explicit CLI command.
use std::{io, path::Path};
fn quote(s: &str) -> io::Result<String> {
    if s.contains(['\n', '\r', '\0']) {
        return Err(io::Error::other("newline in service path"));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    ))
}
pub fn unit(exe: &Path, base: &Path) -> io::Result<String> {
    Ok(format!("[Unit]\nDescription=T3 compatible o3-server\nAfter=network.target\n\n[Service]\nType=simple\nEnvironment={}\nExecStart={} serve --base-dir {}\nRestart=on-failure\nKillMode=mixed\n\n[Install]\nWantedBy=default.target\n",quote(&format!("T3CODE_HOME={}",base.display()))?,quote(&exe.to_string_lossy())?,quote(&base.to_string_lossy())?))
}
pub fn install(base: &Path, dry_run: bool) -> io::Result<()> {
    let content = unit(&std::env::current_exe()?, base)?;
    if dry_run {
        print!("{content}");
        return Ok(());
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let dir = config.join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("t3code.service");
    if let Ok(old) = std::fs::read_to_string(&path) {
        if !old.contains("Description=T3 compatible o3-server") {
            return Err(io::Error::other(
                "refusing to replace a service owned by another installation",
            ));
        }
    }
    std::fs::write(path, content)?;
    for args in [
        &["--user", "daemon-reload"][..],
        &["--user", "enable", "--now", "t3code.service"][..],
    ] {
        if !std::process::Command::new("systemctl")
            .args(args)
            .status()?
            .success()
        {
            return Err(io::Error::other("systemctl failed"));
        }
    }
    Ok(())
}
