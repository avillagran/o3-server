//! Optional document text, parsed in a bounded, isolated process. Original files
//! remain available even when parsing fails. No office runtime or scripts run.
mod html_entities;
mod office;
mod plain;
mod xml;
use std::{
    collections::HashMap,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
const MAX_INPUT: u64 = 16 * 1024 * 1024;
const MAX_OUTPUT: usize = 1_000_000;
const MARKER: &str = "\n\n[Extracted text truncated at document extraction limits.]";
#[derive(Debug)]
enum Error {
    Invalid,
    Truncated,
}
type Result<T> = std::result::Result<T, Error>;
struct Sink {
    text: String,
    remaining: usize,
    depth: usize,
    started: bool,
    space: bool,
}
impl Sink {
    fn new() -> Self {
        Self {
            text: String::new(),
            remaining: MAX_OUTPUT,
            depth: 0,
            started: false,
            space: false,
        }
    }
    fn add(&mut self, raw: &str) -> Result<()> {
        let mut value = String::new();
        let mut used = 0;
        for c in raw.chars() {
            let n = c.len_utf16();
            if used + n > self.remaining {
                self.remaining = 0;
                break;
            }
            used += n;
            if c != '\0' {
                value.push(c)
            }
        }
        self.remaining = self.remaining.saturating_sub(used);
        if self.depth > 0 {
            let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
            if !normalized.is_empty() {
                if self.started && (self.space || value.starts_with(char::is_whitespace)) {
                    self.text.push(' ')
                }
                self.text.push_str(&normalized);
                self.started = true
            }
            self.space = value.ends_with(char::is_whitespace) || (value.is_empty() && self.space);
        } else {
            self.text.push_str(&value)
        }
        if self.remaining == 0 {
            Err(Error::Truncated)
        } else {
            Ok(())
        }
    }
    fn cell_start(&mut self) {
        if self.depth == 0 {
            self.started = false;
            self.space = false
        } else if self.started {
            self.space = true
        }
        self.depth += 1;
    }
    fn cell_end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth > 0 && self.started {
            self.space = true
        }
    }
    fn fallback(&mut self, value: &str) -> Result<()> {
        if !self.started {
            self.add(value)
        } else {
            Ok(())
        }
    }
    fn finish(self, truncated: bool) -> Result<String> {
        let re = regex::Regex::new(r"\n(?:[ \t]*\n){2,}").unwrap();
        let text = re.replace_all(&self.text, "\n\n").trim().to_owned();
        if text.is_empty() && !truncated {
            return Err(Error::Invalid);
        }
        Ok(format!("{text}{}", if truncated { MARKER } else { "" }))
    }
}
fn extract(path: &Path, extension: &str) -> Result<String> {
    let mut out = Sink::new();
    let result = if std::fs::metadata(path).map_err(|_| Error::Invalid)?.len() > MAX_INPUT {
        Err(Error::Truncated)
    } else {
        match extension {
            "ipynb" | "rtf" => {
                let data = std::fs::read(path).map_err(|_| Error::Invalid)?;
                if extension == "rtf" {
                    plain::rtf(&data, &mut out)
                } else {
                    plain::notebook(&data, &mut out)
                }
            }
            _ => office::extract(path, extension, &mut out),
        }
    };
    match result {
        Ok(()) => out.finish(false),
        Err(Error::Truncated) => out.finish(true),
        Err(e) => Err(e),
    }
}
fn extension(path: &Path) -> Option<String> {
    let e = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(
        e.as_str(),
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "epub" | "ipynb"
    )
    .then_some(e)
}
pub(crate) fn sidecar(path: &Path) -> Option<PathBuf> {
    extension(path)?;
    let target = PathBuf::from(format!("{}.txt", path.to_str()?));
    std::fs::symlink_metadata(&target)
        .ok()
        .filter(|m| m.is_file())
        .map(|_| target)
}
/// Internal worker command. Exit 2 is a permanent invalid-document result.
#[doc(hidden)]
pub fn worker(path: &Path, ext: &str) -> ! {
    match extract(path, ext.trim_start_matches('.')) {
        Ok(text) => {
            if std::io::stdout().write_all(text.as_bytes()).is_ok() {
                std::process::exit(0)
            }
            std::process::exit(1)
        }
        Err(_) => std::process::exit(2),
    }
}
#[derive(Default)]
struct Cache {
    failed: HashMap<PathBuf, Option<Instant>>,
}
#[derive(Default)]
pub(crate) struct Registry {
    gate: Mutex<Cache>,
    worker: Mutex<Option<PathBuf>>,
}
impl Registry {
    pub(crate) fn set_worker(&self, path: PathBuf) {
        *self.worker.lock().unwrap() = Some(path)
    }
    fn program(&self) -> Option<PathBuf> {
        if let Some(path) = self.worker.lock().unwrap().clone() {
            return Some(path);
        }
        let exe = std::env::current_exe().ok()?;
        (exe.file_name()?.to_str()? == "o3-server").then_some(exe)
    }
    pub(crate) fn ensure(&self, path: &Path) -> Option<PathBuf> {
        let ext = extension(path)?;
        if let Some(p) = sidecar(path) {
            return Some(p);
        }
        let mut cache = self.gate.lock().unwrap();
        if let Some(p) = sidecar(path) {
            return Some(p);
        }
        if cache
            .failed
            .get(path)
            .is_some_and(|until| until.is_none_or(|t| t > Instant::now()))
        {
            return None;
        }
        if std::fs::metadata(path).ok()?.len() > MAX_INPUT {
            return None;
        }
        let program = self.program()?;
        let input = path.to_owned();
        // A dedicated runtime also supports synchronous embedding callers. No SQLite lock is held.
        let result = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?
                .block_on(run(&program, &input, &ext))
        })
        .join()
        .ok()
        .flatten();
        let Some((code, text)) = result else {
            cache_failure(&mut cache, path, false);
            return None;
        };
        if code != 0 {
            cache_failure(&mut cache, path, code == 2);
            return None;
        }
        let target = PathBuf::from(format!("{}.txt", path.to_str()?));
        let temporary = PathBuf::from(format!(
            "{}.{}.part",
            target.to_str()?,
            crate::store::random_id().ok()?
        ));
        let saved = (|| {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .ok()?;
            f.write_all(text.as_bytes()).ok()?;
            f.sync_all().ok()?;
            std::fs::rename(&temporary, &target).ok()?;
            Some(target)
        })();
        let _ = std::fs::remove_file(temporary);
        saved
    }
}
fn cache_failure(cache: &mut Cache, path: &Path, permanent: bool) {
    if cache.failed.len() >= 1024 {
        cache.failed.clear()
    }
    cache.failed.insert(
        path.to_owned(),
        if permanent {
            None
        } else {
            Some(Instant::now() + Duration::from_secs(1))
        },
    );
}
async fn run(program: &Path, path: &Path, ext: &str) -> Option<(i32, String)> {
    use tokio::io::AsyncReadExt;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(["__document-text"])
        .arg(path)
        .arg(format!(".{ext}"))
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    // Isolate parser memory/CPU and keep any spawned descendants in the same group.
    unsafe {
        cmd.pre_exec(|| {
            let memory = libc::rlimit {
                rlim_cur: 256 * 1024 * 1024,
                rlim_max: 256 * 1024 * 1024,
            };
            let cpu = libc::rlimit {
                rlim_cur: 15,
                rlim_max: 15,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                || libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;
    let pid = child.id()?;
    let mut stdout = child.stdout.take()?.take(4 * 1024 * 1024 + 1025);
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        stdout.read_to_end(&mut bytes).await?;
        if bytes.len() > 4 * 1024 * 1024 + 1024 {
            return Err(std::io::Error::other("document output limit"));
        }
        child.wait().await
    })
    .await;
    // Also close descendants holding stdout when a faulty embedding worker times out.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.wait().await;
    let status = result.ok()?.ok()?;
    Some((status.code().unwrap_or(1), String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "o3-document-{label}-{}-{}",
            std::process::id(),
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn worker_failures_retry_transient_cache_permanent_and_always_allow_existing_sidecars() {
        let root = root("failures");
        let program = root.join("worker");
        let source = root.join("test.rtf");
        std::fs::write(&source, "rtf").unwrap();
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = Registry::default();
        registry.set_worker(program.clone());
        assert!(registry.ensure(&source).is_none());
        std::fs::write(&program, "#!/bin/sh\nprintf 'Recovered'\n").unwrap();
        assert!(registry.ensure(&source).is_none());
        std::thread::sleep(Duration::from_millis(1050));
        let path = registry.ensure(&source).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Recovered");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_file(&path);
        std::fs::write(&program, "#!/bin/sh\nexit 2\n").unwrap();
        assert!(registry.ensure(&source).is_none());
        std::fs::write(&program, "#!/bin/sh\nprintf 'Wrong'\n").unwrap();
        std::thread::sleep(Duration::from_millis(1050));
        assert!(registry.ensure(&source).is_none());
        std::fs::write(&path, "Cached").unwrap();
        assert_eq!(registry.ensure(&source), Some(path));
        // Fixture trees are retained for private container teardown.
    }
    #[test]
    fn worker_output_limit_and_input_limit_leave_no_sidecar() {
        let root = root("bounds");
        let program = root.join("worker");
        let source = root.join("test.rtf");
        std::fs::write(&source, "rtf").unwrap();
        std::fs::write(
            &program,
            "#!/usr/bin/python3\nimport sys\nsys.stdout.write('x' * (4*1024*1024+1025))\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = Registry::default();
        registry.set_worker(program);
        assert!(registry.ensure(&source).is_none());
        assert!(sidecar(&source).is_none());
        std::fs::File::create(&source)
            .unwrap()
            .set_len(MAX_INPUT + 1)
            .unwrap();
        assert!(registry.ensure(&source).is_none());
        assert!(sidecar(&source).is_none());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        // Fixture trees are retained for private container teardown.
    }
}
