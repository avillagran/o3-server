//! Workspace file RPCs: `projects.readFile`, `projects.searchEntries`, `filesystem.browse`,
//! `filesystem.getMetadata` and `shell.openInEditor`. Blocking disk work runs on the blocking pool.
use crate::{paths, proc};
use serde_json::{json, Value};
use std::{
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

type Reply = Result<Value, Value>;
const READ_MAX: u64 = 1024 * 1024;
const INDEX_MAX_ENTRIES: usize = 25_000;
const METADATA_BATCH: usize = 64;

fn blocking_failed() -> Value {
    json!({"_tag":"RpcNotImplemented","message":"file operation was cancelled"})
}
pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Value> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| blocking_failed())
}

// ---------------------------------------------------------------- projects.readFile

fn read_error(
    p: &Value,
    failure: &str,
    op: Option<&str>,
    resolved: Option<&Path>,
    message: String,
) -> Value {
    let mut e = json!({"_tag":"ProjectReadFileError","cwd":p["cwd"],"relativePath":p["relativePath"],"failure":failure,"message":message});
    if let Some(op) = op {
        e["operation"] = json!(op);
    }
    if let Some(r) = resolved {
        e["resolvedPath"] = json!(r.to_string_lossy());
    }
    e
}

pub async fn read_file(p: Value) -> Reply {
    blocking(move || read_file_sync(&p)).await?
}

fn read_file_sync(p: &Value) -> Reply {
    let cwd = p["cwd"].as_str().filter(|s| !s.trim().is_empty());
    let requested = p["relativePath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (Some(cwd), Some(requested)) = (cwd, requested) else {
        return Err(read_error(
            p,
            "operation_failed",
            None,
            None,
            "cwd and relativePath are required".into(),
        ));
    };
    let allow_missing = p["allowMissing"] == true;
    let host = paths::expand_home(requested);
    // Absolute and `~/` paths read a host file in place, as T3 does; only relative paths are rooted.
    let (target, shown, root_real): (PathBuf, String, Option<PathBuf>) = if host.is_absolute() {
        (host, requested.to_owned(), None)
    } else {
        let root = PathBuf::from(cwd);
        let Some((abs, rel)) = paths::resolve_relative(&root, requested) else {
            return Err(read_error(
                p,
                "workspace_path_outside_root",
                None,
                None,
                format!("Workspace file path must be relative to the project root: {requested}"),
            ));
        };
        let real_root = std::fs::canonicalize(&root).map_err(|e| {
            read_error(
                p,
                "operation_failed",
                Some("realpath-workspace-root"),
                Some(&root),
                e.to_string(),
            )
        })?;
        match paths::real_or_ancestor(&abs) {
            Ok((real, exists)) => {
                if !paths::within(&real_root, &real) {
                    return Err(read_error(
                        p,
                        "resolved_path_outside_root",
                        None,
                        Some(&real),
                        format!("Workspace file resolves outside the project root: {requested}"),
                    ));
                }
                if !exists && !allow_missing {
                    return Err(read_error(
                        p,
                        "operation_failed",
                        Some("realpath-target"),
                        Some(&abs),
                        "No such file or directory".into(),
                    ));
                }
                if !exists {
                    return Ok(absent(&rel));
                }
                (real, rel, Some(real_root))
            }
            Err(e) => {
                return Err(read_error(
                    p,
                    "operation_failed",
                    Some("realpath-target"),
                    Some(&abs),
                    e.to_string(),
                ))
            }
        }
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc_nonblock())
        .open(&target)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_missing => {
            return Ok(absent(&shown))
        }
        Err(e) => {
            return Err(read_error(
                p,
                "operation_failed",
                Some("open"),
                Some(&target),
                e.to_string(),
            ))
        }
    };
    // A path swapped for a link after the check must still land inside the root.
    if let Some(root) = &root_real {
        use std::os::fd::AsRawFd;
        if let Ok(now) = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
            if !paths::within(root, &now) {
                return Err(read_error(
                    p,
                    "resolved_path_outside_root",
                    None,
                    Some(&now),
                    "Workspace file resolves outside the project root".into(),
                ));
            }
        }
    }
    let meta = file.metadata().map_err(|e| {
        read_error(
            p,
            "operation_failed",
            Some("stat"),
            Some(&target),
            e.to_string(),
        )
    })?;
    if !meta.is_file() {
        return Err(read_error(
            p,
            "path_not_file",
            None,
            Some(&target),
            format!("Workspace path is not a file: {shown}"),
        ));
    }
    let mut bytes = Vec::new();
    file.take(READ_MAX).read_to_end(&mut bytes).map_err(|e| {
        read_error(
            p,
            "operation_failed",
            Some("read"),
            Some(&target),
            e.to_string(),
        )
    })?;
    if bytes.contains(&0) {
        return Err(read_error(
            p,
            "binary_file",
            None,
            Some(&target),
            format!("Workspace file '{shown}' is binary and cannot be previewed as text."),
        ));
    }
    Ok(
        json!({"relativePath":shown,"contents":String::from_utf8_lossy(&bytes),"byteLength":meta.len(),"truncated":meta.len()>READ_MAX}),
    )
}
fn absent(rel: &str) -> Value {
    json!({"relativePath":rel,"contents":"","byteLength":0,"truncated":false,"exists":false})
}
const fn libc_nonblock() -> i32 {
    0o4000 // O_NONBLOCK on Linux: a FIFO must not hang the open.
}

