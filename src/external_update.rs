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
        let npm = crate::executable::resolve(&prefix.join("bin/npm"))
            .ok_or("Update it with npm install --global using the npm belonging to this prefix; that npm executable is unavailable.")?;
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
        let brew = crate::executable::resolve(&prefix.join("bin/brew")).ok_or(
            "Use brew upgrade with the original formula; this prefix's brew is unavailable.",
        )?;
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
            program: real,
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
                    program: link,
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
