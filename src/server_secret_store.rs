//! Private file slots. No identity migration or server consumers are activated here.
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    pub tag: &'static str,
    pub resource: String,
    pub already_exists: bool,
}
impl StoreError {
    pub fn new(tag: &'static str, name: &str) -> Self {
        Self {
            tag,
            resource: format!("secret {name}"),
            already_exists: false,
        }
    }
    fn io(tag: &'static str, name: &str, e: io::Error) -> Self {
        Self {
            already_exists: e.kind() == io::ErrorKind::AlreadyExists,
            ..Self::new(tag, name)
        }
    }
}
pub trait SecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn set(&self, name: &str, value: &[u8]) -> Result<(), StoreError>;
    fn remove(&self, name: &str) -> Result<(), StoreError>;
}
pub struct FileSecretStore {
    directory: File,
}
fn name(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(io::Error::other)
}
fn open(parent: &File, value: &str, flags: i32, mode: u32) -> io::Result<File> {
    let value = name(value)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            value.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn slot(value: &str) -> io::Result<String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(io::Error::other("invalid secret slot"));
    }
    Ok(format!("{value}.bin"))
}
fn chmod(file: &File, mode: u32) -> io::Result<()> {
    if unsafe { libc::fchmod(file.as_raw_fd(), mode) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn unlink(parent: &File, value: &str) -> io::Result<()> {
    let value = name(value)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), value.as_ptr(), 0) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
struct OwnedTemporary<'a> {
    directory: &'a File,
    name: String,
    file: File,
}
impl Drop for OwnedTemporary<'_> {
    fn drop(&mut self) {
        // A failed exclusive create never constructs this guard. A replaced entry
        // is not ours, even when it has the same temporary filename.
        if let Ok(current) = open(
            self.directory,
            &self.name,
            libc::O_RDONLY | libc::O_NONBLOCK,
            0,
        ) {
            if let (Ok(a), Ok(b)) = (current.metadata(), self.file.metadata()) {
                if a.dev() == b.dev() && a.ino() == b.ino() {
                    let _ = unlink(self.directory, &self.name);
                }
            }
        }
    }
}
impl FileSecretStore {
    pub fn open(directory: &Path) -> Result<Self, StoreError> {
        let secure = |_: io::Error| StoreError {
            tag: "SecretStoreSecureError",
            resource: format!("secrets directory {}", directory.display()),
            already_exists: false,
        };
        let mut current = File::open("/").map_err(secure)?;
        if directory.parent().is_none() || !directory.is_absolute() {
            return Err(secure(io::Error::other("absolute path required")));
        }
        for component in directory.components() {
            let Component::Normal(part) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(secure(io::Error::other("invalid directory")));
            };
            let part = part
                .to_str()
                .ok_or_else(|| secure(io::Error::other("invalid directory")))?;
            current = match open(&current, part, libc::O_RDONLY | libc::O_DIRECTORY, 0) {
                Ok(next) => next,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let c = name(part).map_err(secure)?;
                    if unsafe { libc::mkdirat(current.as_raw_fd(), c.as_ptr(), 0o700) } < 0 {
                        let e = io::Error::last_os_error();
                        if e.kind() != io::ErrorKind::AlreadyExists {
                            return Err(secure(e));
                        }
                    }
                    open(&current, part, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(secure)?
                }
                Err(e) => return Err(secure(e)),
            };
        }
        chmod(&current, 0o700).map_err(secure)?;
        Ok(Self { directory: current })
    }
    fn persist(&self, secret: &str, bytes: &[u8], exclusive: bool) -> Result<(), StoreError> {
        let target =
            slot(secret).map_err(|e| StoreError::io("SecretStorePersistError", secret, e))?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| StoreError::new("SecretStoreTemporaryPathError", secret))?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.persist_named(secret, bytes, exclusive, &target, &suffix)
    }
    fn persist_named(
        &self,
        secret: &str,
        bytes: &[u8],
        exclusive: bool,
        target: &str,
        suffix: &str,
    ) -> Result<(), StoreError> {
        let temporary = format!("{target}.{suffix}.tmp");
        let file = open(
            &self.directory,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|e| StoreError::io("SecretStorePersistError", secret, e))?;
        let mut owned = OwnedTemporary {
            directory: &self.directory,
            name: temporary,
            file,
        };
        let result = (|| {
            owned.file.write_all(bytes)?;
            owned.file.sync_all()?;
            chmod(&owned.file, 0o600)?;
            let from = name(&owned.name)?;
            let to = name(target)?;
            let rc = unsafe {
                if exclusive {
                    libc::linkat(
                        self.directory.as_raw_fd(),
                        from.as_ptr(),
                        self.directory.as_raw_fd(),
                        to.as_ptr(),
                        0,
                    )
                } else {
                    libc::renameat(
                        self.directory.as_raw_fd(),
                        from.as_ptr(),
                        self.directory.as_raw_fd(),
                        to.as_ptr(),
                    )
                }
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            chmod(&owned.file, 0o600)
        })();
        result.map_err(|e| StoreError::io("SecretStorePersistError", secret, e))
    }
    #[cfg(test)]
    pub fn create_with_fixture_nonce(&self, secret: &str, value: &[u8]) -> Result<(), StoreError> {
        let target =
            slot(secret).map_err(|e| StoreError::io("SecretStorePersistError", secret, e))?;
        self.persist_named(secret, value, true, &target, "fixed-fixture")
    }
    pub fn create(&self, name: &str, value: &[u8]) -> Result<(), StoreError> {
        self.persist(name, value, true)
    }
    pub fn get_or_create_random(&self, name: &str, size: usize) -> Result<Vec<u8>, StoreError> {
        if let Some(value) = self.get(name)? {
            return Ok(value);
        }
        let mut bytes = vec![0; size];
        getrandom::fill(&mut bytes)
            .map_err(|_| StoreError::new("SecretStoreRandomGenerationError", name))?;
        match self.create(name, &bytes) {
            Ok(()) => Ok(bytes),
            Err(e) if e.already_exists => self
                .get(name)?
                .ok_or_else(|| StoreError::new("SecretStoreConcurrentReadError", name)),
            Err(e) => Err(e),
        }
    }
}
impl SecretStore for FileSecretStore {
    fn get(&self, secret: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let path = slot(secret).map_err(|e| StoreError::io("SecretStoreReadError", secret, e))?;
        match open(&self.directory, &path, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(mut file) => {
                if !file.metadata().is_ok_and(|m| m.is_file()) {
                    return Err(StoreError::new("SecretStoreReadError", secret));
                }
                let mut value = Vec::new();
                file.read_to_end(&mut value)
                    .map_err(|e| StoreError::io("SecretStoreReadError", secret, e))?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::io("SecretStoreReadError", secret, e)),
        }
    }
    fn set(&self, name: &str, value: &[u8]) -> Result<(), StoreError> {
        self.persist(name, value, false)
    }
    fn remove(&self, secret: &str) -> Result<(), StoreError> {
        let path = slot(secret).map_err(|e| StoreError::io("SecretStoreRemoveError", secret, e))?;
        match open(&self.directory, &path, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(file) => {
                if !file
                    .metadata()
                    .map_err(|e| StoreError::io("SecretStoreRemoveError", secret, e))?
                    .is_file()
                {
                    return Err(StoreError::new("SecretStoreRemoveError", secret));
                }
                unlink(&self.directory, &path)
                    .map_err(|e| StoreError::io("SecretStoreRemoveError", secret, e))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::io("SecretStoreRemoveError", secret, e)),
        }
    }
}
