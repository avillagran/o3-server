//! Small admin reads and project helpers of the P2 tier, plus the typed "not available here" answers
//! for surfaces that need a runtime o3-server does not have (device hub, MCP apps, guest links,
//! workflow control). Nothing here runs unless a client calls it.
use crate::{
    p2::{invalid, str_field, Reply},
    paths,
    proc::{self, Opts},
    store::{now_ms, State},
};
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "server.getHostResources"
            | "server.checkOtlpEndpoint"
            | "server.refreshUsageRates"
            | "projects.listIgnoredFiles"
            | "projects.createNew"
            | "projectClone.start"
            | "projectClone.cancel"
            | "worktreeSetup.cancel"
            | "orchestration.getOlderSettledThreads"
            | "orchestration.getWorkflowScript"
            | "orchestration.stopWorkflow"
            | "guestShare.create"
            | "mcpApps.callTool"
            | "mcpApps.readResource"
            | "mcpApps.updateModelContext"
    ) || tag.starts_with("device.")
}

pub async fn call(s: &std::sync::Arc<State>, tag: &str, p: &Value) -> Reply {
    if tag.starts_with("device.") {
        return crate::device::call(s, tag, p).await;
    }
    match tag {
        "server.getHostResources" => host_resources().await,
        "server.checkOtlpEndpoint" => check_otlp(p).await,
        "server.refreshUsageRates" => Ok(crate::usage_rates::refresh(s).await),
        "projects.listIgnoredFiles" => list_ignored(p).await,
        "projects.createNew" => create_new(s, p).await,
        "projectClone.start" => clone_start(s, p).await,
        "projectClone.cancel" => Ok(crate::clone_jobs::cancel(s, str_field(p, "projectId")?)),
        "worktreeSetup.cancel" => {
            Ok(json!({"cancelled":crate::worktree_progress::cancel(s,str_field(p,"threadId")?)}))
        }
        "orchestration.getOlderSettledThreads" => older_settled(s, p),
        "orchestration.getWorkflowScript" => workflow_script(p).await,
        "orchestration.stopWorkflow" => Err(json!({
            "_tag":"OrchestrationV2StopWorkflowError","threadId":p["threadId"],"subagentId":p["subagentId"],
            "reason":"unsupported","message":"This provider cannot stop a workflow."
        })),
        "guestShare.create" => Err(
            json!({"_tag":"GuestShareCreateError","reason":"disabled","message":"Guest links are off."}),
        ),
        "mcpApps.callTool" | "mcpApps.readResource" | "mcpApps.updateModelContext" => Err(json!({
            "_tag":"McpAppRequestError","threadId":p["threadId"],"reason":"provider-unsupported",
            "message":"This thread's provider cannot run MCP app requests."
        })),
        "device.easAccount" => Err(
            json!({"_tag":"DeviceEasAccountError","reason":DEVICE_REASON,"message":DEVICE_REASON}),
        ),
        _ => Err(device_unavailable()),
    }
}

const DEVICE_REASON: &str =
    "o3-server has no device hub: simulators, emulators and device hosts are not supported.";
pub fn device_unavailable() -> Value {
    json!({"_tag":"DeviceToolUnavailableError","reason":DEVICE_REASON,"message":DEVICE_REASON})
}

// ---- host resources ------------------------------------------------------------------------

fn cpu_times() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let fields: Vec<u64> = text
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    if fields.len() < 5 {
        return None;
    }
    let idle = fields[3] + fields[4];
    Some((fields.iter().sum(), idle))
}

fn meminfo(key: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with(key))?;
    Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? * 1024)
}

async fn host_resources() -> Reply {
    let first = cpu_times();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let utilization = match (first, cpu_times()) {
        (Some((t0, i0)), Some((t1, i1))) if t1 > t0 => {
            Some((1.0 - (i1 - i0) as f64 / (t1 - t0) as f64).clamp(0.0, 1.0))
        }
        _ => None,
    };
    let total = meminfo("MemTotal:").ok_or_else(
        || json!({"_tag":"RpcInternalError","message":"/proc/meminfo is unreadable"}),
    )?;
    Ok(json!({
        "sampledAt":now_ms(),"cpuUtilization":utilization,
        "cpuCount":std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "availableMemoryBytes":meminfo("MemAvailable:").unwrap_or(0),"totalMemoryBytes":total,
    }))
}

