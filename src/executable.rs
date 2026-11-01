//! Resolve installed tools without invoking profile-dependent mise shims.
use std::path::{Path, PathBuf};
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub fn resolve(name: &Path) -> Option<PathBuf> {
    let candidates: Vec<_> = if name.components().count() > 1 {
        vec![name.to_owned()]
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|p| p.join(name))
            .collect()
    };
    for candidate in candidates {
        if !executable(&candidate) {
            continue;
        }
        let canonical = candidate.canonicalize().ok()?;
        if canonical.file_name().is_some_and(|n| n == "mise") {
            // A mise symlink changes behavior when HOME changes. Follow its install
            // root directly; never run `mise which` using the isolated profile.
            let root = if candidate
                .parent()?
                .file_name()
                .is_some_and(|n| n == "shims")
            {
                candidate.parent()?.parent()?
            } else {
                candidate.parent()?.parent()?.parent()?
            };
            for alias in ["global", "latest"] {
                let real = root
                    .join("installs")
                    .join(candidate.file_name()?)
                    .join(alias)
                    .join(candidate.file_name()?);
                if executable(&real) {
                    return real.canonicalize().ok();
                }
            }
            continue;
        }
        return Some(canonical);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_mise_wrapper_to_installed_binary_without_profile_or_execution() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!(
            "o3-resolver-{}",
            crate::store::random_id().unwrap()
        ));
        let wrapper = root.join("command-wrappers/bin/codex");
        let real = root.join("installs/codex/global/codex");
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        for (path, body) in [
            (root.join("mise"), "#!/bin/sh\nexit 77\n"),
            (real.clone(), "#!/bin/sh\necho resolved\n"),
        ] {
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        symlink(root.join("mise"), &wrapper).unwrap();
        let resolved = resolve(&wrapper).unwrap();
        assert_eq!(resolved, real.canonicalize().unwrap());
        let output = std::process::Command::new(resolved)
            .env("HOME", root.join("isolated"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"resolved\n");
        std::fs::remove_dir_all(root).unwrap();
    }
}