// ---------------------------------------------------------------- projects.searchEntries

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "ico", "bmp", "heic",
];
const SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target", ".hg", ".svn", ".jj"];

fn search_error(p: &Value, failure: &str, message: String) -> Value {
    json!({"_tag":"ProjectSearchEntriesError","cwd":p["cwd"],"queryLength":p["query"].as_str().map(|q|q.chars().count()),"limit":p["limit"],"failure":failure,"message":message})
}

/// Files below `root`, from git when it is a repository (honouring ignore rules), else a bounded walk.
async fn list_workspace(root: &Path) -> Vec<(String, bool)> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let git = proc::git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        proc::Opts {
            timeout: std::time::Duration::from_secs(10),
            cap: 16 * 1024 * 1024,
            allow_truncate: true,
        },
    )
    .await;
    if let Some(out) = git.ok().filter(|o| o.ok()) {
        let mut dirs = std::collections::BTreeSet::new();
        for path in out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let path = String::from_utf8_lossy(path).into_owned();
            let mut cut = 0;
            while let Some(i) = path[cut..].find('/') {
                cut += i;
                dirs.insert(path[..cut].to_owned());
                cut += 1;
            }
            entries.push((path, false));
            if entries.len() >= INDEX_MAX_ENTRIES {
                break;
            }
        }
        entries.extend(dirs.into_iter().map(|d| (d, true)));
        return entries;
    }
    let root = root.to_path_buf();
    blocking(move || {
        let mut out = Vec::new();
        let mut stack = vec![(root.clone(), String::new())];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Some((dir, prefix)) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(ft) = e.file_type() else { continue };
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                if ft.is_dir() {
                    if SKIPPED_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    out.push((rel.clone(), true));
                    stack.push((e.path(), rel));
                } else if ft.is_file() || ft.is_symlink() {
                    out.push((rel, false));
                }
                if out.len() >= INDEX_MAX_ENTRIES || std::time::Instant::now() > deadline {
                    return out;
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Lower is better; `None` filters the entry out. Basename matches outrank path matches.
fn score(path: &str, query: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(path.matches('/').count() as u32);
    }
    let lower = path.to_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    if base == query {
        return Some(0);
    }
    if base.starts_with(query) {
        return Some(10);
    }
    if let Some(i) = base.find(query) {
        return Some(20 + i as u32);
    }
    if let Some(i) = lower.find(query) {
        return Some(100 + i as u32);
    }
    let mut chars = query.chars();
    let mut want = chars.next();
    let mut gaps = 0u32;
    for c in lower.chars() {
        match want {
            Some(w) if w == c => want = chars.next(),
            Some(_) => gaps += 1,
            None => break,
        }
    }
    want.is_none().then_some(1000 + gaps.min(5000))
}

pub async fn search_entries(p: Value) -> Reply {
    let cwd = p["cwd"].as_str().unwrap_or("").trim().to_owned();
    let root = paths::normalize(&paths::expand_home(&cwd));
    if cwd.is_empty() || !root.is_absolute() {
        return Err(search_error(
            &p,
            "workspace_root_not_found",
            format!("Workspace root does not exist: {cwd}"),
        ));
    }
    match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(search_error(
                &p,
                "workspace_root_not_directory",
                format!("Workspace root is not a directory: {}", root.display()),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(search_error(
                &p,
                "workspace_root_not_found",
                format!("Workspace root does not exist: {}", root.display()),
            ))
        }
        Err(e) => {
            return Err(search_error(
                &p,
                "workspace_root_stat_failed",
                e.to_string(),
            ))
        }
    }
    let limit = p["limit"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
    let query = p["query"]
        .as_str()
        .unwrap_or("")
        .trim()
        .trim_start_matches(['@', '.', '/'])
        .to_lowercase();
    let kind = p["kind"].as_str().map(str::to_owned);
    let image_only = p["imageOnly"] == true;
    let all = list_workspace(&root).await;
    let mut hits: Vec<(u32, &(String, bool))> = all
        .iter()
        .filter_map(|e| {
            if kind.as_deref() == Some("file") && e.1
                || kind.as_deref() == Some("directory") && !e.1
            {
                return None;
            }
            if image_only {
                let ext = e.0.rsplit('.').next().unwrap_or("").to_lowercase();
                if e.1 || !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                    return None;
                }
            }
            score(&e.0, &query).map(|s| (s, e))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1 .0.len().cmp(&b.1 .0.len()))
            .then(a.1 .0.cmp(&b.1 .0))
    });
    let truncated = hits.len() > limit || all.len() >= INDEX_MAX_ENTRIES;
    let entries: Vec<Value> = hits
        .into_iter()
        .take(limit)
        .map(|(_, e)| json!({"path":e.0,"kind":if e.1 {"directory"} else {"file"}}))
        .collect();
    Ok(json!({"entries":entries,"truncated":truncated}))
}

// ---------------------------------------------------------------- filesystem.browse

fn browse_error(p: &Value, failure: &str, parent: Option<&Path>, message: String) -> Value {
    let mut e = json!({"_tag":"FilesystemBrowseError","partialPath":p["partialPath"],"failure":failure,"message":message});
    if !p["cwd"].is_null() {
        e["cwd"] = p["cwd"].clone();
    }
    if let Some(parent) = parent {
        e["parentPath"] = json!(parent.to_string_lossy());
    }
    e
}

pub async fn browse(p: Value) -> Reply {
    blocking(move || {
        let partial = p["partialPath"].as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 512);
        let Some(partial) = partial else { return Err(browse_error(&p, "read_directory_failed", None, "partialPath is required".into())) };
        let b = partial.as_bytes();
        if partial.contains('\\') && b.len() > 1 && b[1] == b':' || partial.starts_with("\\\\") {
            return Err(browse_error(&p, "windows_path_unsupported", None, format!("Windows paths are not supported on this host: {partial}")));
        }
        let explicit_relative = partial == "." || partial == ".." || partial.starts_with("./") || partial.starts_with("../");
        let resolved = if explicit_relative {
            let Some(cwd) = p["cwd"].as_str().filter(|s| !s.is_empty()) else {
                return Err(browse_error(&p, "current_project_required", None, format!("A current project is required to browse relative workspace path '{partial}'.")));
            };
            paths::normalize(&paths::expand_home(cwd).join(partial))
        } else {
            paths::normalize(&paths::expand_home(partial))
        };
        let ends_sep = partial.ends_with('/') || partial == "~" || partial == "." || partial == "..";
        let (parent, prefix) = if ends_sep {
            (resolved.clone(), String::new())
        } else {
            (resolved.parent().unwrap_or(Path::new("/")).to_path_buf(), resolved.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        };
        let read = match std::fs::read_dir(&parent) {
            Ok(r) => Some(r),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) => None,
            Err(e) => return Err(browse_error(&p, "read_directory_failed", Some(&parent), format!("Failed to read workspace directory '{}' while browsing '{partial}': {e}", parent.display()))),
        };
        let show_hidden = ends_sep || prefix.starts_with('.');
        let lower = prefix.to_lowercase();
        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        for e in read.into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // `is_dir` follows links, like a directory picker expects.
            if name.to_lowercase().starts_with(&lower) && (show_hidden || !name.starts_with('.')) && e.path().is_dir() {
                entries.push((name.clone(), parent.join(name)));
            }
            if entries.len() >= 5000 { break; }
        }
        entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.0.cmp(&b.0)));
        Ok(json!({"parentPath":parent.to_string_lossy(),"entries":entries.into_iter().map(|(n,f)|json!({"name":n,"fullPath":f.to_string_lossy()})).collect::<Vec<_>>()}))
    }).await?
}