// ---- OTLP endpoint check -------------------------------------------------------------------

async fn check_otlp(p: &Value) -> Reply {
    let signal = p["signal"].as_str().unwrap_or("");
    let body = match signal {
        "traces" => r#"{"resourceSpans":[]}"#,
        "metrics" => r#"{"resourceMetrics":[]}"#,
        "logs" => r#"{"resourceLogs":[]}"#,
        _ => return Err(invalid("signal is invalid")),
    };
    let url = str_field(p, "url")?;
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://"))
        || url.len() > 2048
        || url.chars().any(char::is_control)
    {
        return Err(invalid("url must be http(s)"));
    }
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new("curl");
    // No redirects are followed, the body is a valid empty export, and the output is only the status.
    cmd.args([
        "-sS",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "--max-time",
        "5",
        "--proto",
        "=http,https",
        "-X",
        "POST",
        "-H",
        "content-type: application/json",
        "--data-binary",
        body,
        "--",
        url,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(8), cmd.output()).await;
    let Ok(Ok(out)) = out else {
        return Ok(json!({"_tag":"Unreachable","timedOut":out.is_err()}));
    };
    match out.status.code() {
        Some(0) => {
            let status: u64 = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            Ok(if (200..300).contains(&status) {
                json!({"_tag":"Accepted","latencyMs":started.elapsed().as_millis() as u64})
            } else {
                json!({"_tag":"Rejected","status":status})
            })
        }
        Some(28) => Ok(json!({"_tag":"Unreachable","timedOut":true})),
        _ => Ok(json!({"_tag":"Unreachable","timedOut":false})),
    }
}

// ---- ignored files -------------------------------------------------------------------------

const IGNORED_LIMIT: usize = 200;

fn entries_error(cwd: &str, failure: &str, message: String) -> Value {
    json!({"_tag":"ProjectListEntriesError","cwd":cwd,"failure":failure,"message":message})
}

async fn list_ignored(p: &Value) -> Reply {
    let cwd = str_field(p, "cwd")?.trim().to_owned();
    let root = paths::normalize(&paths::expand_home(&cwd));
    match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() && root.is_absolute() => {}
        Ok(m) if !m.is_dir() => {
            return Err(entries_error(
                &cwd,
                "workspace_root_not_directory",
                format!("Workspace root is not a directory: {}", root.display()),
            ))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(entries_error(
                &cwd,
                "workspace_root_stat_failed",
                e.to_string(),
            ))
        }
        _ => {
            return Err(entries_error(
                &cwd,
                "workspace_root_not_found",
                format!("Workspace root does not exist: {}", root.display()),
            ))
        }
    }
    // `--directory` folds a folder of nothing but ignored files into one `name/` entry, which is how
    // node_modules and build output drop out.
    let opts = Opts {
        timeout: Duration::from_secs(10),
        cap: 4 * 1024 * 1024,
        allow_truncate: false,
    };
    let out = proc::git(
        &root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
            "-z",
        ],
        opts,
    )
    .await;
    // Not a repository (or git failed): there is nothing to list.
    let Some(out) = out.ok().filter(proc::Out::ok) else {
        return Ok(json!({"files":[],"truncated":false}));
    };
    let mut paths_found: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|e| !e.is_empty() && !e.ends_with(b"/"))
        .map(|e| String::from_utf8_lossy(e).into_owned())
        .collect();
    paths_found.sort();
    let truncated = paths_found.len() > IGNORED_LIMIT;
    let files: Vec<Value> = paths_found
        .iter()
        .take(IGNORED_LIMIT)
        .filter_map(|rel| {
            // lstat: a symlink usually points outside the workspace, where reads refuse to go.
            let meta = std::fs::symlink_metadata(root.join(rel)).ok()?;
            meta.is_file()
                .then(|| json!({"path":rel,"byteLength":meta.len()}))
        })
        .collect();
    Ok(json!({"files":files,"truncated":truncated}))
}

