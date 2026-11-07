//! `vcs.*` RPCs over the git CLI. Every call is an argv subprocess (see `proc`); mutations hold the
//! repository's lock so two clients cannot interleave ref or index changes.
use crate::{
    paths,
    proc::{self, Opts, Out, RunError},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub type Reply = Result<Value, Value>;
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

// ------------------------------------------------------------------ errors

pub fn run_err(op: &str, cwd: &Path, e: RunError) -> Value {
    match e {
        RunError::Spawn(cause) => {
            json!({"_tag":"VcsProcessSpawnError","operation":op,"command":"git","cwd":cwd.to_string_lossy(),"cause":cause,"message":format!("Failed to start git: {cause}")})
        }
        RunError::Timeout(ms) => {
            json!({"_tag":"VcsProcessTimeoutError","operation":op,"command":"git","cwd":cwd.to_string_lossy(),"timeoutMs":ms,"message":format!("git timed out after {ms} ms")})
        }
    }
}
pub fn reason_of(stderr: &str) -> Option<&'static str> {
    let s = stderr.to_lowercase();
    let has = |n: &str| s.contains(n);
    Some(
        if has("authentication failed")
            || has("could not read username")
            || has("permission denied (publickey)")
        {
            "authentication_failed"
        } else if has("host key verification failed") {
            "host_key_unverified"
        } else if has("not a git repository") {
            "not_a_repository"
        } else if has("is already used by worktree") || has("already checked out at") {
            "branch_checked_out_in_worktree"
        } else if has("a branch named") && has("already exists") {
            "branch_already_exists"
        } else if has("already exists and is not an empty directory") {
            "path_already_exists"
        } else if has("non-fast-forward") || has("fetch first") {
            "push_rejected_non_fast_forward"
        } else if has("remote rejected")
            || has("pre-receive hook declined")
            || has("[remote rejected]")
        {
            "push_rejected_by_remote"
        } else if has("would clobber existing tag") {
            "tag_would_be_clobbered"
        } else if has("could not resolve host")
            || has("could not read from remote")
            || has("unable to access")
            || has("does not appear to be a git repository")
        {
            "remote_unreachable"
        } else if has("unknown revision")
            || has("invalid reference")
            || has("not a valid ref")
            || has("did not match any")
            || has("not a valid object name")
        {
            "ref_not_found"
        } else {
            return None;
        },
    )
}
pub fn git_fail(op: &str, cwd: &Path, args: &[&str], o: &Out) -> Value {
    let detail = if o.stderr.is_empty() {
        format!(
            "git {} exited with code {:?}",
            args.first().unwrap_or(&""),
            o.code
        )
    } else {
        o.stderr.chars().take(2000).collect()
    };
    let mut e = json!({"_tag":"GitCommandError","operation":op,"command":format!("git {}", args.first().unwrap_or(&"")),"cwd":cwd.to_string_lossy(),"argumentCount":args.len(),"stderrLength":o.stderr.len(),"detail":detail,"message":detail});
    if let Some(c) = o.code {
        e["exitCode"] = json!(c);
    }
    if let Some(r) = reason_of(&o.stderr) {
        e["reason"] = json!(r);
    }
    e
}
pub fn simple_fail(op: &str, cwd: &Path, detail: &str, reason: Option<&str>) -> Value {
    let mut e = json!({"_tag":"GitCommandError","operation":op,"command":"git","cwd":cwd.to_string_lossy(),"detail":detail,"message":detail});
    if let Some(r) = reason {
        e["reason"] = json!(r);
    }
    e
}

/// Runs git and returns its output, mapping spawn/timeout failures to wire errors (non-zero exit is the caller's call).
pub async fn out(op: &str, cwd: &Path, args: &[&str], opts: Opts) -> Result<Out, Value> {
    proc::git(cwd, args, opts)
        .await
        .map_err(|e| run_err(op, cwd, e))
}
/// Like `out` but a non-zero exit is a `GitCommandError`.
pub async fn ok(op: &str, cwd: &Path, args: &[&str], opts: Opts) -> Result<Out, Value> {
    let o = out(op, cwd, args, opts).await?;
    if o.ok() {
        Ok(o)
    } else {
        Err(git_fail(op, cwd, args, &o))
    }
}

/// An existing absolute directory named by the client (T3 trusts `cwd`; it only has to exist).
pub fn existing_dir(p: &Value) -> Option<PathBuf> {
    let raw = p["cwd"]
        .as_str()
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.contains('\0'))?;
    let path = paths::normalize(&paths::expand_home(raw));
    (path.is_absolute() && path.is_dir()).then_some(path)
}
fn need_dir(op: &str, p: &Value) -> Result<PathBuf, Value> {
    existing_dir(p).ok_or_else(|| {
        simple_fail(
            op,
            Path::new(p["cwd"].as_str().unwrap_or("")),
            "The working directory does not exist.",
            Some("not_a_repository"),
        )
    })
}

/// Ref names are passed to git as plain arguments, so they must not look like options.
pub fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && !name.starts_with('-')
        && !name.contains(['\0', '\n', '\r', ' ', '~', '^', ':', '?', '*', '[', '\\'])
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
}

// ------------------------------------------------------------------ repository facts

