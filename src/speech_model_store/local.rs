//! Owned local files and HTTP effects for the source-pinned ModelStore kernel.
use super::*;
use crate::cleanup::{FileTree, Identity, Kind, RealRemoval, Removal, RemovalPlan};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Component,
};
use tokio::io::AsyncWriteExt;
fn platform(error: impl fmt::Display) -> Error {
    Error::Platform(error.to_string())
}
fn component(name: &std::ffi::OsStr) -> io::Result<CString> {
    let path = Path::new(name);
    let mut parts = path.components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(io::Error::other("Invalid owned model filename"));
    }
    CString::new(name.as_bytes()).map_err(io::Error::other)
}
fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::other("Model directory must be absolute"));
    }
    let mut file = File::open("/")?;
    for part in path.components().skip(1) {
        let Component::Normal(name) = part else {
            return Err(io::Error::other("Noncanonical model directory"));
        };
        let name = component(name)?;
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
        let info = file.metadata()?;
        if (info.uid() != 0 && info.uid() != unsafe { libc::geteuid() })
            || (info.mode() & 0o022 != 0 && info.mode() & 0o1000 == 0)
        {
            return Err(io::Error::other("Untrusted model directory ancestor"));
        }
    }
    let info = file.metadata()?;
    if info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o022 != 0 {
        return Err(io::Error::other("Model directory is not privately owned"));
    }
    Ok(file)
}
struct Directory {
    path: PathBuf,
    file: File,
    identity: Identity,
}
impl Directory {
    fn open(path: PathBuf) -> io::Result<Self> {
        let file = directory(&path)?;
        let info = file.metadata()?;
        Ok(Self {
            path,
            file,
            identity: Identity {
                device: info.dev(),
                inode: info.ino(),
                kind: Kind::Directory,
            },
        })
    }
    fn child(&self, name: &str) -> io::Result<Self> {
        self.validate()?;
        let name = component(std::ffi::OsStr::new(name))?;
        let result = unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o700) };
        if result != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
            return Err(io::Error::last_os_error());
        }
        Self::open(self.path.join(std::ffi::OsStr::from_bytes(name.as_bytes())))
    }
    fn child_new(&self, name: &str) -> io::Result<Self> {
        self.validate()?;
        let name = component(std::ffi::OsStr::new(name))?;
        if unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Self::open(self.path.join(std::ffi::OsStr::from_bytes(name.as_bytes())))
    }
    fn validate(&self) -> io::Result<()> {
        let file = directory(&self.path)?;
        let info = file.metadata()?;
        if info.dev() != self.identity.device || info.ino() != self.identity.inode {
            return Err(io::Error::other("Owned model directory changed identity"));
        }
        Ok(())
    }
    fn open_file(&self, path: &Path, flags: i32, mode: u32) -> io::Result<File> {
        self.validate()?;
        let relative = path.strip_prefix(&self.path).map_err(io::Error::other)?;
        let mut parts = relative.components().peekable();
        let mut parent = self.file.try_clone()?;
        while let Some(part) = parts.next() {
            let Component::Normal(name) = part else {
                return Err(io::Error::other("Model path escaped owned directory"));
            };
            let name = component(name)?;
            let actual = if parts.peek().is_some() {
                libc::O_RDONLY | libc::O_DIRECTORY
            } else {
                flags | libc::O_NONBLOCK
            };
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    actual | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    mode,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            parent = unsafe { File::from_raw_fd(fd) };
        }
        Ok(parent)
    }
    fn info(&self, path: &Path) -> Option<std::fs::Metadata> {
        #[cfg(target_os = "linux")]
        let flags = libc::O_PATH;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::O_RDONLY;
        self.open_file(path, flags, 0).ok()?.metadata().ok()
    }
}
pub struct Local {
    root: Arc<Directory>,
    staging: Arc<Directory>,
    removal: Arc<dyn Removal>,
}
impl Local {
    /// Requires the already owned private server state directory. Native legacy
    /// partials remain untouched: a filename pattern cannot prove ownership.
    pub fn open(state: &Path) -> Result<Self, Error> {
        Self::with_removal(state, Arc::new(RealRemoval))
    }
    pub fn with_removal(state: &Path, removal: Arc<dyn Removal>) -> Result<Self, Error> {
        let state = Directory::open(state.to_owned()).map_err(platform)?;
        let speech = state.child("speech").map_err(platform)?;
        let root = Arc::new(speech.child("models").map_err(platform)?);
        let staging = Arc::new(root.child(".o3-staging").map_err(platform)?);
        Ok(Self {
            root,
            staging,
            removal,
        })
    }
}
fn marker_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".verified");
    name.into()
}
fn file_info(info: std::fs::Metadata) -> FileInfo {
    let millis =
        (info.mtime() as f64 * 1000.0 + info.mtime_nsec() as f64 / 1_000_000.0).trunc() as i64;
    FileInfo {
        regular: info.is_file(),
        size: info.len(),
        mtime_ms: Some(millis),
        device: info.dev(),
        inode: Some(info.ino()),
    }
}
impl Backend for Local {
    fn directory(&self) -> &Path {
        &self.root.path
    }
    fn info(&self, path: &Path) -> Option<FileInfo> {
        self.root.info(path).map(file_info)
    }
    fn read_marker(&self, path: &Path) -> Option<String> {
        let mut file = self
            .root
            .open_file(&marker_path(path), libc::O_RDONLY, 0)
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(1025)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= 1024)
            .then(|| String::from_utf8(bytes).ok())
            .flatten()
    }
    fn write_marker(&self, path: &Path, marker: &str) -> Result<(), Error> {
        let path = marker_path(path);
        let identity = |info: &std::fs::Metadata| {
            (
                info.dev(),
                info.ino(),
                info.is_file(),
                info.len(),
                info.mtime(),
                info.mtime_nsec(),
                info.ctime(),
                info.ctime_nsec(),
            )
        };
        let before = self.root.info(&path);
        if before.as_ref().is_some_and(|info| !info.is_file()) {
            return Err(platform("Model marker is not a regular file"));
        }
        let id = crate::store::random_id().map_err(platform)?;
        let directory = self.staging.child_new(&id).map_err(platform)?;
        let mut stage = OwnedStage {
            data_path: directory.path.join("marker.new"),
            directory,
            root: self.root.clone(),
            staging: self.staging.clone(),
            file: None,
            published: false,
        };
        let mut file = stage
            .directory
            .open_file(
                &stage.data_path,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
            .map_err(platform)?;
        file.write_all(marker.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(platform)?;
        let current = self.root.info(&path);
        if before.as_ref().map(identity) != current.as_ref().map(identity) {
            return Err(platform("Model marker changed before publication"));
        }
        self.root.validate().map_err(platform)?;
        stage.directory.validate().map_err(platform)?;
        let relative = path.strip_prefix(&self.root.path).map_err(platform)?;
        let name = component(relative.as_os_str()).map_err(platform)?;
        let temporary = CString::new("marker.new").expect("Static marker filename");
        if unsafe {
            libc::renameat(
                stage.directory.file.as_raw_fd(),
                temporary.as_ptr(),
                self.root.file.as_raw_fd(),
                name.as_ptr(),
            )
        } != 0
        {
            return Err(platform(io::Error::last_os_error()));
        }
        stage.published = true;
        Ok(())
    }
    fn digest<'a>(&'a self, path: &'a Path) -> Task<'a, Option<String>> {
        let file = self.root.open_file(path, libc::O_RDONLY, 0);
        Box::pin(async move {
            let mut file = file.ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            tokio::task::spawn_blocking(move || {
                let mut hash = Sha256::new();
                let mut bytes = vec![0; 64 * 1024];
                loop {
                    let count = file.read(&mut bytes).ok()?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&bytes[..count]);
                }
                Some(format!("{:x}", hash.finalize()))
            })
            .await
            .ok()
            .flatten()
        })
    }
    fn stage<'a>(&'a self, model: &'a Model) -> Task<'a, Result<Box<dyn Transfer>, Error>> {
        Box::pin(async move {
            let id = crate::store::random_id().map_err(platform)?;
            let directory = self.staging.child_new(&id).map_err(platform)?;
            let owned = OwnedStage {
                data_path: directory.path.join("data.gguf"),
                directory,
                root: self.root.clone(),
                staging: self.staging.clone(),
                file: None,
                published: false,
            };
            let receipt=serde_json::to_vec(&serde_json::json!({"owner":"o3-speech-transfer-v1","transferId":id,"modelId":model.id(),"filename":model.filename(),"size":model.size(),"sha256":model.sha256(),"directoryDevice":owned.directory.identity.device.to_string(),"directoryInode":owned.directory.identity.inode.to_string()})).map_err(platform)?;
            let mut manifest = owned
                .directory
                .open_file(
                    &owned.directory.path.join("manifest.json"),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )
                .map_err(platform)?;
            manifest
                .write_all(&receipt)
                .and_then(|_| manifest.sync_all())
                .map_err(platform)?;
            Ok(Box::new(owned) as Box<dyn Transfer>)
        })
    }
    fn remove<'a>(&'a self, model: &'a Model) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            self.root.validate().map_err(platform)?;
            let marker = format!("{}.verified", model.filename());
            let plan = RemovalPlan::prepare_files(
                &self.root.path,
                &[model.filename(), &marker],
                &FileTree,
            )
            .map_err(platform)?;
            self.removal.execute(&plan).map_err(platform)
        })
    }
}
struct OwnedStage {
    data_path: PathBuf,
    directory: Directory,
    root: Arc<Directory>,
    staging: Arc<Directory>,
    file: Option<tokio::fs::File>,
    published: bool,
}
impl Drop for OwnedStage {
    fn drop(&mut self) {
        // Retain the whole exactly-owned entry. Unknown or replaced entries are
        // never traversed or purged, including after partial writes/cancellation.
        if self.root.validate().is_ok() && self.staging.validate().is_ok() {
            if let Ok(mut terminal) = self.directory.open_file(
                &self.directory.path.join("terminal.json"),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            ) {
                let phase = if self.published {
                    br#"{"published":true}"#.as_slice()
                } else {
                    br#"{"published":false}"#.as_slice()
                };
                let _ = terminal.write_all(phase);
            }
            let _ = crate::cleanup::quarantine_owned(&self.directory.path, self.directory.identity);
        }
    }
}
impl Transfer for OwnedStage {
    fn path(&self) -> &Path {
        &self.data_path
    }
    fn offset(&self) -> u64 {
        self.directory
            .info(&self.data_path)
            .map_or(0, |info| info.len())
    }
    fn restart(&mut self) -> Task<'_, Result<(), Error>> {
        Box::pin(async move {
            use tokio::io::AsyncSeekExt;
            if self.file.is_none() {
                let file = self
                    .directory
                    .open_file(
                        &self.data_path,
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                        0o600,
                    )
                    .map_err(platform)?;
                self.file = Some(tokio::fs::File::from_std(file));
            }
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| platform("Model staging file unavailable"))?;
            file.set_len(0).await.map_err(platform)?;
            file.seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(platform)?;
            Ok(())
        })
    }
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            if self.file.is_none() {
                let file = self
                    .directory
                    .open_file(
                        &self.data_path,
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                        0o600,
                    )
                    .map_err(platform)?;
                self.file = Some(tokio::fs::File::from_std(file));
            }
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| platform("Model staging file unavailable"))?;
            file.write_all(bytes).await.map_err(platform)?;
            file.flush().await.map_err(platform)
        })
    }
    fn publish<'a>(&'a mut self, path: &'a Path) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            self.root.validate().map_err(platform)?;
            self.directory.validate().map_err(platform)?;
            if self.root.info(path).is_some_and(|info| !info.is_file()) {
                return Err(platform("Model destination is not a regular file"));
            }
            let relative = path.strip_prefix(&self.root.path).map_err(platform)?;
            let name = component(relative.as_os_str()).map_err(platform)?;
            let data = CString::new("data.gguf").expect("Static owned data filename");
            let result = unsafe {
                libc::renameat(
                    self.directory.file.as_raw_fd(),
                    data.as_ptr(),
                    self.root.file.as_raw_fd(),
                    name.as_ptr(),
                )
            };
            if result != 0 {
                return Err(platform(io::Error::last_os_error()));
            }
            self.published = true;
            Ok(())
        })
    }
}
pub struct Http {
    client: reqwest::Client,
}
impl Http {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}
struct HttpBody {
    response: reqwest::Response,
}
impl Body for HttpBody {
    fn next(&mut self) -> Task<'_, Result<Option<Vec<u8>>, Error>> {
        Box::pin(async move {
            self.response
                .chunk()
                .await
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
                .map_err(|error| Error::Transport(error.to_string()))
        })
    }
}
impl Transport for Http {
    fn get<'a>(&'a self, url: &'a str, offset: u64) -> Task<'a, Result<Response, Error>> {
        Box::pin(async move {
            let mut request = self.client.get(url);
            if offset > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
            }
            let response = request
                .send()
                .await
                .map_err(|error| Error::Transport(error.to_string()))?;
            Ok(Response {
                status: response.status().as_u16(),
                content_range: response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                content_length: response
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                body: Box::new(HttpBody { response }),
            })
        })
    }
}