// ---- new project ---------------------------------------------------------------------------

fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
        'ç' | 'ć' | 'č' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ñ' | 'ń' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        'š' | 'ś' => "s",
        'ž' | 'ź' | 'ż' => "z",
        _ => return None,
    })
}

/// T3 `newProjectFolderName`: a lowercase ASCII slug of at most 64 characters.
pub fn folder_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(|c| c.to_lowercase()) {
        match fold(c) {
            Some(f) => slug.push_str(f),
            None if c.is_ascii_alphanumeric() => slug.push(c),
            None if !slug.ends_with('-') => slug.push('-'),
            None => {}
        }
    }
    let slug: String = slug.trim_start_matches('-').chars().take(64).collect();
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        return "project".into();
    }
    let reserved = matches!(slug.as_str(), "con" | "prn" | "aux" | "nul")
        || (slug.len() == 4
            && (slug.starts_with("com") || slug.starts_with("lpt"))
            && slug.as_bytes()[3].is_ascii_digit()
            && slug.as_bytes()[3] != b'0');
    if reserved {
        format!("{slug}-project")
    } else {
        slug
    }
}

const ICON_BACKGROUNDS: [&str; 15] = [
    "#dc2626", "#ea580c", "#d97706", "#16a34a", "#059669", "#0d9488", "#0891b2", "#0284c7",
    "#2563eb", "#4f46e5", "#7c3aed", "#9333ea", "#c026d3", "#db2777", "#e11d48",
];

fn escape_xml(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn icon_svg(name: &str) -> String {
    let words = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty());
    let mut initials: String = words
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if initials.is_empty() {
        initials = name
            .trim()
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_else(|| "?".into());
    }
    let mut hash: u32 = 0;
    for c in name.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(c as u32);
    }
    let background = ICON_BACKGROUNDS[hash as usize % ICON_BACKGROUNDS.len()];
    let size = if initials.chars().count() > 1 { 26 } else { 32 };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">\n  <rect width=\"64\" height=\"64\" rx=\"14\" fill=\"{background}\"/>\n  <text x=\"32\" y=\"32\" dy=\"0.35em\" text-anchor=\"middle\" font-family=\"ui-sans-serif, system-ui, -apple-system, sans-serif\" font-size=\"{size}\" font-weight=\"600\" fill=\"#ffffff\">{}</text>\n</svg>\n",
        escape_xml(&initials)
    )
}

fn commit_failure(stderr: &str) -> String {
    let low = stderr.to_lowercase();
    if low.contains("identity unknown")
        || low.contains("tell me who you are")
        || low.contains("no name was given")
        || low.contains("no email was given")
    {
        return "Git has no name or email on this machine. Set user.name and user.email, then commit.".into();
    }
    stderr
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("Git could not make the first commit.")
        .to_owned()
}

fn dispatch_error(message: &str) -> Value {
    json!({"_tag":"OrchestrationDispatchCommandError","message":message})
}

