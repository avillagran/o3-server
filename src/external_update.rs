//! Conservative external install ownership. Unknown/manual and privileged installs are advice only.
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Plan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub command: String,
}
fn writable(path: &Path) -> bool {
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
    let Ok(m) = path.metadata() else { return false };
    // Root must not turn a system installation into an apparently user-owned one.
    if m.uid() != unsafe { libc::geteuid() } || m.uid() == 0 {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}
// A recognized layout alone cannot authorize executing an arbitrary helper.
fn installer(path: &Path, prefix: &Path, tool: &str) -> Result<PathBuf, &'static str> {
    use std::{
        os::{
            fd::AsRawFd,
            unix::{fs::MetadataExt, process::CommandExt},
        },
        process::Stdio,
        time::{Duration, Instant},
    };
    let real = path
        .canonicalize()
        .map_err(|_| "Installer is unavailable; update manually.")?;
    let m = real
        .metadata()
        .map_err(|_| "Installer metadata is unavailable.")?;
    let uid = unsafe { libc::geteuid() };
    let system = m.uid() == 0
        && ((tool == "npm"
            && (real == Path::new("/usr/bin/npm") || real.starts_with("/usr/share/nodejs/npm/")))
            || (tool == "brew" && real.starts_with("/opt/homebrew/")));
    if !m.is_file()
        || m.mode() & 0o111 == 0
        || m.mode() & 0o6022 != 0
        || !(system
            || m.uid() == uid
                && uid != 0
                && real.starts_with(
                    prefix
                        .canonicalize()
                        .map_err(|_| "Invalid installer prefix")?,
                ))
    {
        return Err(
            "Installer ownership, privilege bits or resolved location is unsafe; update manually.",
        );
    }
    // Validate all ancestors. Sticky shared scratch roots may contain user-owned
    // fixture/install prefixes, but never qualify a root-owned system helper.
    for parent in real.ancestors().skip(1) {
        let m = parent.metadata().map_err(|_| "Unsafe installer ancestry")?;
        if (m.uid() != 0 && m.uid() != uid)
            || (m.mode() & 0o022 != 0 && (system || m.mode() & 0o1000 == 0))
        {
            return Err("Installer directory can be replaced by another user; update manually.");
        }
    }
    struct ProbeGroup(u32);
    impl Drop for ProbeGroup {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
    let mut child = std::process::Command::new(&real)
        .arg("--version")
        .process_group(0)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", prefix)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot verify installer identity")?;
    let _group = ProbeGroup(child.id());
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Installer identity probe timed out");
            }
        }
    };
    let mut bytes = Vec::new();
    // A descendant may retain stdout after the direct child exits. Never wait
    // for that writer to close; the guard also stops the probe process group.
    let stdout = child.stdout.take().unwrap();
    let fd = stdout.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err("Cannot bound installer output");
        }
    }
    match stdout.take(1025).read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(_) => return Err("Invalid installer identity"),
    }
    let text = String::from_utf8_lossy(&bytes);
    let identity = match tool {
        "npm" => {
            text.trim().split('.').count() == 3
                && text
                    .trim()
                    .split('.')
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        }
        "brew" => text.starts_with("Homebrew "),
        "claude" => text.contains("Claude Code"),
        "opencode" => {
            text.trim().split('.').count() == 3
                && text
                    .trim()
                    .split('.')
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        }
        _ => false,
    };
    if !status.success() || bytes.len() > 1024 || !identity {
        return Err("Installer identity does not match the expected tool; update manually.");
    }
    Ok(real)
}
pub fn detect(binary: &Path, driver: &str) -> Result<Plan, &'static str> {
    let package = match driver {
        "codex" => "@openai/codex",
        "claudeAgent" => "@anthropic-ai/claude-code",
        "opencode" => "opencode-ai",
        "pi" => "@earendil-works/pi-coding-agent",
        "grok" => "@xai-official/grok",
        "cursor" => return Err("Cursor updates are managed by its original installer; run agent update in that installation's terminal, then refresh providers."),
        "muse" => return Err("Muse standalone binaries have no self-update command. Refresh the original official launcher with MUSE_SYNC_UPDATE=1, or replace the binary using its original installer, then refresh providers."),
        "antigravity" => return Err("Update Antigravity using its original application installer, then refresh providers."),
        "acpRegistry" => return Err("Update this ACP agent using its registry installation's original package manager, then refresh providers."),
        _ => return Err("Update this CLI using its original installer; its installation method is not supported."),
    };
    let real = crate::executable::resolve(binary)
        .ok_or("Install the CLI with its original package manager first.")?;
    for root in real.ancestors().skip(1).take(8) {
        let manifest = root.join("package.json");
        let metadata = std::fs::File::open(&manifest)
            .ok()
            .and_then(|f| {
                let mut b = Vec::new();
                f.take(65537).read_to_end(&mut b).ok()?;
                Some(b)
            })
            .filter(|b| b.len() <= 65536)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let package = metadata
            .as_ref()
            .and_then(|m| m["name"].as_str())
            .filter(|name| *name == package || driver == "opencode" && *name == "@opencode/cli");
        let Some(package) = package else {
            continue;
        };
        // Only standard npm global layout is recognized. Never convert a local npm install.
        let modules = if package.starts_with('@') {
            root.parent().and_then(Path::parent)
        } else {
            root.parent()
        };
        let Some(modules) = modules else {
            continue;
        };
        if modules.file_name().is_none_or(|s| s != "node_modules")
            || modules
                .parent()
                .and_then(Path::file_name)
                .is_none_or(|s| s != "lib")
        {
            continue;
        }
        let prefix = modules.parent().and_then(Path::parent).unwrap();
        let npm = installer(&prefix.join("bin/npm"), prefix, "npm")?;
        if ![prefix, modules, root, &real].iter().all(|p| writable(p)) {
            return Err("Update it with npm install --global using the original prefix in a privileged terminal; o3 never elevates privileges.");
        }
        return Ok(Plan {
            program: npm,
            args: vec![
                "install".into(),
                "--global".into(),
                "--prefix".into(),
                prefix.to_string_lossy().into(),
                "--ignore-scripts".into(),
                "--no-audit".into(),
                "--no-fund".into(),
                format!("{package}@latest"),
            ],
            command: format!("npm install --global {package}@latest (original prefix)"),
        });
    }
    // Require both a recognized keg layout and its local installation receipt.
    // npm above wins when Homebrew's Node owns a global npm package.
    for keg in real.ancestors().skip(1).take(8) {
        let Some(rack) = keg.parent() else { continue };
        let Some(store) = rack.parent() else { continue };
        let cask = store.file_name().is_some_and(|n| n == "Caskroom");
        if !cask && store.file_name().is_none_or(|n| n != "Cellar") {
            continue;
        }
        let prefix = store.parent().ok_or("Homebrew prefix is unavailable")?;
        let name = rack
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid Homebrew package name")?;
        let expected = match driver {
            "codex" => "codex",
            "claudeAgent" => "claude-code",
            "opencode" => "opencode",
            "pi" => "pi",
            _ => "",
        };
        if name != expected {
            return Err("Homebrew package identity does not match the provider; use the original package manager manually.");
        }
        let receipt = keg.join("INSTALL_RECEIPT.json");
        // Cask receipts differ across releases. Do not guess from an arbitrary path.
        if cask {
            return Err("Update this cask with brew upgrade --cask using its original cask name; cask receipt verification is unavailable.");
        }
        let metadata = read_json(&receipt).ok_or("Homebrew receipt is missing or invalid; repair or update the original formula manually.")?;
        if metadata["source"]["tap"] != "homebrew/core"
            || !metadata["source"]["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(&format!("/{name}.rb")))
        {
            return Err(
                "Homebrew receipt does not establish the original formula; update manually.",
            );
        }
        let brew = installer(&prefix.join("bin/brew"), prefix, "brew")?;
        if ![prefix, store, rack, keg, &receipt, &real, &brew]
            .iter()
            .all(|p| writable(p))
        {
            return Err("Homebrew installation needs its owner's permissions; run brew upgrade in that owner's terminal. o3 never elevates privileges.");
        }
        return Ok(Plan {
            program: brew,
            args: vec!["upgrade".into(), "--formula".into(), name.into()],
            command: format!("brew upgrade --formula {name}"),
        });
    }
    if driver == "opencode" && real.ends_with(".opencode/bin/opencode") {
        let bin = real.parent().unwrap();
        if ![&real, bin, bin.parent().unwrap()]
            .iter()
            .all(|p| writable(p))
        {
            return Err("OpenCode native installation needs its owner's permissions; run opencode upgrade in that owner's terminal.");
        }
        return Ok(Plan {
            program: installer(&real, bin.parent().unwrap(), "opencode")?,
            args: vec!["upgrade".into()],
            command: "opencode upgrade (native installer)".into(),
        });
    }
    if driver == "claudeAgent" {
        // Claude's native installer links .local/bin/claude to a versioned binary.
        // A similarly named standalone binary elsewhere is insufficient evidence.
        if let Some(versions) = real
            .parent()
            .filter(|p| p.ends_with(".local/share/claude/versions"))
        {
            let local = versions
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .unwrap();
            let link = local.join("bin/claude");
            let version = real.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let valid_version = version.split('.').count() >= 3
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
                && version.starts_with(|c: char| c.is_ascii_digit());
            if valid_version
                && link
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                && link.canonicalize().is_ok_and(|p| p == real)
            {
                if ![local, &local.join("bin"), versions, &real]
                    .iter()
                    .all(|p| writable(p))
                {
                    return Err("Claude native installation needs its owner's permissions; run claude update in that owner's terminal. o3 never elevates privileges.");
                }
                return Ok(Plan {
                    program: installer(&link, local, "claude")?,
                    args: vec!["update".into()],
                    command: "claude update (native installer)".into(),
                });
            }
        }
    }
    Err("Installation ownership is unknown. Update this CLI with its original installer (npm, brew or system package manager), then refresh providers.")
}
fn read_json(path: &Path) -> Option<Value> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(65537)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 65536 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    #[test]
    fn privileged_fake_npm_is_never_offered() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!("o3-unsafe-installer-{}", std::process::id()));
        // Fixture trees are retained for private container teardown.
        let pkg = root.join("lib/node_modules/@openai/codex");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(pkg.join("package.json"), r#"{"name":"@openai/codex"}"#).unwrap();
        let cli = pkg.join("codex");
        std::fs::write(&cli, "#!/bin/sh\necho codex-cli 1.0.0\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let privileged = root.join("fixture-privileged-helper");
        std::fs::write(&privileged, "#!/bin/sh\nexit 91\n").unwrap();
        std::fs::set_permissions(&privileged, std::fs::Permissions::from_mode(0o4755))
            .unwrap();
        symlink(&privileged, root.join("bin/npm")).unwrap();
        assert!(
            detect(&cli, "codex").is_err(),
            "privileged helper advertised as npm"
        );
        // Fixture trees are retained for private container teardown.
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn probe_does_not_wait_for_descendant_stdout() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = std::env::temp_dir().join(format!("o3-probe-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let npm = root.join("npm");
        std::fs::write(&npm, "#!/bin/sh\n/bin/sleep 3 &\necho 10.1.0\n").unwrap();
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A fork in a concurrent test can briefly hold the just-written script open for
        // writing (ETXTBSY); retry that transient failure and time the successful probe.
        let mut elapsed = None;
        for _ in 0..20 {
            let start = std::time::Instant::now();
            if installer(&npm, &root, "npm").is_ok() {
                elapsed = Some(start.elapsed());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let elapsed = elapsed.expect("probe never succeeded");
        // Fixture trees are retained for private container teardown.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "probe waited for inherited stdout"
        );
    }
}
