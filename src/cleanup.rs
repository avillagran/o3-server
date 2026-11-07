//! Explicit no-follow profile removal plans and non-destructive rollback retention.
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    Entry,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
    pub kind: Kind,
}
pub trait Tree {
    fn identity(&self, path: &Path) -> io::Result<Identity>;
    fn children(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}
pub struct FileTree;
impl Tree for FileTree {
    fn identity(&self, path: &Path) -> io::Result<Identity> {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)?;
        Ok(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
            kind: if metadata.is_dir() {
                Kind::Directory
            } else {
                Kind::Entry
            },
        })
    }
    fn children(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        use std::os::{fd::IntoRawFd, unix::ffi::OsStringExt};
        let directory = open_directory(path, &BTreeMap::new())?;
        let fd = directory.into_raw_fd();
        let pointer = unsafe { libc::fdopendir(fd) };
        if pointer.is_null() {
            unsafe {
                libc::close(fd);
            };
            return Err(io::Error::last_os_error());
        }
        struct Directory(*mut libc::DIR);
        impl Drop for Directory {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let directory = Directory(pointer);
        let mut children = Vec::new();
        loop {
            let entry = unsafe { libc::readdir(directory.0) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            children.push(path.join(std::ffi::OsString::from_vec(name.to_vec())));
            if children.len() > 100_000 {
                return Err(io::Error::other(
                    "Removal directory exceeds bounded entries",
                ));
            }
        }
        Ok(children)
    }
}
#[derive(Clone, Debug)]
pub struct Operation {
    pub path: PathBuf,
    pub identity: Identity,
}
#[derive(Clone, Debug)]
pub struct RemovalPlan {
    operations: Vec<Operation>,
    directories: BTreeMap<PathBuf, Identity>,
}
impl RemovalPlan {
    pub fn prepare(root: &Path, tree: &impl Tree) -> io::Result<Self> {
        if !root.is_absolute()
            || root.parent().is_none()
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(io::Error::other("Invalid removal root"));
        }
        let mut plan = Self {
            operations: Vec::new(),
            directories: BTreeMap::new(),
        };
        let mut pending = vec![(root.to_owned(), false, 0)];
        let mut count = 0;
        while let Some((path, visited, depth)) = pending.pop() {
            if visited {
                let identity = plan.directories[&path];
                plan.operations.push(Operation { path, identity });
                continue;
            }
            count += 1;
            if count > 100_000 || depth > 128 {
                return Err(io::Error::other("Removal plan exceeds bounded traversal"));
            }
            let identity = tree.identity(&path)?;
            if path == root && identity.kind != Kind::Directory {
                return Err(io::Error::other("Removal root is not a real directory"));
            }
            if identity.kind == Kind::Directory {
                plan.directories.insert(path.clone(), identity);
                pending.push((path.clone(), true, depth));
                let mut children = tree.children(&path)?;
                children.sort();
                for child in children.into_iter().rev() {
                    if child.parent() != Some(path.as_path()) || child.file_name().is_none() {
                        return Err(io::Error::other("Invalid removal child"));
                    }
                    pending.push((child, false, depth + 1));
                }
            } else {
                plan.operations.push(Operation { path, identity });
            }
        }
        Ok(plan)
    }
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }
}
pub trait Removal: Send + Sync {
    fn execute(&self, plan: &RemovalPlan) -> io::Result<()>;
}
fn open_directory(
    path: &Path,
    expected: &BTreeMap<PathBuf, Identity>,
) -> io::Result<std::os::fd::OwnedFd> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };
    let mut file = fs::File::open("/")?;
    let mut current = PathBuf::from("/");
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        current.push(name);
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { fs::File::from_raw_fd(fd) };
        if let Some(identity) = expected.get(&current) {
            let metadata = file.metadata()?;
            if identity.device != metadata.dev() || identity.inode != metadata.ino() {
                return Err(io::Error::other("Removal directory changed identity"));
            }
        }
    }
    Ok(file.into())
}