async fn create_new(s: &State, p: &Value) -> Reply {
    let name = str_field(p, "name")?.trim().to_owned();
    if name.chars().count() > 200 {
        return Err(invalid("name is too long"));
    }
    let base = s.dir.parent().unwrap_or(&s.dir).to_path_buf();
    let root = base.join("projects");
    std::fs::create_dir_all(&root)
        .map_err(|e| dispatch_error(&format!("Could not create {}: {e}", root.display())))?;
    let slug = folder_name(&name);
    // `create_dir` is atomic: the first free name is ours.
    let mut folder = None;
    for attempt in 1..=100 {
        let candidate = root.join(if attempt == 1 {
            slug.clone()
        } else {
            format!("{slug}-{attempt}")
        });
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                folder = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(dispatch_error(&format!(
                    "Could not create {}: {e}",
                    candidate.display()
                )))
            }
        }
    }
    let folder = folder
        .ok_or_else(|| dispatch_error(&format!("Every folder name for \"{slug}\" is taken.")))?;
    let cleanup = |e: String| {
        let _ = std::fs::remove_dir_all(&folder);
        dispatch_error(&e)
    };
    let init = proc::git(&folder, &["init", "--initial-branch=main"], Opts::WRITE).await;
    if !matches!(init, Ok(ref o) if o.ok()) {
        return Err(cleanup("git init failed".into()));
    }
    let readme = format!("<img src=\"assets/icon.svg\" width=\"64\" height=\"64\" alt=\"\">\n\n# {name}\n\nCreated in o3-server.\n");
    std::fs::create_dir_all(folder.join("assets"))
        .and_then(|_| std::fs::write(folder.join("README.md"), readme))
        .and_then(|_| std::fs::write(folder.join("assets/icon.svg"), icon_svg(&name)))
        .map_err(|e| cleanup(e.to_string()))?;
    // Explicit filenames so global ignore rules cannot drop them.
    let added = proc::git(
        &folder,
        &["add", "--force", "--", "README.md", "assets/icon.svg"],
        Opts::WRITE,
    )
    .await;
    if !matches!(added, Ok(ref o) if o.ok()) {
        return Err(cleanup("git add failed".into()));
    }
    let commit_error = match proc::git(
        &folder,
        &["commit", "--message", "Initial commit"],
        Opts::WRITE,
    )
    .await
    {
        Ok(o) if o.ok() => None,
        Ok(o) => Some(commit_failure(&o.stderr)),
        Err(_) => Some("Git could not make the first commit.".into()),
    };
    let id = crate::store::random_id().map_err(cleanup)?;
    let created = crate::projects::mutate_resolved(s, &json!({"type":"project.create","commandId":format!("named-project:{id}"),"projectId":id,"title":name,"workspaceRoot":folder,"scripts":[]})).await;
    if let Err(e) = created {
        return Err(cleanup(e));
    }
    let mut out = json!({"projectId":id,"workspaceRoot":folder});
    if let Some(e) = commit_error {
        out["commitError"] = json!(e);
    }
    Ok(out)
}

// ---- clone ---------------------------------------------------------------------------------

fn repository_error(operation: &str, detail: &str) -> Value {
    json!({"_tag":"SourceControlRepositoryError","provider":"github","operation":operation,"detail":detail,"message":format!("Source control repository operation {operation} failed for github: {detail}")})
}

async fn clone_start(s: &std::sync::Arc<State>, p: &Value) -> Reply {
    str_field(p, "projectId")?;
    str_field(p, "title")?;
    let destination =
        paths::normalize(&paths::expand_home(str_field(p, "destinationPath")?.trim()));
    if !destination.is_absolute() {
        return Err(repository_error(
            "clone",
            "The destination must be an absolute path.",
        ));
    }
    let remote = match p["remoteUrl"]
        .as_str()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(url) => url.to_owned(),
        None => {
            if p["provider"]
                .as_str()
                .is_some_and(|provider| provider != "github")
            {
                return Err(repository_error("clone","This provider requires an explicit remoteUrl; repository-name resolution is unavailable."));
            }
            let repo = str_field(p, "repository")
                .map_err(|_| repository_error("clone", "A remoteUrl or repository is required."))?;
            if repo.split('/').count() != 2 || repo.starts_with('-') {
                return Err(repository_error(
                    "clone",
                    "The repository must be owner/name.",
                ));
            }
            if p["protocol"] == "ssh" {
                format!("git@github.com:{repo}.git")
            } else {
                format!("https://github.com/{repo}.git")
            }
        }
    };
    if remote.starts_with('-') || remote.chars().any(char::is_control) {
        return Err(repository_error("clone", "The remote URL is invalid."));
    }
    if destination.exists()
        && destination
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(repository_error(
            "clone",
            &format!("{} already exists and is not empty.", destination.display()),
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| repository_error("clone", &e.to_string()))?;
    }
    // Refuse URL userinfo: snapshots and start responses must never contain credentials.
    if let Ok(url) = url::Url::parse(&remote) {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(repository_error(
                "clone",
                "Credential-bearing remote URLs are not accepted.",
            ));
        }
    }
    crate::clone_jobs::start(s.clone(), p.clone(), destination, remote)
}

// ---- older settled threads -----------------------------------------------------------------

