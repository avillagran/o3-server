//! Pinned device-tool inventory. No tree is deleted and symlinks are never traversed.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub enum Tool {
    Hub,
    Agent,
}
impl Tool {
    pub fn package(self) -> &'static str {
        match self {
            Self::Hub => "expo-device-hub",
            Self::Agent => "agent-device",
        }
    }
    pub fn version(self) -> &'static str {
        match self {
            Self::Hub => "0.19.1",
            Self::Agent => "0.21.31",
        }
    }
    pub fn entry(self) -> &'static str {
        match self {
            Self::Hub => "dist/server/cli.mjs",
            Self::Agent => "bin/agent-device.mjs",
        }
    }
}
#[derive(Clone, Debug)]
pub struct InstalledTool {
    pub root: PathBuf,
    pub entry: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: u32,
    package: String,
    version: String,
    files: BTreeMap<String, String>,
}
const MANIFEST: &str = ".o3-device-install.json";
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
/// Reject symlink components before reading an owned installation tree.
pub(crate) fn real_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid("Device tool directories must be absolute"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        match part {
            Component::RootDir | Component::Normal(_) => current.push(part),
            _ => return Err(invalid("Invalid device tool directory")),
        }
        let meta = fs::symlink_metadata(&current)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(invalid(
                "Device tool directory contains a link or non-directory",
            ));
        }
    }
    Ok(())
}
fn read_regular(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > cap {
        return Err(invalid(
            "Device tool file is not regular or exceeds its size limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(invalid("Device tool file grew beyond its size limit"));
    }
    Ok(bytes)
}
fn inventory(root: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                return Err(invalid(
                    "Device tool inventory cannot contain symbolic links",
                ));
            }
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            if !meta.is_file() {
                return Err(invalid("Device tool inventory contains a special file"));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| invalid("Invalid inventory path"))?
                .to_str()
                .ok_or_else(|| invalid("Non-UTF8 device tool path"))?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if relative == MANIFEST {
                continue;
            }
            total = total
                .checked_add(meta.len())
                .ok_or_else(|| invalid("Device tool inventory overflow"))?;
            if files.len() >= 100_000 || total > 4 * 1024 * 1024 * 1024 {
                return Err(invalid("Device tool inventory exceeds its bounded size"));
            }
            use std::{io::Read, os::unix::fs::OpenOptionsExt};
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&path)?;
            if !file.metadata()?.is_file() {
                return Err(invalid("Device tool inventory file changed type"));
            }
            let mut digest = Sha256::new();
            let mut input = file.take(meta.len() + 1);
            if io::copy(&mut input, &mut digest)? != meta.len() {
                return Err(invalid("Device tool inventory file changed size"));
            }
            files.insert(relative, format!("{:x}", digest.finalize()));
        }
    }
    Ok(files)
}
fn validate_package(root: &Path, tool: Tool) -> io::Result<PathBuf> {
    let package = root.join("node_modules").join(tool.package());
    real_directory(&package)?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&read_regular(&package.join("package.json"), 1024 * 1024)?)
            .map_err(io::Error::other)?;
    if metadata["name"] != tool.package() || metadata["version"] != tool.version() {
        return Err(invalid(
            "Device tool package does not match the pinned name and version",
        ));
    }
    let entry = package.join(tool.entry());
    if !fs::symlink_metadata(&entry)?.is_file() {
        return Err(invalid("Device tool entry is not a regular file"));
    }
    Ok(entry)
}
impl InstalledTool {
    /// A successful npm exit or an exposed entry alone is never an installation sentinel.
    pub fn inspect(root: &Path, tool: Tool) -> io::Result<Self> {
        real_directory(root)?;
        let manifest_path = root.join(MANIFEST);
        if !fs::symlink_metadata(&manifest_path)?.is_file() {
            return Err(invalid("Device tool manifest is not a regular file"));
        }
        let bytes = read_regular(&manifest_path, 32 * 1024 * 1024)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(invalid("Device tool manifest is too large"));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if manifest.format != 1
            || manifest.package != tool.package()
            || manifest.version != tool.version()
        {
            return Err(invalid("Device tool manifest pin mismatch"));
        }
        if inventory(root)? != manifest.files {
            return Err(invalid("Device tool installation inventory changed"));
        }
        let entry = validate_package(root, tool)?;
        Ok(Self {
            root: root.to_owned(),
            entry,
        })
    }
    /// Seal only a private, exclusively owned staging directory after acquisition succeeds.
    /// The caller must retain its acquisition lock until publishing by atomic rename.
    pub fn seal_staging(root: &Path, tool: Tool) -> io::Result<Self> {
        real_directory(root)?;
        let files = inventory(root)?;
        let entry = validate_package(root, tool)?;
        let manifest = Manifest {
            format: 1,
            package: tool.package().into(),
            version: tool.version().into(),
            files,
        };
        use std::io::Write;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(MANIFEST))?;
        output.write_all(&serde_json::to_vec(&manifest).map_err(io::Error::other)?)?;
        output.sync_all()?;
        Ok(Self {
            root: root.to_owned(),
            entry,
        })
    }
}
/// Installation argv never names a mutable dist-tag and never executes package lifecycle scripts.
pub fn npm_acquisition_args(staging: &Path, tool: Tool) -> Vec<String> {
    vec![
        "install".into(),
        "--prefix".into(),
        staging.to_string_lossy().into_owned(),
        "--no-bin-links".into(),
        "--ignore-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--omit=dev".into(),
        "--package-lock=true".into(),
        "--".into(),
        format!("{}@{}", tool.package(), tool.version()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        crate::test_sandbox::assert_isolated();
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "device-tool-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let package = root.join("node_modules/expo-device-hub");
        fs::create_dir_all(package.join("dist/server")).unwrap();
        fs::write(
            package.join("package.json"),
            br#"{"name":"expo-device-hub","version":"0.19.1"}"#,
        )
        .unwrap();
        fs::write(
            package.join("dist/server/cli.mjs"),
            "// Private fixture, never executed\n",
        )
        .unwrap();
        root
    }
    #[test]
    fn entry_presence_is_not_an_install_and_complete_inventory_is_immutable() {
        let root = fixture();
        assert!(InstalledTool::inspect(&root, Tool::Hub).is_err());
        InstalledTool::seal_staging(&root, Tool::Hub).unwrap();
        InstalledTool::inspect(&root, Tool::Hub).unwrap();
        fs::write(
            root.join("node_modules/expo-device-hub/new-code.mjs"),
            "unexpected",
        )
        .unwrap();
        assert!(InstalledTool::inspect(&root, Tool::Hub).is_err());
        let root = fixture();
        InstalledTool::seal_staging(&root, Tool::Hub).unwrap();
        fs::write(
            root.join("node_modules/expo-device-hub/dist/server/cli.mjs"),
            "modified",
        )
        .unwrap();
        assert!(InstalledTool::inspect(&root, Tool::Hub).is_err());
    }
    #[test]
    fn symbolic_links_are_rejected_without_reading_their_targets() {
        let root = fixture();
        let target = root.join("private-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), "fixture data").unwrap();
        std::os::unix::fs::symlink(&target, root.join("link")).unwrap();
        assert!(InstalledTool::seal_staging(&root, Tool::Hub).is_err());
        assert_eq!(
            fs::read_to_string(target.join("kept")).unwrap(),
            "fixture data"
        );
    }
}