pub async fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let o = proc::git(cwd, &["rev-parse", "--show-toplevel"], Opts::READ)
        .await
        .ok()?;
    o.ok().then(|| PathBuf::from(o.text().trim()))
}
pub async fn default_branch(cwd: &Path) -> Option<String> {
    if let Ok(o) = proc::git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        Opts::READ,
    )
    .await
    {
        if o.ok() {
            if let Some(b) = o.text().trim().strip_prefix("origin/") {
                return Some(b.to_owned());
            }
        }
    }
    for candidate in ["main", "master", "trunk"] {
        let full = format!("refs/heads/{candidate}");
        if proc::git(cwd, &["show-ref", "--verify", "--quiet", &full], Opts::READ)
            .await
            .is_ok_and(|o| o.ok())
        {
            return Some(candidate.to_owned());
        }
    }
    None
}
async fn has_remote(cwd: &Path) -> bool {
    proc::git(cwd, &["remote"], Opts::READ)
        .await
        .is_ok_and(|o| o.ok() && o.text().lines().any(|l| !l.trim().is_empty()))
}

// ------------------------------------------------------------------ vcs.refreshStatus

fn not_a_repo_status() -> Value {
    json!({"isRepo":false,"hasPrimaryRemote":false,"isDefaultRef":false,"refName":null,"hasWorkingTreeChanges":false,
        "workingTree":{"files":[],"insertions":0,"deletions":0},"hasUpstream":false,"aheadCount":0,"behindCount":0,"aheadOfDefaultCount":0,"pr":null})
}

#[derive(Default)]
pub struct PorcelainV2 {
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub unborn: bool,
    pub changed: Vec<String>,
    pub untracked: Vec<String>,
}
pub fn parse_porcelain_v2(raw: &[u8]) -> PorcelainV2 {
    let mut st = PorcelainV2::default();
    let mut fields = raw
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(f) = fields.next() {
        if let Some(h) = f.strip_prefix("# branch.head ") {
            st.head = (h != "(detached)").then(|| h.to_owned());
        } else if let Some(o) = f.strip_prefix("# branch.oid ") {
            st.unborn = o == "(initial)";
        } else if let Some(u) = f.strip_prefix("# branch.upstream ") {
            st.upstream = Some(u.to_owned());
        } else if let Some(ab) = f.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ');
            st.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            st.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if let Some(path) = f.strip_prefix("? ") {
            st.untracked.push(path.to_owned());
        } else if f.starts_with("1 ") || f.starts_with("u ") || f.starts_with("2 ") {
            let before = match &f[..1] {
                "1" => 8,
                "2" => 9,
                _ => 10,
            };
            if let Some(path) = f.splitn(before + 1, ' ').nth(before) {
                st.changed.push(path.to_owned());
            }
            if f.starts_with("2 ") {
                fields.next(); // original path of a rename
            }
        }
    }
    st
}

fn parse_numstat(raw: &[u8]) -> Vec<(String, u64, u64)> {
    let mut files = Vec::new();
    let fields: Vec<String> = raw
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let mut i = 0;
    while i < fields.len() {
        let mut parts = fields[i].splitn(3, '\t');
        if let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) {
            let num = |s: &str| {
                if s == "-" {
                    Some(0)
                } else {
                    s.parse::<u64>().ok()
                }
            };
            if let (Some(a), Some(d)) = (num(a), num(d)) {
                let path = if p.is_empty() {
                    i += 2;
                    fields.get(i).cloned().unwrap_or_default()
                } else {
                    p.to_owned()
                };
                files.push((path, a, d));
            }
        }
        i += 1;
    }
    files
}

fn count_lines(path: &Path) -> u64 {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else {
        return 0;
    };
    let mut data = Vec::new();
    if f.take(1024 * 1024).read_to_end(&mut data).is_err() || data.contains(&0) || data.is_empty() {
        return 0;
    }
    data.iter().filter(|b| **b == b'\n').count() as u64 + u64::from(data.last() != Some(&b'\n'))
}

// Successful host reads are retained briefly; an authoritative empty result clears the entry.
fn last_known_pr(key: String, read: Option<Value>) -> Value {
    use std::{
        collections::VecDeque,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };
    type Entries = VecDeque<(String, Instant, Value)>;
    static CACHE: OnceLock<Mutex<Entries>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Mutex::default).lock().unwrap();
    cache.retain(|(_, at, _)| at.elapsed() < Duration::from_secs(300));
    if let Some(value) = read {
        cache.retain(|(k, _, _)| *k != key);
        if !value.is_null() {
            cache.push_back((key, Instant::now(), value.clone()));
            while cache.len() > 128 {
                cache.pop_front();
            }
        }
        value
    } else {
        cache
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, _, v)| v.clone())
            .unwrap_or(Value::Null)
    }
}