#[derive(Default)]
pub struct RealRemoval;
impl Removal for RealRemoval {
    fn execute(&self, plan: &RemovalPlan) -> io::Result<()> {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };
        for operation in &plan.operations {
            let parent = operation
                .path
                .parent()
                .ok_or_else(|| io::Error::other("Missing removal parent"))?;
            let directory = open_directory(parent, &plan.directories)?;
            let name = CString::new(
                operation
                    .path
                    .file_name()
                    .ok_or_else(|| io::Error::other("Missing removal name"))?
                    .as_bytes(),
            )?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let stat = unsafe { stat.assume_init() };
            let kind = if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
                Kind::Directory
            } else {
                Kind::Entry
            };
            #[cfg(target_os = "macos")]
            let device = u64::try_from(stat.st_dev)
                .map_err(|_| io::Error::other("Invalid removal device identity"))?;
            #[cfg(not(target_os = "macos"))]
            let device = stat.st_dev;
            if device != operation.identity.device
                || stat.st_ino != operation.identity.inode
                || kind != operation.identity.kind
            {
                return Err(io::Error::other("Removal entry changed identity"));
            }
            let flags = if kind == Kind::Directory {
                libc::AT_REMOVEDIR
            } else {
                0
            };
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), flags) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}
/// Retain an owned failed/temporary tree by renaming its entry, never traversing it.
pub fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing quarantine parent"))?;
    let destination = parent.join(format!(
        ".o3-retained-{}",
        crate::store::random_id().map_err(io::Error::other)?
    ));
    publish_directory(path, &destination)?;
    Ok(destination)
}

