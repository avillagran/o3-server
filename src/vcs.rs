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
        .is_ok_and(|o| o.ok() && o.text().lines().any(|l| l.trim() == "origin"))
}

// ------------------------------------------------------------------ vcs.refreshStatus

fn not_a_repo_status() -> Value {
    json!({"isRepo":false,"hasPrimaryRemote":false,"isDefaultRef":false,"refName":null,"hasWorkingTreeChanges":false,
        "workingTree":{"files":[],"insertions":0,"deletions":0},"hasUpstream":false,"aheadCount":0,"behindCount":0,"pr":null})
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
        "hasUpstream":st.upstream.is_some(),"aheadCount":st.ahead,"behindCount":st.behind,"pr":null});
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
    Ok(v)
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
    let mut v = json!({"refs":page,"isRepo":true,"hasPrimaryRemote":remotes.iter().any(|r| r == "origin"),"nextCursor":next,"totalCount":total});
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
