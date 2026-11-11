//! Native provider homes and Codex account overlays, adapted from nightly4179 (MIT).
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    ffi::{CStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
type Result<T> = std::result::Result<T, String>;
const SHARED_DIRS: [&str; 11] = [
    "sessions",
    "archived_sessions",
    "sqlite",
    "shell_snapshots",
    "worktrees",
    "skills",
    "plugins",
    "cache",
    "logs",
    "mcp-oauth-locks",
    "thread-writer-locks",
];
const PRIVATE: [&str; 3] = ["auth.json", "auth.lock", "models_cache.json"];
const LOCAL: [&str; 4] = ["log", "memories", "tmp", ".DS_Store"];
const REPLACEABLE: [&str; 3] = [
    "mcp-oauth-locks",
    "thread-writer-locks",
    ".sqlite-maintenance.lock",
];
#[derive(Clone, Debug)]
pub(crate) struct Home {
    pub driver: String,
    pub shared: PathBuf,
    pub runtime: Option<PathBuf>,
    pub overlay: bool,
    pub environment: Vec<(String, String)>,
}
pub(crate) fn raw_environment(entry: &Value) -> Vec<(String, String)> {
    entry["environment"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["name"].as_str()?.into(), v["value"].as_str()?.into())))
        .collect()
}
pub(crate) fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            part => out.push(part.as_os_str()),
        }
    }
    out
}
fn expanded(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        home.to_path_buf()
    } else if value.starts_with("~/") || value.starts_with("~\\") {
        home.join(value[2..].trim_start_matches('/'))
    } else {
        value.into()
    }
}
fn field<'a>(entry: &'a Value, name: &str) -> Result<&'a str> {
    match entry["config"].get(name) {
        None => Ok(""),
        Some(Value::String(s)) => Ok(s),
        _ => Err(format!("Invalid provider path {name}")),
    }
}
pub(crate) fn host_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return Ok(home.into());
    }
    use std::os::unix::ffi::OsStringExt;
    let mut buffer = vec![0u8; 16384];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        let code = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if code == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if code != 0 || found.is_null() {
            return Err("Provider home directory is unavailable".into());
        }
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            return Err("Provider home directory is unavailable".into());
        }
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
        return Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())));
    }
}
pub(crate) fn resolve(entry: &Value) -> Result<Option<Home>> {
    resolve_with(
        entry,
        &host_home()?,
        &std::env::current_dir().map_err(|e| e.to_string())?,
        |key| std::env::var(key).ok(),
    )
}
fn resolve_with(
    entry: &Value,
    host: &Path,
    cwd: &Path,
    inherited: impl Fn(&str) -> Option<String>,
) -> Result<Option<Home>> {
    let driver = entry["driver"].as_str().unwrap_or("");
    if !matches!(driver, "codex" | "claudeAgent") {
        return Ok(None);
    }
    let mut environment = raw_environment(entry);
    let lookup = |key: &str| {
        environment
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, v)| v.clone())
            .or_else(|| inherited(key))
    };
    let configured = field(entry, "homePath")?;
    let (shared, runtime, overlay, key) = if driver == "codex" {
        let shadow = field(entry, "shadowHomePath")?.trim();
        let inherited = if configured.trim().is_empty() && shadow.is_empty() {
            lookup("CODEX_HOME").filter(|v| !v.trim().is_empty())
        } else {
            None
        };
        let home = if !configured.trim().is_empty() {
            Some(configured)
        } else {
            inherited.as_deref()
        };
        let shared = home
            .map(|home| absolute(&expanded(home, host), cwd))
            .unwrap_or_else(|| absolute(&host.join(".codex"), cwd));
        let runtime = if !shadow.is_empty() {
            Some(absolute(&expanded(shadow, host), cwd))
        } else if home.is_some() {
            Some(shared.clone())
        } else {
            None
        };
        (shared, runtime, !shadow.is_empty(), "CODEX_HOME")
    } else {
        let configured = configured.trim();
        let inherited = lookup("CLAUDE_CONFIG_DIR");
        let shared = if !configured.is_empty() {
            absolute(&expanded(configured, host), cwd)
        } else if let Some(value) = inherited
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            absolute(Path::new(value), cwd)
        } else {
            absolute(&host.join(".claude"), cwd)
        };
        let runtime = if configured.is_empty() {
            None
        } else {
            Some(shared.clone())
        };
        (shared, runtime, false, "CLAUDE_CONFIG_DIR")
    };
    if let Some(runtime) = &runtime {
        environment.retain(|(name, _)| name != key);
        environment.push((key.into(), runtime.to_string_lossy().into_owned()));
    }
    let home = Home {
        driver: driver.into(),
        shared,
        runtime,
        overlay,
        environment,
    };
    home.check_paths()?;
    Ok(Some(home))
}
fn error(tag: &str, message: String, fields: Value) -> String {
    let mut result = json!({"_tag":tag,"message":message});
    for (key, value) in fields.as_object().into_iter().flatten() {
        result[key] = value.clone()
    }
    result.to_string()
}
pub(crate) fn display_error(message: &str) -> String {
    serde_json::from_str::<Value>(message)
        .ok()
        .filter(|v| {
            v["_tag"]
                .as_str()
                .is_some_and(|s| s.starts_with("CodexShadowHome"))
        })
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| message.into())
}
#[derive(Debug)]
enum Link {
    Missing,
    File,
    Symbolic(PathBuf),
}
impl Home {
    pub(crate) fn continuation_key(&self) -> String {
        format!(
            "{}:home:{}",
            if self.driver == "claudeAgent" {
                "claude"
            } else {
                &self.driver
            },
            self.shared.display()
        )
    }
    fn fields(&self) -> Value {
        json!({"sharedHomePath":self.shared,"effectiveHomePath":self.runtime})
    }
    fn check_paths(&self) -> Result<()> {
        if self.overlay && self.runtime.as_ref() == Some(&self.shared) {
            return Err(error(
                "CodexShadowHomePathConflictError",
                format!(
                    "Codex shadow home path '{}' must be different from the shared home path '{}'.",
                    self.shared.display(),
                    self.shared.display()
                ),
                self.fields(),
            ));
        }
        Ok(())
    }
    fn fs_error(
        &self,
        operation: &str,
        path: &Path,
        entry: Option<&str>,
        target: Option<&Path>,
        cause: std::io::Error,
    ) -> String {
        let mut fields = self.fields();
        fields["operation"] = json!(operation);
        fields["path"] = json!(path);
        fields["cause"] = json!(cause.to_string());
        if let Some(entry) = entry {
            fields["entryName"] = json!(entry)
        }
        if let Some(target) = target {
            fields["targetPath"] = json!(target)
        }
        let target = target
            .map(|p| format!(" to '{}'", p.display()))
            .unwrap_or_default();
        error(
            "CodexShadowHomeFileSystemError",
            format!(
                "Codex shadow home filesystem operation '{operation}' failed for '{}'{target}.",
                path.display()
            ),
            fields,
        )
    }
    fn state(&self, path: &Path, entry: &str) -> Result<Link> {
        match fs::read_link(path) {
            Ok(target) => Ok(Link::Symbolic(target)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Link::Missing),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => Ok(Link::File),
            Err(e) => Err(self.fs_error("readLink", path, Some(entry), None, e)),
        }
    }
    pub(crate) fn check_private(&self) -> Result<()> {
        self.check_paths()?;
        if !self.overlay {
            return Ok(());
        }
        let path = self
            .runtime
            .as_ref()
            .ok_or("Codex overlay home missing")?
            .join("auth.json");
        if matches!(self.state(&path, "auth.json")?, Link::Symbolic(_)) {
            let mut fields = self.fields();
            fields["entryName"] = json!("auth.json");
            fields["path"] = json!(path);
            return Err(error("CodexShadowHomePrivateEntrySymlinkError",format!("Codex shadow home private entry 'auth.json' at '{}' must be a real file, not a symlink.",path.display()),fields));
        }
        Ok(())
    }
    fn create_link(&self, target: &Path, link: &Path, entry: &str) -> Result<()> {
        match std::os::unix::fs::symlink(target, link) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Concurrent setup of the same overlay may have already installed this exact link.
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    if let Ok(existing) = fs::read_link(link) {
                        if absolute(
                            &link.parent().unwrap_or(Path::new("/")).join(existing),
                            Path::new("/"),
                        ) == target
                        {
                            return Ok(());
                        }
                    }
                }
                Err(self.fs_error("symlink", link, Some(entry), Some(target), e))
            }
        }
    }
    fn ensure_link(&self, entry: &str) -> Result<()> {
        let target = self.shared.join(entry);
        let link = self
            .runtime
            .as_ref()
            .ok_or("Codex overlay home missing")?
            .join(entry);
        match self.state(&link, entry)? {
            Link::Missing => self.create_link(&target, &link, entry),
            Link::Symbolic(existing) => {
                if absolute(
                    &link.parent().unwrap_or(Path::new("/")).join(existing),
                    Path::new("/"),
                ) != target
                {
                    fs::remove_file(&link)
                        .map_err(|e| self.fs_error("remove", &link, Some(entry), None, e))?;
                    self.create_link(&target, &link, entry)?
                }
                Ok(())
            }
            Link::File => {
                if !REPLACEABLE.contains(&entry) {
                    let mut fields = self.fields();
                    fields["entryName"] = json!(entry);
                    fields["linkPath"] = json!(link);
                    fields["targetPath"] = json!(target);
                    return Err(error("CodexShadowHomeEntryConflictError",format!("Cannot create Codex shadow home entry '{entry}' because '{}' already exists and is not a symlink.",link.display()),fields));
                }
                if entry == "thread-writer-locks" {
                    let locks = fs::read_dir(&link)
                        .map_err(|e| self.fs_error("readDirectory", &link, Some(entry), None, e))?;
                    for name in locks {
                        let name = name
                            .map_err(|e| {
                                self.fs_error("readDirectory", &link, Some(entry), None, e)
                            })?
                            .file_name();
                        let name = name.to_string_lossy();
                        if name.ends_with(".lock") && name != ".coordination.lock" {
                            eprintln!("Codex shadow home has thread writer locks; local locks remain until a later setup.");
                            return Ok(());
                        }
                    }
                }
                let metadata = fs::symlink_metadata(&link)
                    .map_err(|e| self.fs_error("remove", &link, Some(entry), None, e))?;
                if metadata.is_dir() {
                    fs::remove_dir_all(&link)
                } else {
                    fs::remove_file(&link)
                }
                .map_err(|e| self.fs_error("remove", &link, Some(entry), None, e))?;
                self.create_link(&target, &link, entry)
            }
        }
    }
    pub(crate) fn materialize(&self) -> Result<()> {
        self.check_paths()?;
        if !self.overlay {
            return Ok(());
        }
        let runtime = self.runtime.as_ref().ok_or("Codex overlay home missing")?;
        for path in std::iter::once(self.shared.clone())
            .chain(std::iter::once(runtime.clone()))
            .chain(SHARED_DIRS.iter().map(|entry| self.shared.join(entry)))
        {
            fs::create_dir_all(&path)
                .map_err(|e| self.fs_error("makeDirectory", &path, None, None, e))?;
        }
        let mut entries = SHARED_DIRS
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect::<BTreeSet<_>>();
        for entry in fs::read_dir(&self.shared)
            .map_err(|e| self.fs_error("readDirectory", &self.shared, None, None, e))?
        {
            let entry = entry
                .map_err(|e| self.fs_error("readDirectory", &self.shared, None, None, e))?
                .file_name();
            let entry = entry.to_string_lossy().into_owned();
            if !PRIVATE.contains(&entry.as_str()) && !LOCAL.contains(&entry.as_str()) {
                entries.insert(entry);
            }
        }
        for entry in PRIVATE.into_iter().filter(|entry| *entry != "auth.json") {
            let path = runtime.join(entry);
            if matches!(self.state(&path, entry)?, Link::Symbolic(_)) {
                fs::remove_file(&path)
                    .map_err(|e| self.fs_error("remove", &path, Some(entry), None, e))?;
            }
        }
        for entry in entries {
            self.ensure_link(&entry)?;
        }
        self.check_private()
    }
}
/// Filesystem work runs on a blocking worker and is shared by native probe/session clones.
pub(crate) struct Setup {
    home: Home,
    gate: Mutex<()>,
    ready: AtomicBool,
}
impl Setup {
    pub(crate) fn shared_home(&self) -> &Path {
        &self.home.shared
    }
    pub(crate) fn new(home: Home) -> Arc<Self> {
        let ready = !home.overlay;
        Arc::new(Self {
            home,
            gate: Mutex::new(()),
            ready: AtomicBool::new(ready),
        })
    }
    pub(crate) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub(crate) fn check_private(&self) -> Result<()> {
        self.home.check_private()
    }
    pub(crate) async fn prepare(self: &Arc<Self>) -> Result<()> {
        let setup = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = setup.gate.lock().unwrap();
            let result = setup.home.materialize();
            setup.ready.store(result.is_ok(), Ordering::Release);
            result
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "provider-home-{}",
                crate::store::random_id().unwrap()
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn overlay(&self) -> Home {
            resolve_with(&json!({"driver":"codex","config":{"homePath":self.path("shared"),"shadowHomePath":self.path("shadow")}}),&self.path("host"),&self.0,|_|None).unwrap().unwrap()
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn write(path: impl AsRef<Path>, value: &str) {
        let path = path.as_ref();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    #[test]
    fn pinned_typescript_resolvers_match_layout_environment_and_literal_tilde_rules() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/provider-home/layout.json"))
                .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let home = resolve_with(
                &case["entry"],
                Path::new(case["host"].as_str().unwrap()),
                Path::new(case["cwd"].as_str().unwrap()),
                |key| case["inherited"][key].as_str().map(str::to_owned),
            )
            .unwrap()
            .unwrap();
            let expected = &case["expected"];
            assert_eq!(
                json!(home.shared),
                expected["sharedHomePath"],
                "{}",
                case["entry"]
            );
            if home.driver == "codex" {
                assert_eq!(home.overlay, expected["mode"] == "authOverlay");
                assert_eq!(json!(home.runtime), expected["effectiveHomePath"]);
            } else {
                let mut environment = case["inherited"].clone();
                if !environment.is_object() {
                    environment = json!({})
                }
                for (name, value) in home.environment {
                    environment[&name] = json!(value)
                }
                assert_eq!(environment, expected["environment"]);
            }
        }
    }
    #[test]
    fn overlay_materialization_shares_state_and_preserves_private_auth_cache_and_runtime() {
        let dir = Sandbox::new();
        let home = dir.overlay();
        let shared = &home.shared;
        let shadow = home.runtime.as_ref().unwrap();
        write(shared.join("config.toml"), "shared config");
        write(shared.join("auth.json"), "shared auth");
        write(shared.join("models_cache.json"), "shared models");
        write(shared.join("auth.lock"), "shared lock");
        write(shadow.join("auth.json"), "private auth");
        for entry in LOCAL {
            write(shadow.join(entry).join("marker"), "local");
        }
        std::os::unix::fs::symlink(
            shared.join("models_cache.json"),
            shadow.join("models_cache.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(shared.join("auth.lock"), shadow.join("auth.lock")).unwrap();
        for _ in 0..2 {
            home.materialize().unwrap();
        }
        for entry in SHARED_DIRS {
            assert_eq!(
                fs::read_link(shadow.join(entry)).unwrap(),
                shared.join(entry)
            );
        }
        assert_eq!(
            fs::read_link(shadow.join("config.toml")).unwrap(),
            shared.join("config.toml")
        );
        assert_eq!(
            fs::read_to_string(shadow.join("auth.json")).unwrap(),
            "private auth"
        );
        assert!(!fs::symlink_metadata(shadow.join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!shadow.join("auth.lock").exists());
        assert!(!shadow.join("models_cache.json").exists());
        for entry in LOCAL {
            assert_eq!(
                fs::read_to_string(shadow.join(entry).join("marker")).unwrap(),
                "local"
            );
        }
        write(shared.join("new-state.sqlite"), "new shared state");
        home.materialize().unwrap();
        assert_eq!(
            fs::read_link(shadow.join("new-state.sqlite")).unwrap(),
            shared.join("new-state.sqlite")
        );
        write(shadow.join("models_cache.json"), "private models");
        write(shadow.join("auth.lock"), "private lock");
        home.materialize().unwrap();
        assert_eq!(
            fs::read_to_string(shadow.join("models_cache.json")).unwrap(),
            "private models"
        );
        assert_eq!(
            fs::read_to_string(shadow.join("auth.lock")).unwrap(),
            "private lock"
        );
    }
    #[test]
    fn private_auth_symlinks_and_real_shared_entry_conflicts_are_explicit_and_preserved() {
        let dir = Sandbox::new();
        let home = dir.overlay();
        let shadow = home.runtime.as_ref().unwrap();
        write(home.shared.join("auth.json"), "must not change");
        fs::create_dir_all(shadow).unwrap();
        std::os::unix::fs::symlink(home.shared.join("auth.json"), shadow.join("auth.json"))
            .unwrap();
        let error: Value = serde_json::from_str(&home.materialize().unwrap_err()).unwrap();
        assert_eq!(error["_tag"], "CodexShadowHomePrivateEntrySymlinkError");
        assert!(fs::symlink_metadata(shadow.join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(home.shared.join("auth.json")).unwrap(),
            "must not change"
        );
        fs::remove_file(shadow.join("auth.json")).unwrap();
        write(shadow.join("auth.json"), "private");
        write(home.shared.join("config.toml"), "shared");
        write(shadow.join("config.toml"), "local file stays");
        let error: Value = serde_json::from_str(&home.materialize().unwrap_err()).unwrap();
        assert_eq!(error["_tag"], "CodexShadowHomeEntryConflictError");
        assert_eq!(error["entryName"], "config.toml");
        assert_eq!(
            fs::read_to_string(shadow.join("config.toml")).unwrap(),
            "local file stays"
        );
    }
    #[test]
    fn local_writer_locks_survive_until_release_while_replaceable_locks_become_shared() {
        let dir = Sandbox::new();
        let home = dir.overlay();
        let shadow = home.runtime.as_ref().unwrap();
        write(
            home.shared.join(".sqlite-maintenance.lock"),
            "shared maintenance",
        );
        write(shadow.join(".sqlite-maintenance.lock"), "local maintenance");
        write(
            home.shared.join("mcp-oauth-locks/file-store.lock"),
            "shared oauth",
        );
        write(
            shadow.join("mcp-oauth-locks/file-store.lock"),
            "local oauth",
        );
        write(shadow.join("thread-writer-locks/.coordination.lock"), "");
        write(shadow.join("thread-writer-locks/.DS_Store"), "");
        write(shadow.join("thread-writer-locks/writer.lock"), "active");
        home.materialize().unwrap();
        assert_eq!(
            fs::read_to_string(shadow.join("thread-writer-locks/writer.lock")).unwrap(),
            "active"
        );
        assert!(fs::read_link(shadow.join("thread-writer-locks")).is_err());
        assert_eq!(
            fs::read_link(shadow.join("mcp-oauth-locks")).unwrap(),
            home.shared.join("mcp-oauth-locks")
        );
        assert_eq!(
            fs::read_to_string(home.shared.join("mcp-oauth-locks/file-store.lock")).unwrap(),
            "shared oauth"
        );
        assert_eq!(
            fs::read_link(shadow.join(".sqlite-maintenance.lock")).unwrap(),
            home.shared.join(".sqlite-maintenance.lock")
        );
        fs::remove_file(shadow.join("thread-writer-locks/writer.lock")).unwrap();
        home.materialize().unwrap();
        assert_eq!(
            fs::read_link(shadow.join("thread-writer-locks")).unwrap(),
            home.shared.join("thread-writer-locks")
        );
    }
    #[test]
    fn stale_shared_links_are_retargeted_and_correct_relative_links_are_retained() {
        let dir = Sandbox::new();
        let home = dir.overlay();
        let shadow = home.runtime.as_ref().unwrap();
        write(home.shared.join("config.toml"), "shared");
        write(dir.path("old-config"), "old config");
        fs::create_dir_all(shadow).unwrap();
        std::os::unix::fs::symlink(dir.path("old-config"), shadow.join("config.toml")).unwrap();
        home.materialize().unwrap();
        assert_eq!(
            fs::read_link(shadow.join("config.toml")).unwrap(),
            home.shared.join("config.toml")
        );
        assert_eq!(
            fs::read_to_string(dir.path("old-config")).unwrap(),
            "old config"
        );
        fs::remove_file(shadow.join("config.toml")).unwrap();
        std::os::unix::fs::symlink("../shared/config.toml", shadow.join("config.toml")).unwrap();
        home.materialize().unwrap();
        assert_eq!(
            fs::read_link(shadow.join("config.toml")).unwrap(),
            Path::new("../shared/config.toml")
        );
    }
    #[test]
    fn equal_homes_and_filesystem_failures_report_the_source_error_shapes() {
        let dir = Sandbox::new();
        let equal=resolve_with(&json!({"driver":"codex","config":{"homePath":dir.path("new"),"shadowHomePath":dir.path("new")}}),&dir.path("host"),&dir.0,|_|None).unwrap_err();
        let error: Value = serde_json::from_str(&equal).unwrap();
        assert_eq!(error["_tag"], "CodexShadowHomePathConflictError");
        assert!(!dir.path("new").exists());
        let home = dir.overlay();
        write(&home.shared, "file blocks a directory");
        let error: Value = serde_json::from_str(&home.materialize().unwrap_err()).unwrap();
        assert_eq!(error["_tag"], "CodexShadowHomeFileSystemError");
        assert_eq!(error["operation"], "makeDirectory");
    }
    #[test]
    fn concurrent_setup_installs_identical_links_without_overwriting_private_auth() {
        let dir = Sandbox::new();
        let home = Arc::new(dir.overlay());
        write(
            home.runtime.as_ref().unwrap().join("auth.json"),
            "private auth",
        );
        let workers = (0..8)
            .map(|_| {
                let home = home.clone();
                std::thread::spawn(move || home.materialize())
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        for entry in SHARED_DIRS {
            assert_eq!(
                fs::read_link(home.runtime.as_ref().unwrap().join(entry)).unwrap(),
                home.shared.join(entry)
            );
        }
        assert_eq!(
            fs::read_to_string(home.runtime.as_ref().unwrap().join("auth.json")).unwrap(),
            "private auth"
        );
    }
}