pub async fn refresh_status(p: Value) -> Reply {
    let op = "vcs.refreshStatus";
    let Some(cwd) = existing_dir(&p) else {
        return Ok(not_a_repo_status());
    };
    let Some(root) = toplevel(&cwd).await else {
        return Ok(not_a_repo_status());
    };
    let status = ok(
        op,
        &root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
            "-z",
        ],
        Opts::READ,
    )
    .await?;
    let st = parse_porcelain_v2(&status.stdout);
    let base = if st.unborn { EMPTY_TREE } else { "HEAD" };
    let num = out(
        op,
        &root,
        &[
            "diff",
            "--numstat",
            "--no-renames",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            base,
            "--",
        ],
        Opts {
            cap: 16 * 1024 * 1024,
            ..Opts::READ
        },
    )
    .await?;
    let mut files = if num.ok() {
        parse_numstat(&num.stdout)
    } else {
        Vec::new()
    };
    let untracked = st.untracked.iter().take(2000).cloned().collect::<Vec<_>>();
    let root2 = root.clone();
    let counted = crate::files::blocking(move || {
        untracked
            .into_iter()
            .map(|u| {
                let n = count_lines(&root2.join(&u));
                (u, n, 0)
            })
            .collect::<Vec<_>>()
    })
    .await?;
    files.extend(counted);
    let (ins, del) = files.iter().fold((0, 0), |a, f| (a.0 + f.1, a.1 + f.2));
    let default = default_branch(&root).await;
    let is_default = st.head.is_some() && st.head == default;
    let mut v = json!({
        "isRepo":true,"hasPrimaryRemote":has_remote(&root).await,"isDefaultRef":is_default,"refName":st.head,
        "hasWorkingTreeChanges":!st.changed.is_empty() || !st.untracked.is_empty(),
        "workingTree":{"rootPath":root.to_string_lossy(),"files":files.iter().map(|f|json!({"path":f.0,"insertions":f.1,"deletions":f.2})).collect::<Vec<_>>(),"insertions":ins,"deletions":del},
        "hasUpstream":st.upstream.is_some(),"aheadCount":st.ahead,"behindCount":st.behind,"aheadOfDefaultCount":0,"branchChanges":{"baseRef":null,"insertions":0,"deletions":0},"pr":null});
    if let (Some(head), Some(def)) = (&st.head, &default) {
        if head != def && !st.unborn {
            let range = format!("{def}...HEAD");
            let bn = out(
                op,
                &root,
                &[
                    "diff",
                    "--numstat",
                    "--no-renames",
                    "-z",
                    "--no-ext-diff",
                    "--no-textconv",
                    &range,
                    "--",
                ],
                Opts {
                    cap: 16 * 1024 * 1024,
                    ..Opts::READ
                },
            )
            .await?;
            let (bi, bd) = if bn.ok() {
                parse_numstat(&bn.stdout)
                    .iter()
                    .fold((0, 0), |a, f| (a.0 + f.1, a.1 + f.2))
            } else {
                (0, 0)
            };
            v["branchChanges"] = json!({"baseRef":def,"insertions":bi,"deletions":bd});
            let ahead = format!("{def}..HEAD");
            if let Ok(c) = out(op, &root, &["rev-list", "--count", &ahead], Opts::READ).await {
                if let Some(n) = c
                    .ok()
                    .then(|| c.text().trim().parse::<u64>().ok())
                    .flatten()
                {
                    v["aheadOfDefaultCount"] = json!(n);
                }
            }
        }
    }
    let mut identity = crate::projects::repository_identity(&root.to_string_lossy()).await;
    let mut host = identity["canonicalKey"]
        .as_str()
        .and_then(|k| k.split('/').next())
        .unwrap_or("")
        .to_owned();
    if let Some(remote) = identity["locator"]["remoteUrl"]
        .as_str()
        .and_then(|s| url::Url::parse(s).ok())
    {
        if let Some(name) = remote.host_str() {
            host = format!(
                "{name}{}",
                remote.port().map(|p| format!(":{p}")).unwrap_or_default()
            );
        }
    }
    if identity["provider"].is_null()
        && std::env::var("GITLAB_HOST").ok().is_some_and(|configured| {
            let configured = configured
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
            !host.is_empty() && configured.eq_ignore_ascii_case(&host)
        })
    {
        identity["provider"] = json!("gitlab");
    }

    let mut host_read = false;
    if let Some(kind) = identity["provider"].as_str() {
        let (name, base) = if kind == "github" {
            ("GitHub", "https://github.com")
        } else {
            ("GitLab", "https://gitlab.com")
        };
        v["sourceControlProvider"] = json!({"kind":kind,"name":name,"baseUrl":if host.is_empty() { base.to_owned() } else { format!("https://{host}") }});
        if kind == "github" && p["localOnly"] != true {
            if let Some(head) = st.head.as_deref() {
                let result = proc::run(
                    "gh",
                    &root,
                    &[
                        "pr",
                        "list",
                        "--head",
                        head,
                        "--state",
                        "all",
                        "--limit",
                        "1",
                        "--json",
                        "number,title,url,baseRefName,headRefName,state,isDraft,updatedAt",
                    ],
                    Opts::READ,
                    &[],
                )
                .await;
                if let Ok(o) = result {
                    if o.ok() {
                        if let Ok(rows) = serde_json::from_slice::<Vec<Value>>(&o.stdout) {
                            host_read = true;
                            if let Some(pr) = rows.first() {
                                v["pr"] = json!({"number":pr["number"],"title":pr["title"],"url":pr["url"],"baseRef":pr["baseRefName"],"headRef":pr["headRefName"],"state":pr["state"].as_str().unwrap_or("OPEN").to_lowercase(),"isDraft":pr["isDraft"],"updatedAt":pr["updatedAt"]});
                            }
                        }
                    }
                }
            }
        }
        if kind == "gitlab" && p["localOnly"] != true {
            if let Some(head) = st.head.as_deref() {
                if let Ok(o) = proc::run(
                    "glab",
                    &root,
                    &[
                        "mr",
                        "list",
                        "--source-branch",
                        head,
                        "--all",
                        "--per-page",
                        "1",
                        "--output",
                        "json",
                        "--order",
                        "updated_at",
                        "--sort",
                        "desc",
                    ],
                    Opts::READ,
                    &[
                        ("GLAB_SEND_TELEMETRY", Path::new("false")),
                        ("GLAB_CHECK_UPDATE", Path::new("false")),
                    ],
                )
                .await
                {
                    if o.ok() {
                        if let Ok(rows) = serde_json::from_slice::<Vec<Value>>(&o.stdout) {
                            host_read = true;
                            if let Some(pr) = rows.first().and_then(gitlab_status_pr) {
                                if !default
                                    .as_deref()
                                    .is_some_and(|d| Some(d) == st.head.as_deref())
                                    || pr["state"] == "open"
                                {
                                    v["pr"] = pr;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if p["localOnly"] != true && matches!(identity["provider"].as_str(), Some("github" | "gitlab"))
    {
        if let (Some(repo), Some(head)) = (identity["canonicalKey"].as_str(), st.head.as_deref()) {
            let key =
                serde_json::to_string(&(root.to_string_lossy(), repo, head)).unwrap_or_default();
            v["pr"] = last_known_pr(key, host_read.then(|| v["pr"].clone()));
        }
    }
    Ok(v)
}

fn gitlab_status_pr(pr: &Value) -> Option<Value> {
    let state = match pr["state"].as_str()? {
        "opened" | "open" => "open",
        "closed" => "closed",
        "merged" => "merged",
        _ => return None,
    };
    Some(
        json!({"number":pr["iid"].as_u64().filter(|n| *n > 0)?,"title":pr["title"].as_str()?,"url":pr["web_url"].as_str()?,"baseRef":pr["target_branch"].as_str()?,"headRef":pr["source_branch"].as_str()?,"state":state,"isDraft":pr["draft"].as_bool().or_else(|| pr["work_in_progress"].as_bool()).unwrap_or(false),"updatedAt":pr["updated_at"]}),
    )
}

// ------------------------------------------------------------------ vcs.listRefs

/// (commit time, wire ref)
type Ref = (i64, Value);

pub async fn list_refs(p: Value) -> Reply {
    let op = "vcs.listRefs";
    let empty =
        json!({"refs":[],"isRepo":false,"hasPrimaryRemote":false,"nextCursor":null,"totalCount":0});
    let Some(cwd) = existing_dir(&p) else {
        return Ok(empty);
    };
    let Some(root) = toplevel(&cwd).await else {
        return Ok(empty);
    };
    let fmt = "%(refname)%09%(committerdate:unix)%09%(symref)%09%(upstream)%09%(HEAD)";
    let format_arg = format!("--format={fmt}");
    let refs_out = ok(
        op,
        &root,
        &["for-each-ref", &format_arg, "refs/heads", "refs/remotes"],
        Opts::READ,
    )
    .await?;
    let default = default_branch(&root).await;
    let remotes: Vec<String> = out(op, &root, &["remote"], Opts::READ)
        .await
        .map(|o| o.text().lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let mut worktrees = std::collections::HashMap::new();
    if let Ok(o) = out(op, &root, &["worktree", "list", "--porcelain"], Opts::READ).await {
        let (mut path, mut branch) = (None::<String>, None::<String>);
        for line in o.text().lines().chain(std::iter::once("")) {
            if let Some(w) = line.strip_prefix("worktree ") {
                path = Some(w.to_owned());
                branch = None;
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_owned());
            } else if line.is_empty() {
                if let (Some(pa), Some(br)) = (path.take(), branch.take()) {
                    if Path::new(&pa).exists() {
                        worktrees.insert(br, pa);
                    }
                }
            }
        }
    }
    let toplevel_s = root.to_string_lossy().into_owned();
    let (mut local, mut remote): (Vec<Ref>, Vec<Ref>) = (Vec::new(), Vec::new());
    for line in refs_out.text().lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(full), time, sym, upstream, head) = (
            f.first().copied(),
            f.get(1).and_then(|t| t.parse::<i64>().ok()).unwrap_or(0),
            f.get(2).copied().unwrap_or(""),
            f.get(3).copied().unwrap_or(""),
            f.get(4).copied().unwrap_or(""),
        ) else {
            continue;
        };
        if !sym.is_empty() {
            continue;
        }
        if let Some(name) = full.strip_prefix("refs/heads/") {
            let wt = worktrees
                .get(name)
                .cloned()
                .or_else(|| (head == "*").then(|| toplevel_s.clone()));
            let mut r = json!({"name":name,"current":head=="*","isRemote":false,"isDefault":Some(name)==default.as_deref(),"worktreePath":wt});
            if !upstream.is_empty() {
                r["upstreamRef"] = json!(upstream);
            }
            local.push((time, r));
        } else if let Some(name) = full.strip_prefix("refs/remotes/") {
            let remote_name = remotes
                .iter()
                .filter(|r| name.starts_with(&format!("{r}/")))
                .max_by_key(|r| r.len());
            let branch = remote_name.map(|r| &name[r.len() + 1..]);
            let mut r = json!({"name":name,"current":false,"isRemote":true,"isDefault":remote_name.map(String::as_str)==Some("origin") && branch.is_some() && branch==default.as_deref(),"worktreePath":null});
            if let Some(rn) = remote_name {
                r["remoteName"] = json!(rn);
            }
            remote.push((time, r));
        }
    }
    let by_recency = |a: &(i64, Value), b: &(i64, Value)| {
        b.0.cmp(&a.0)
            .then(a.1["name"].as_str().cmp(&b.1["name"].as_str()))
    };
    local.sort_by(by_recency);
    remote.sort_by(by_recency);
    let local_names: std::collections::HashSet<String> = local
        .iter()
        .filter_map(|l| l.1["name"].as_str().map(str::to_owned))
        .collect();
    let include_matching = p["includeMatchingRemoteRefs"] == true;
    let mut all: Vec<Value> = local.into_iter().map(|l| l.1).collect();
    all.extend(remote.into_iter().map(|r| r.1).filter(|r| {
        include_matching
            || !r["remoteName"]
                .as_str()
                .zip(r["name"].as_str())
                .is_some_and(|(rn, n)| local_names.contains(&n[rn.len() + 1..]))
    }));
    all.sort_by_key(|r| {
        if r["current"] == true {
            0
        } else if r["isDefault"] == true {
            1
        } else {
            2
        }
    });
    match p["refKind"].as_str() {
        Some("local") => all.retain(|r| r["isRemote"] != true),
        Some("remote") => all.retain(|r| r["isRemote"] == true),
        _ => {}
    }
    if let Some(q) = p["query"]
        .as_str()
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty())
    {
        all.retain(|r| {
            r["name"]
                .as_str()
                .is_some_and(|n| n.to_lowercase().contains(&q))
        });
    }
    let total = all.len();
    let cursor = p["cursor"].as_u64().unwrap_or(0) as usize;
    let limit = p["limit"].as_u64().unwrap_or(100).clamp(1, 200) as usize;
    let page: Vec<Value> = all.into_iter().skip(cursor).take(limit).collect();
    let next = (cursor + page.len() < total).then_some(cursor + page.len());
    let mut v = json!({"refs":page,"isRepo":true,"hasPrimaryRemote":!remotes.is_empty(),"nextCursor":next,"totalCount":total});
    v["isAtRepositoryRoot"] =
        json!(std::fs::canonicalize(&cwd).ok() == std::fs::canonicalize(&root).ok());
    Ok(v)
}

// ------------------------------------------------------------------ mutations

async fn current_branch(cwd: &Path) -> Option<String> {
    let o = proc::git(cwd, &["branch", "--show-current"], Opts::READ)
        .await
        .ok()?;
    Some(o.text().trim().to_owned()).filter(|b| o.ok() && !b.is_empty())
}
fn ref_arg(op: &str, cwd: &Path, name: &Value) -> Result<String, Value> {
    name.as_str()
        .map(str::trim)
        .filter(|n| valid_ref_name(n))
        .map(str::to_owned)
        .ok_or_else(|| simple_fail(op, cwd, "Invalid ref name.", Some("ref_not_found")))
}
async fn ref_exists(cwd: &Path, full: &str) -> bool {
    proc::git(cwd, &["show-ref", "--verify", "--quiet", full], Opts::READ)
        .await
        .is_ok_and(|o| o.ok())
}

pub async fn create_ref(p: Value) -> Reply {
    let op = "vcs.createRef";
    let cwd = need_dir(op, &p)?;
    let name = ref_arg(op, &cwd, &p["refName"])?;
    let _guard = proc::lock_repo(&cwd).await;
    if ref_exists(&cwd, &format!("refs/heads/{name}")).await {
        return Err(simple_fail(
            op,
            &cwd,
            &format!("A branch named '{name}' already exists."),
            Some("branch_already_exists"),
        ));
    }
    if p["switchRef"] == true {
        ok(op, &cwd, &["switch", "-c", &name], Opts::WRITE).await?;
    } else {
        ok(op, &cwd, &["branch", &name], Opts::WRITE).await?;
    }
    Ok(json!({"refName":name}))
}

pub async fn switch_ref(p: Value) -> Reply {
    let op = "vcs.switchRef";
    let cwd = need_dir(op, &p)?;
    let name = ref_arg(op, &cwd, &p["refName"])?;
    let _guard = proc::lock_repo(&cwd).await;
    if ref_exists(&cwd, &format!("refs/heads/{name}")).await {
        ok(op, &cwd, &["switch", &name], Opts::WRITE).await?;
    } else if ref_exists(&cwd, &format!("refs/remotes/{name}")).await {
        // A remote ref becomes a local tracking branch of the same name.
        let local = name
            .split_once('/')
            .map(|(_, b)| b.to_owned())
            .unwrap_or(name.clone());
        if ref_exists(&cwd, &format!("refs/heads/{local}")).await {
            ok(op, &cwd, &["switch", &local], Opts::WRITE).await?;
        } else {
            ok(
                op,
                &cwd,
                &["switch", "-c", &local, "--track", &name],
                Opts::WRITE,
            )
            .await?;
        }
    } else {
        return Err(simple_fail(
            op,
            &cwd,
            &format!("Ref '{name}' was not found."),
            Some("ref_not_found"),
        ));
    }
    Ok(json!({"refName":current_branch(&cwd).await}))
}

pub async fn pull(p: Value) -> Reply {
    let op = "vcs.pull";
    let cwd = need_dir(op, &p)?;
    let _guard = proc::lock_repo(&cwd).await;
    let Some(branch) = current_branch(&cwd).await else {
        return Err(simple_fail(
            op,
            &cwd,
            "Cannot pull from a detached HEAD.",
            None,
        ));
    };
    let upstream = out(
        op,
        &cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        Opts::READ,
    )
    .await?;
    if !upstream.ok() {
        return Err(simple_fail(
            op,
            &cwd,
            &format!("Branch '{branch}' has no upstream to pull from."),
            None,
        ));
    }
    let upstream_ref = upstream.text().trim().to_owned();
    let before = out(op, &cwd, &["rev-parse", "HEAD"], Opts::READ)
        .await?
        .text();
    ok(op, &cwd, &["pull", "--ff-only"], Opts::NETWORK).await?;
    let after = out(op, &cwd, &["rev-parse", "HEAD"], Opts::READ)
        .await?
        .text();
    Ok(
        json!({"status":if before==after {"skipped_up_to_date"} else {"pulled"},"refName":branch,"upstreamRef":upstream_ref}),
    )
}

pub async fn init(p: Value) -> Reply {
    let op = "vcs.init";
    let cwd = need_dir(op, &p)?;
    if matches!(p["kind"].as_str(), Some("jj")) {
        return Err(
            json!({"_tag":"VcsUnsupportedOperationError","operation":op,"kind":"jj","detail":"o3-server initializes Git repositories only.","message":"o3-server initializes Git repositories only."}),
        );
    }
    let _guard = proc::lock_repo(&cwd).await;
    ok(op, &cwd, &["init"], Opts::WRITE).await?;
    Ok(Value::Null)
}

/// Discovery probes are bounded and do not initiate provider login or installation. They run
/// concurrently: sequential CLI auth checks (gh contacts its host) could exceed a client timeout.
pub async fn discover() -> Reply {
    discover_with(
        |name| name.to_owned(),
        |key| std::env::var(key).ok(),
        std::time::Duration::from_secs(5),
    )
    .await
}
pub async fn discover_for(state: &crate::store::State) -> Reply {
    discover_context(
        |name| name.to_owned(),
        |key| std::env::var(key).ok(),
        std::time::Duration::from_secs(5),
        Some(state),
    )
    .await
}
async fn discover_with(
    executable: impl Fn(&str) -> String + Copy,
    environment: impl Fn(&str) -> Option<String> + Copy,
    probe_timeout: std::time::Duration,
) -> Reply {
    discover_context(executable, environment, probe_timeout, None).await
}
async fn discover_context(
    executable: impl Fn(&str) -> String + Copy,
    environment: impl Fn(&str) -> Option<String> + Copy,
    probe_timeout: std::time::Duration,
    state: Option<&crate::store::State>,
) -> Reply {
    let jobs = [
        (
            "git",
            "Git",
            true,
            "Install Git from https://git-scm.com/downloads or with your package manager.",
        ),
        (
            "jj",
            "Jujutsu",
            false,
            "Install Jujutsu with `brew install jj` or from https://github.com/jj-vcs/jj.",
        ),
    ]
    .into_iter()
    .map(|(kind, label, implemented, hint)| { let command=executable(kind);async move {
        let output = proc::run(
            &command,
            Path::new("/"),
            &["--version"],
            Opts {
                timeout: probe_timeout,
                cap: 8000,
                allow_truncate: true,
            },
            &[],
        )
        .await;
        let (status, version, detail) = match output {
            Ok(o) if o.ok() => (
                "available",
                o.text().lines().next().map(str::to_owned),
                None,
            ),
            Ok(o) => ("missing", None, Some(o.stderr)),
            Err(e) => ("missing", None, Some(format!("{e:?}"))),
        };
        let option = |v: Option<String>| match v {
            Some(v) => json!({"_tag":"Some","value":v}),
            None => json!({"_tag":"None"}),
        };
        json!({"kind":kind,"label":label,"executable":kind,"implemented":implemented,"status":status,"version":option(version),"installHint":hint,"detail":option(detail)})
    }});
    let systems = futures_util::future::join_all(jobs);
    let option = |v: Option<String>| match v {
        Some(v) if !v.trim().is_empty() => json!({"_tag":"Some","value":v.trim()}),
        _ => json!({"_tag":"None"}),
    };
    let probe = || Opts {
        timeout: probe_timeout,
        cap: 16_000,
        allow_truncate: true,
    };
    // CLI-backed hosts, as upstream: version probe, then a read-only auth status. Never logs in.
    let jobs = [
        (
            "github",
            "GitHub",
            "gh",
            &["auth", "status", "--json", "hosts"][..],
            "Install the GitHub command-line tool (`gh`) via https://cli.github.com/ or your package manager (for example `brew install gh`).",
        ),
        (
            "gitlab",
            "GitLab",
            "glab",
            &["auth", "status"][..],
            "Install the GitLab command-line tool (`glab`) via https://gitlab.com/gitlab-org/cli or your package manager (for example `brew install glab`).",
        ),
        (
            "azure-devops",
            "Azure DevOps",
            "az",
            &["account", "show", "--query", "user.name", "-o", "tsv"][..],
            "Install the Azure command-line tools (`az`), then enable Azure DevOps support with `az extension add --name azure-devops`.",
        ),
    ]
    .into_iter()
    .map(|(kind, label, exe, auth_args, hint)| { let command=executable(exe);async move {
        let version = proc::run(&command, Path::new("/"), &["--version"], probe(), &[]).await;
        let (status, version, detail) = match version {
            Ok(o) if o.ok() => ("available", o.text().lines().next().map(str::to_owned), None),
            Ok(o) => ("missing", None, Some(o.stderr)),
            Err(e) => ("missing", None, Some(format!("{e:?}"))),
        };
        let mut auth = json!({"status":"unknown","account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}});
        if status == "available" {
            match proc::run(&command, Path::new("/"), auth_args, probe(), &[]).await {
                Ok(o) if o.ok() => {
                    auth["status"] = json!("authenticated");
                    let text = o.text();
                    if kind == "github" {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            let entry = v["hosts"].as_object().into_iter().flatten().flat_map(|(h, list)| {
                                list.as_array().into_iter().flatten().map(move |e| (h.clone(), e.clone()))
                            }).find(|(_, e)| e["active"] == true && e["state"] == "success");
                            if let Some((host, e)) = entry {
                                auth["host"] = option(Some(host));
                                auth["account"] = option(e["login"].as_str().map(str::to_owned));
                            }
                        }
                    } else if kind == "azure-devops" {
                        auth["account"] = option(Some(text));
                    }
                }
                Ok(_o) => {
                    auth["status"] = json!("unauthenticated");
                    auth["detail"] = option(Some("Credential probe failed; inspect the CLI locally.".into()));
                }
                Err(_) => {}
            }
        }
        if matches!(kind, "forgejo" | "gitcafe") && status == "available" {
            // Exit status alone cannot distinguish offline/refused/unparseable auth.
            auth["status"] = json!("unknown");
        }
        json!({"kind":kind,"label":label,"executable":exe,"status":status,"version":option(version),"installHint":hint,"detail":option(detail),"auth":auth})
    }});
    let cli_providers = futures_util::future::join_all(jobs);
    let jobs = [
        (
            "bitbucket",
            "Bitbucket",
            "BITBUCKET_ACCESS_TOKEN",
            Some("https://api.bitbucket.org/2.0/user".to_owned()),
        ),
        (
            "forgejo",
            "Forgejo / Gitea",
            "FORGEJO_TOKEN",
            environment("FORGEJO_URL").and_then(|url| {
                let mut url = url::Url::parse(&url).ok()?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return None;
                }
                url.set_path(&format!("{}/api/v1/user", url.path().trim_end_matches('/')));
                Some(url.to_string())
            }),
        ),
        (
            "gitcafe",
            "GitCafe",
            "CAFE_TOKEN",
            Some("https://git.cafe/api/auth/principal".to_owned()),
        ),
    ]
    .into_iter()
    .map(|(kind, label, token_env, endpoint)| { let cafe=executable("cafe");async move {
        if let Some(state) = state.filter(|_| matches!(kind, "bitbucket" | "gitcafe")) {
            if let Some(auth) = crate::source_credentials_runtime::auth(state, kind).await {
                return json!({"kind":kind,"label":label,"status":"available","version":{"_tag":"None"},"installHint":format!("Configure {token_env} on the server."),"detail":option(Some("Settings credentials and native secret references supported for authentication discovery.".into())),"auth":auth});
            }
        }
        let token = if state.is_some() && kind == "gitcafe" {
            None
        } else {
            environment(token_env).filter(|s| !s.trim().is_empty())
        };
        let mut auth = json!({"status":"unknown","account":{"_tag":"None"},"host":{"_tag":"None"},"detail":{"_tag":"None"}});
        if let Some(endpoint) = endpoint.as_deref() {
            auth["host"] = option(
                url::Url::parse(endpoint)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned)),
            );
            if let Some(token) = token.as_deref() {
                auth = api_token_auth(endpoint, token).await;
            } else if kind == "gitcafe" {
                // Documented authenticated API command: the CLI owns its stored
                // login. Never retrieve a token or expose CLI output/errors.
                if let Ok(out) = proc::run(
                    &cafe,
                    Path::new("/"),
                    &[
                        "--host",
                        "https://git.cafe/api",
                        "--no-update-check",
                        "--no-input",
                        "--json",
                        "api",
                        "/auth/principal",
                    ],
                    Opts {
                        timeout: probe_timeout,
                        cap: 65536,
                        allow_truncate: false,
                    },
                    &[],
                )
                .await
                {
                    if out.ok() {
                        if let Ok(principal) = serde_json::from_slice::<Value>(&out.stdout) {
                            if principal["actorId"].as_str().is_some_and(|v| !v.is_empty()) {
                                if let Some(handle) = principal["handle"]
                                    .as_str()
                                    .filter(|v| !v.is_empty() && v.len() <= 200)
                                {
                                    auth["status"] = json!("authenticated");
                                    auth["account"] = option(Some(handle.into()));
                                }
                            }
                        }
                    }
                }
            } else {
                auth["status"] = json!("unauthenticated");
            }
        }
        let detail = if kind == "gitcafe" {
            "CAFE_TOKEN or cafe CLI login supported; CLI failures remain unknown."
        } else if kind == "forgejo" && auth["host"]["_tag"] == "None" {
            "Configure an explicit HTTPS FORGEJO_URL; credentials are never sent to repository-selected hosts."
        } else {
            "Environment credentials supported; desktop credential storage is not implemented."
        };
        json!({"kind":kind,"label":label,"status":"available","version":{"_tag":"None"},"installHint":format!("Configure {token_env} on the server."),"detail":option(Some(detail.into())),"auth":auth})
    }});
    let token_providers = futures_util::future::join_all(jobs);
    let (systems, mut providers, tokens) = tokio::join!(systems, cli_providers, token_providers);
    providers.extend(tokens);
    Ok(json!({"versionControlSystems":systems,"sourceControlProviders":providers}))
}

/// Credential probes never return response bodies, transport errors, or tokens.
async fn api_token_auth(endpoint: &str, token: &str) -> Value {
    let option = |v: Option<String>| {
        v.map(|v| json!({"_tag":"Some","value":v}))
            .unwrap_or_else(|| json!({"_tag":"None"}))
    };
    let host = url::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned));
    let mut auth = json!({"status":"unknown","account":{"_tag":"None"},"host":option(host),"detail":{"_tag":"None"}});
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return auth;
    };
    let Ok(mut response) = client
        .get(endpoint)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
    else {
        return auth;
    };
    if response.status().as_u16() == 401 {
        auth["status"] = json!("unauthenticated");
        return auth;
    }
    if !response.status().is_success() {
        return auth;
    }
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len() + chunk.len() > 65536 {
            return auth;
        }
        body.extend_from_slice(&chunk);
    }
    if let Ok(user) = serde_json::from_slice::<Value>(&body) {
        let fields: &[&str] = if endpoint.ends_with("/auth/principal") {
            if !user["actorId"].as_str().is_some_and(|s| !s.is_empty()) {
                return auth;
            }
            &["handle"]
        } else {
            &["login", "username", "account_id"]
        };
        let account = fields
            .iter()
            .copied()
            .find_map(|k| user[k].as_str())
            .filter(|s| !s.is_empty() && s.len() <= 200 && *s != token);
        if let Some(account) = account {
            auth["status"] = json!("authenticated");
            auth["account"] = option(Some(account.into()));
        }
    }
    auth
}