// ---------------------------------------------------------------- filesystem.getMetadata

fn header_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if b.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if b.starts_with(&[0x50, 0x4b, 3, 4]) {
        return Some("application/zip");
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if b.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if b.starts_with(b"SQLite format 3\0") {
        return Some("application/vnd.sqlite3");
    }
    if b.starts_with(b"#!") {
        let line =
            String::from_utf8_lossy(b.split(|c| *c == b'\n').next().unwrap_or(b"")).into_owned();
        for (needle, mime) in [
            ("python", "text/x-python"),
            ("node", "text/javascript"),
            ("ruby", "text/x-ruby"),
            ("bash", "text/x-shellscript"),
            ("zsh", "text/x-shellscript"),
            ("fish", "text/x-shellscript"),
            ("sh", "text/x-shellscript"),
        ] {
            if line.contains(needle) {
                return Some(mime);
            }
        }
    }
    None
}

fn metadata_for(path: &str) -> Value {
    let expanded = paths::expand_home(path);
    if !expanded.is_absolute() {
        return Value::Null;
    }
    let Ok(meta) = std::fs::metadata(&expanded) else {
        return Value::Null;
    };
    if meta.is_dir() {
        return json!({"kind":"directory"});
    }
    if !meta.is_file() {
        return json!({"kind":"other"});
    }
    let mut v = json!({"kind":"file","byteLength":meta.len()});
    if expanded.extension().is_none() && meta.len() > 0 {
        if let Ok(f) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc_nonblock())
            .open(&expanded)
        {
            let mut head = Vec::new();
            if f.take(512).read_to_end(&mut head).is_ok() {
                if let Some(m) = header_mime(&head) {
                    v["mimeType"] = json!(m);
                }
            }
        }
    }
    v
}