fn older_settled(s: &State, p: &Value) -> Reply {
    let fail = |m: &str| json!({"_tag":"OrchestrationV2GetShellSnapshotError","message":m});
    let cut = p["before"]["settledAt"]
        .as_str()
        .ok_or_else(|| invalid("before.settledAt is required"))?;
    let after_id = p["before"]["threadId"].as_str();
    let limit = p["limit"]
        .as_u64()
        .filter(|l| *l <= 50)
        .ok_or_else(|| invalid("limit must be 0-50"))? as usize;
    let projects: Option<Vec<&str>> = p["projectIds"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect());
    let shell = crate::events::shell(s).map_err(|m| fail(&m))?;
    let mut matching: Vec<Value> = shell["threads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            t["lineage"]["relationshipToParent"] != "subagent" && t["settledOverride"] == "settled"
        })
        .filter(|t| {
            projects
                .as_ref()
                .is_none_or(|ids| t["projectId"].as_str().is_some_and(|id| ids.contains(&id)))
        })
        .filter(|t| {
            t["settledAt"].as_str().is_some_and(|at| {
                at < cut
                    || (at == cut
                        && after_id.is_some_and(|id| t["id"].as_str().is_some_and(|tid| tid > id)))
            })
        })
        .cloned()
        .collect();
    matching.sort_by(|a, b| {
        b["settledAt"]
            .as_str()
            .cmp(&a["settledAt"].as_str())
            .then(a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let seq = shell["snapshotSequence"].clone();
    if limit == 0 {
        let mut counts: Vec<(String, u64)> = Vec::new();
        for t in &matching {
            let id = t["projectId"].as_str().unwrap_or("").to_owned();
            match counts.iter_mut().find(|(p, _)| *p == id) {
                Some((_, n)) => *n += 1,
                None => counts.push((id, 1)),
            }
        }
        return Ok(
            json!({"snapshotSequence":seq,"threads":[],"counts":counts.into_iter().map(|(p, n)| json!({"projectId":p,"count":n})).collect::<Vec<_>>()}),
        );
    }
    matching.truncate(limit);
    Ok(json!({"snapshotSequence":seq,"threads":matching,"counts":[]}))
}

// ---- workflow scripts ----------------------------------------------------------------------

fn script_error(reason: &str, path: &str) -> Value {
    json!({"_tag":"OrchestrationGetWorkflowScriptError","reason":reason,"scriptPath":path})
}

/// A `.js` file below `$CLAUDE_CONFIG_DIR|~/.claude` `/projects`, read up to 256 KiB.
async fn workflow_script(p: &Value) -> Reply {
    let path = str_field(p, "scriptPath")?.to_owned();
    str_field(p, "threadId")?;
    let candidate = Path::new(&path);
    if !candidate.is_absolute() {
        return Err(script_error("invalid-path", &path));
    }
    if candidate.extension().and_then(|e| e.to_str()) != Some("js") {
        return Err(script_error("invalid-path", &path));
    }
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude")));
    let Some(root) = config
        .map(|c| c.join("projects"))
        .and_then(|r| std::fs::canonicalize(r).ok())
    else {
        return Err(script_error("root-unavailable", &path));
    };
    let resolved =
        std::fs::canonicalize(candidate).map_err(|_| script_error("not-found", &path))?;
    let shown = resolved.to_string_lossy().into_owned();
    if !resolved.starts_with(&root) {
        return Err(script_error("outside-root", &shown));
    }
    if resolved.extension().and_then(|e| e.to_str()) != Some("js") {
        return Err(script_error("not-js", &shown));
    }
    crate::files::blocking(move || {
        use std::io::Read;
        let file = std::fs::File::open(&resolved).map_err(|_| script_error("read-failed", &shown))?;
        let meta = file.metadata().map_err(|_| script_error("read-failed", &shown))?;
        if !meta.is_file() {
            return Err(script_error("not-regular-file", &shown));
        }
        let mut buf = Vec::new();
        file.take(256 * 1024).read_to_end(&mut buf).map_err(|_| script_error("read-failed", &shown))?;
        Ok(json!({"scriptPath":shown,"contents":String::from_utf8_lossy(&buf),"truncated":meta.len() > 256 * 1024}))
    })
    .await?
}
