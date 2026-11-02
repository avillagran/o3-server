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
        if metadata.as_ref().is_none_or(|m| m["name"] != package) {
            continue;
        }
        // Only standard npm global layout is recognized. Never convert a local npm install.
        let Some(modules) = root.parent().and_then(Path::parent) else {
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
    // Homebrew and system packages require administrator-managed action until ownership
    // can be established from their package database, never infer ownership from PATH alone.
    if real
        .components()
        .any(|c| c.as_os_str() == "Cellar" || c.as_os_str() == "Caskroom")
    {
        return Err("Update it with brew upgrade using the original formula or cask; automatic Homebrew ownership verification is unavailable.");
    }
    Err("Installation ownership is unknown. Update this CLI with its original installer (npm, brew or system package manager), then refresh providers.")
}