/// Only the supplied owned files and their now-empty directory may be removed.
pub fn remove_owned_files(root: &Path, expected: Identity, names: &[&str]) -> io::Result<()> {
    if FileTree.identity(root)? != expected || expected.kind != Kind::Directory {
        return Err(io::Error::other("Owned directory changed"));
    }
    let mut operations = Vec::new();
    for name in names {
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(io::Error::other("Invalid owned filename"));
        }
        let path = root.join(name);
        match FileTree.identity(&path) {
            Ok(identity) if identity.kind == Kind::Entry => {
                operations.push(Operation { path, identity })
            }
            Ok(_) => return Err(io::Error::other("Owned filename became a directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    operations.push(Operation {
        path: root.to_owned(),
        identity: expected,
    });
    RealRemoval.execute(&RemovalPlan {
        operations,
        directories: BTreeMap::from([(root.to_owned(), expected)]),
    })
}
pub fn quarantine_owned(path: &Path, expected: Identity) -> io::Result<PathBuf> {
    if FileTree.identity(path)? != expected {
        return Err(io::Error::other("Owned directory changed before retention"));
    }
    quarantine(path)
}
/// Create missing components without accepting a symlink component.
pub fn ensure_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("Expected absolute directory"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component),
            _ => return Err(io::Error::other("Invalid directory component")),
        }
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        if !fs::symlink_metadata(&current)?.is_dir() {
            return Err(io::Error::other(
                "Directory contains a link or non-directory",
            ));
        }
    }
    Ok(())
}
/// Publish only to a vacant path. Never replace a concurrent or pre-existing tree.
pub fn publish_directory(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MemoryTree {
        entries: BTreeMap<PathBuf, Identity>,
        children: BTreeMap<PathBuf, Vec<PathBuf>>,
    }
    impl Tree for MemoryTree {
        fn identity(&self, path: &Path) -> io::Result<Identity> {
            self.entries
                .get(path)
                .copied()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        fn children(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.children
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::other("Never traverse a link or non-directory"))
        }
    }
    fn identity(inode: u64, kind: Kind) -> Identity {
        Identity {
            device: 1,
            inode,
            kind,
        }
    }
    #[test]
    fn shared_plan_is_postorder_and_never_traverses_links_or_foreign_children() {
        crate::test_sandbox::assert_isolated();
        let root = PathBuf::from("/fixture/profile");
        let nested = root.join("nested");
        let file = nested.join("data");
        let link = root.join("link");
        let tree = MemoryTree {
            entries: BTreeMap::from([
                (root.clone(), identity(1, Kind::Directory)),
                (nested.clone(), identity(2, Kind::Directory)),
                (file.clone(), identity(3, Kind::Entry)),
                (link.clone(), identity(4, Kind::Entry)),
            ]),
            children: BTreeMap::from([
                (root.clone(), vec![nested.clone(), link.clone()]),
                (nested.clone(), vec![file.clone()]),
            ]),
        };
        let plan = RemovalPlan::prepare(&root, &tree).unwrap();
        let paths = plan
            .operations()
            .iter()
            .map(|operation| operation.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec![link, file, nested, root.clone()]);
        // Acceptance applies exactly the production plan against memory, not a physical tree.
        let mut remaining = tree.entries.clone();
        for operation in plan.operations() {
            assert_eq!(remaining.get(&operation.path), Some(&operation.identity));
            assert!(!remaining
                .keys()
                .any(|path| path != &operation.path && path.starts_with(&operation.path)));
            remaining.remove(&operation.path);
        }
        assert!(remaining.is_empty());
        let invalid = MemoryTree {
            entries: tree.entries,
            children: BTreeMap::from([(root.clone(), vec![PathBuf::from("/fixture/foreign")])]),
        };
        assert!(RemovalPlan::prepare(&root, &invalid).is_err());
        assert!(RemovalPlan::prepare(Path::new("/"), &invalid).is_err());
    }
    fn fixture() -> PathBuf {
        crate::test_sandbox::assert_isolated();
        let root = std::env::temp_dir().join(format!(
            "owned-cleanup-{}",
            crate::store::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn bounded_config_unlink_keeps_unknown_files_and_never_follows_replaced_root() {
        let root = fixture();
        fs::write(root.join("config.json"), "private synthetic config").unwrap();
        fs::write(root.join("unknown"), "retained").unwrap();
        let expected = FileTree.identity(&root).unwrap();
        assert!(remove_owned_files(&root, expected, &["config.json"]).is_err());
        assert!(!root.join("config.json").exists());
        assert_eq!(
            fs::read_to_string(root.join("unknown")).unwrap(),
            "retained"
        );
        let target = root.join("private-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("config.json"), "untouched").unwrap();
        let link = root.join("replacement-link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(remove_owned_files(&link, expected, &["config.json"]).is_err());
        assert_eq!(
            fs::read_to_string(target.join("config.json")).unwrap(),
            "untouched"
        );
    }
    #[test]
    fn real_executor_rejects_replaced_ancestor_before_any_unlink() {
        let root = fixture();
        let owned = root.join("profile");
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join("data"), "owned").unwrap();
        let plan = RemovalPlan::prepare(&owned, &FileTree).unwrap();
        let retained = root.join("retained");
        fs::rename(&owned, &retained).unwrap();
        let target = root.join("private-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("data"), "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &owned).unwrap();
        assert!(FileTree.children(&owned).is_err());
        assert!(RealRemoval.execute(&plan).is_err());
        assert_eq!(
            fs::read_to_string(target.join("data")).unwrap(),
            "untouched"
        );
        assert_eq!(fs::read_to_string(retained.join("data")).unwrap(), "owned");
    }
    #[test]
    fn rollback_retains_only_identified_owned_directory_without_traversing_contents() {
        let root = fixture();
        let owned = root.join("owned");
        fs::create_dir(&owned).unwrap();
        let expected = FileTree.identity(&owned).unwrap();
        let target = root.join("private-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "retained").unwrap();
        std::os::unix::fs::symlink(&target, owned.join("link")).unwrap();
        let retained = quarantine_owned(&owned, expected).unwrap();
        assert!(retained
            .join("link")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "retained");
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join("unknown"), "unchanged").unwrap();
        assert!(quarantine_owned(&owned, expected).is_err());
        assert!(owned.join("unknown").is_file());
    }
}
