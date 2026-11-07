//! Owner-only admission of regular files. No wire deserialization or PATH lookup.
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

pub struct SealedFile {
    path: PathBuf,
    sha256: String,
    bytes: u64,
    lease: File,
}

impl SealedFile {
    pub fn admit(path: &Path, expected_sha256: &str, expected_bytes: u64) -> io::Result<Self> {
        if !path.is_absolute()
            || path.to_str().is_none()
            || expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(io::Error::other("Invalid owned speech receipt"));
        }
        let mut directory = File::open("/")?;
        let parts = path.components().skip(1).collect::<Vec<_>>();
        let mut result = None;
        for (index, component) in parts.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(io::Error::other("Noncanonical speech path"));
            };
            use std::os::unix::ffi::OsStrExt;
            let name = CString::new(name.as_bytes())
                .map_err(|_| io::Error::other("Invalid speech path"))?;
            let last = index + 1 == parts.len();
            let flags = libc::O_RDONLY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | if last { 0 } else { libc::O_DIRECTORY };
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if last {
                result = Some(file);
            } else {
                directory = file;
            }
        }
        let mut lease = result.ok_or_else(|| io::Error::other("Missing speech file"))?;
        let before = lease.metadata()?;
        if !before.is_file() || before.len() != expected_bytes {
            return Err(io::Error::other("Changed owned speech file"));
        }
        let mut hasher = Sha256::new();
        let mut block = vec![0; 64 * 1024];
        let mut count = 0u64;
        loop {
            let length = lease.read(&mut block)?;
            if length == 0 {
                break;
            }
            count = count
                .checked_add(length as u64)
                .ok_or_else(|| io::Error::other("Oversized speech file"))?;
            if count > expected_bytes {
                return Err(io::Error::other("Changed speech file"));
            }
            hasher.update(&block[..length]);
        }
        let after = lease.metadata()?;
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let sha256 = format!("{:x}", hasher.finalize());
        if count != expected_bytes
            || identity(&before) != identity(&after)
            || sha256 != expected_sha256
        {
            return Err(io::Error::other("Changed owned speech receipt"));
        }
        Ok(Self {
            path: path.to_owned(),
            sha256,
            bytes: count,
            lease,
        })
    }

    pub(super) fn snapshot(&self, destination: &Path, executable: bool) -> io::Result<Self> {
        let _parent = directory(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("Missing snapshot parent"))?,
        )?;
        let mut input = self.lease.try_clone()?;
        input.seek(SeekFrom::Start(0))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if executable { 0o500 } else { 0o400 })
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(destination)?;
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut block = vec![0; 64 * 1024];
        loop {
            let size = input.read(&mut block)?;
            if size == 0 {
                break;
            }
            count += size as u64;
            if count > self.bytes {
                return Err(io::Error::other("Changed speech snapshot"));
            }
            hash.update(&block[..size]);
            output.write_all(&block[..size])?;
        }
        output.sync_all()?;
        if count != self.bytes || format!("{:x}", hash.finalize()) != self.sha256 {
            return Err(io::Error::other("Changed speech snapshot"));
        }
        Self::admit(destination, &self.sha256, self.bytes)
    }

    pub fn receipt(&self) -> serde_json::Value {
        serde_json::json!({"path": self.path, "sha256": self.sha256, "bytes": self.bytes.to_string()})
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn is_private_immutable_executable(&self) -> io::Result<bool> {
        let info = self.lease.metadata()?;
        Ok(info.uid() == unsafe { libc::geteuid() }
            && info.nlink() == 1
            && info.mode() & 0o222 == 0
            && self.is_executable()?)
    }
    pub fn is_executable(&self) -> io::Result<bool> {
        let mode = self.lease.metadata()?.mode();
        Ok(mode & 0o111 != 0 && mode & 0o6000 == 0)
    }
}

/// Exact package manifest, including every transitive runtime dependency. Nothing
/// outside this graph is copied into the private engine directory.
pub struct EnginePackage {
    entry: PathBuf,
    files: Vec<(PathBuf, SealedFile)>,
}
impl EnginePackage {
    pub fn admit(entry: PathBuf, files: Vec<(PathBuf, SealedFile)>) -> io::Result<Self> {
        let safe = |path: &Path| {
            !path.as_os_str().is_empty()
                && path.to_str().is_some()
                && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        };
        let mut seen = std::collections::BTreeSet::new();
        if files.is_empty() || files.len() > 100_000 || !safe(&entry) {
            return Err(io::Error::other("Invalid engine package graph"));
        }
        for (path, _) in &files {
            if !safe(path) || !seen.insert(path.clone()) {
                return Err(io::Error::other("Invalid engine package graph"));
            }
        }
        if !seen.contains(&entry)
            || seen
                .iter()
                .any(|path| path.ancestors().skip(1).any(|parent| seen.contains(parent)))
        {
            return Err(io::Error::other("Invalid engine package entry"));
        }
        Ok(Self { entry, files })
    }
    pub(super) fn paths(&self, root: &Path) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|(relative, _)| root.join(relative))
            .collect()
    }
    pub(super) fn snapshot(&self, root: &Path) -> io::Result<SealedFile> {
        std::fs::DirBuilder::new().mode(0o700).create(root)?;
        let mut directories = std::collections::BTreeSet::new();
        for (relative, _) in &self.files {
            for parent in relative
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                directories.insert(parent.to_owned());
            }
        }
        for relative in directories {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(relative))?;
        }
        let mut entry = None;
        for (relative, file) in &self.files {
            let copy = file.snapshot(&root.join(relative), file.is_executable()?)?;
            if *relative == self.entry {
                entry = Some(copy);
            }
        }
        entry.ok_or_else(|| io::Error::other("Missing engine entry"))
    }
}

pub(super) fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::other("Nonabsolute speech directory"));
    }
    let mut file = File::open("/")?;
    for component in path.components().skip(1) {
        let Component::Normal(name) = component else {
            return Err(io::Error::other("Noncanonical speech directory"));
        };
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::other("Invalid speech directory"))?;
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        let trusted = metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() };
        if !trusted || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0) {
            return Err(io::Error::other("Untrusted speech destination ancestor"));
        }
    }
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "Speech destination parent is not privately owned",
        ));
    }
    Ok(file)
}