#[cfg(test)]
mod token_tests {
    use super::*;
    #[tokio::test]
    async fn token_status_is_bounded_and_never_echoes_body_or_credential() {
        for (status, body, expected) in [
            ("200 OK", r#"{"login":"reader"}"#, "authenticated"),
            (
                "200 OK",
                r#"{"actorId":"fixture-actor","handle":"reader","displayName":null,"avatarUrl":null}"#,
                "authenticated",
            ),
            ("401 Unauthorized", "fixture-secret", "unauthenticated"),
            ("403 Forbidden", "fixture-secret", "unknown"),
            ("200 OK", "fixture-secret", "unknown"),
        ] {
            use std::io::{Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let path = if body.contains("actorId") {
                "/auth/principal"
            } else {
                "/user"
            };
            let auth = api_token_auth(&format!("http://{address}{path}"), "fixture-secret").await;
            assert_eq!(auth["status"], expected);
            assert!(!auth.to_string().contains("fixture-secret"));
            worker.join().unwrap();
        }
    }
}

#[cfg(test)]
mod discovery_budget_tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "discovery-budget-{}",
            crate::store::random_id().unwrap()
        ));
        fs::create_dir(&p).unwrap();
        p
    }
    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
    fn script(root: &Path, name: &str, body: &str) -> String {
        let p = root.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p.to_string_lossy().into_owned()
    }
    #[tokio::test]
    async fn independent_vcs_and_host_probes_start_together() {
        let root = root();
        let started = quote(&root.join("auth-started"));
        let done = quote(&root.join("git-done"));
        let git=script(&root,"git",&format!("while [ ! -f {started} ]; do /usr/bin/sleep 0.01; done\nprintf 'fixture-git 1.0\\n'\n: > {done}"));
        let gh=script(&root,"gh",&format!("if [ \"$1\" = '--version' ]; then printf 'fixture-gh 1.0\\n'; exit 0; fi\n: > {started}\nwhile [ ! -f {done} ]; do /usr/bin/sleep 0.01; done\nprintf '%s\\n' '{{\"hosts\":{{\"github.com\":[{{\"active\":true,\"state\":\"success\",\"login\":\"fixture\"}}]}}}}'"));
        let resolve = |name: &str| match name {
            "git" => git.clone(),
            "gh" => gh.clone(),
            _ => root.join("missing").to_string_lossy().into_owned(),
        };
        let value = discover_with(resolve, |_| None, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(value["versionControlSystems"][0]["status"], "available");
        assert_eq!(
            value["sourceControlProviders"][0]["auth"]["status"],
            "authenticated"
        );
        assert_eq!(
            value["sourceControlProviders"][0]["auth"]["account"]["value"],
            "fixture"
        );
        // Fixture trees are retained for private container teardown.
    }
    #[tokio::test]
    async fn timed_out_auth_is_unknown_and_its_owned_process_is_reaped() {
        let root = root();
        let pid_file = root.join("auth-pid");
        let git = script(&root, "git", "printf 'fixture-git 1.0\\n'");
        let gh=script(&root,"gh",&format!("if [ \"$1\" = '--version' ]; then printf 'fixture-gh 1.0\\n'; exit 0; fi\nprintf '%s' \"$$\" > {}\n/usr/bin/sleep 30",quote(&pid_file)));
        let resolve = |name: &str| match name {
            "git" => git.clone(),
            "gh" => gh.clone(),
            _ => root.join("missing").to_string_lossy().into_owned(),
        };
        let value = discover_with(resolve, |_| None, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(value["versionControlSystems"][0]["status"], "available");
        let host = &value["sourceControlProviders"][0];
        assert_eq!(host["status"], "available");
        assert_eq!(host["auth"]["status"], "unknown");
        let pid = fs::read_to_string(pid_file).unwrap();
        assert!(
            !Path::new("/proc").join(pid).exists(),
            "probe process was not reaped"
        );
        // Fixture trees are retained for private container teardown.
    }
}