pub async fn get_metadata(p: Value) -> Reply {
    let list: Vec<String> = match p["paths"].as_array() {
        Some(a)
            if (1..=METADATA_BATCH).contains(&a.len())
                && a.iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 512)) =>
        {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        }
        _ => {
            return Err(
                json!({"_tag":"RpcNotImplemented","message":format!("paths must hold 1 to {METADATA_BATCH} non-empty paths")}),
            )
        }
    };
    blocking(move || {
        let mut seen = std::collections::HashMap::new();
        let entries: Vec<Value> = list
            .iter()
            .map(|path| {
                seen.entry(path.clone())
                    .or_insert_with(|| metadata_for(path))
                    .clone()
            })
            .collect();
        Ok(json!({"entries":entries}))
    })
    .await?
}

// ---------------------------------------------------------------- shell.openInEditor

/// (id, commands, base args)
const EDITORS: &[(&str, &[&str], &[&str])] = &[
    ("cursor", &["cursor"], &["--classic"]),
    ("trae", &["trae"], &[]),
    ("kiro", &["kiro"], &["ide"]),
    ("vscode", &["code"], &[]),
    ("vscode-insiders", &["code-insiders"], &[]),
    ("vscodium", &["codium"], &[]),
    ("zed", &["zed", "zeditor"], &[]),
    ("antigravity", &["antigravity-ide", "agy-ide"], &[]),
    ("idea", &["idea"], &[]),
    ("aqua", &["aqua"], &[]),
    ("clion", &["clion"], &[]),
    ("datagrip", &["datagrip"], &[]),
    ("dataspell", &["dataspell"], &[]),
    ("goland", &["goland"], &[]),
    ("phpstorm", &["phpstorm"], &[]),
    ("pycharm", &["pycharm"], &[]),
    ("rider", &["rider"], &[]),
    ("rubymine", &["rubymine"], &[]),
    ("rustrover", &["rustrover"], &[]),
    ("webstorm", &["webstorm"], &[]),
    ("file-manager", &["xdg-open"], &[]),
];

fn which(command: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(command))
        .find(|c| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(c).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

pub(crate) fn available_editors() -> Vec<&'static str> {
    EDITORS
        .iter()
        .filter(|(_, commands, _)| {
            commands
                .iter()
                .any(|c| crate::executable::resolve(std::path::Path::new(c)).is_some())
        })
        .map(|(id, _, _)| *id)
        .collect()
}
pub async fn open_in_editor(p: Value) -> Reply {
    let fail = |m: String| json!({"_tag":"ExternalLauncherError","message":m});
    let cwd = p["cwd"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let Some(cwd) = cwd.filter(|c| !c.starts_with('-') && !c.contains('\0')) else {
        return Err(fail("cwd is not a valid path".into()));
    };
    let Some((_, commands, base)) = p["editor"]
        .as_str()
        .and_then(|id| EDITORS.iter().find(|e| e.0 == id))
    else {
        return Err(fail(format!("Unknown editor: {}", p["editor"])));
    };
    let Some(exe) = commands.iter().find_map(|c| which(c)) else {
        return Err(fail(format!("Editor command not found: {}", commands[0])));
    };
    let mut target = cwd.to_owned();
    if p["editor"] == "file-manager" && p["reveal"] != true {
        // The file manager opens the folder itself; a file is shown through its parent.
        if let Some(parent) = Path::new(cwd).parent().filter(|_| Path::new(cwd).is_file()) {
            target = parent.to_string_lossy().into_owned();
        }
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(*base)
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| fail(format!("Failed to launch {}: {e}", commands[0])))?;
    // Reap in the background; editors detach on their own.
    let _ = std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(Value::Null)
}
