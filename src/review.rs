//! `review.getDiffPreview` / `review.getDiffFileContents`: diffs of a workspace against HEAD and
//! against the merge base with the default branch, including untracked files (through a throwaway
//! index, so the real index is never touched). The cwd must be inside a known workspace.
use crate::{
    paths,
    proc::{self, Opts},
    store::{iso, now_ms, random_id, State},
    vcs::{default_branch, git_fail, out, run_err, simple_fail, toplevel, Reply, EMPTY_TREE},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const PATCH_MAX: usize = 120_000;
const FILE_PATCH_MAX: usize = 1024 * 1024;
const METADATA_MAX: usize = 16 * 1024 * 1024;
const UNTRACKED_MAX: usize = 5_000;
const FILE_MAX: usize = 1024 * 1024;

fn detection_error(op: &str, cwd: &str, detail: &str) -> Value {
    json!({"_tag":"VcsRepositoryDetectionError","operation":op,"cwd":cwd,"detail":detail,"message":detail})
}

fn bound(s: &State, op: &str, p: &Value) -> Result<PathBuf, Value> {
    let raw = p["cwd"].as_str().unwrap_or("");
    let path = paths::normalize(&paths::expand_home(raw.trim()));
    let what = if op.ends_with("getDiffPreview") {
        "Review diff preview"
    } else if op.ends_with("getBranchCommits") {
        "Review branch commits"
    } else {
        "Review diff file contents"
    };
    if raw.trim().is_empty() || !path.is_absolute() || !paths::is_known_workspace(s, &path) {
        return Err(detection_error(
            op,
            raw,
            &format!("{what} cwd must stay within the configured workspace root."),
        ));
    }
    Ok(path)
}

struct Stat {
    path: String,
    previous: Option<String>,
    add: u64,
    del: u64,
}
fn parse_numstat(raw: &[u8]) -> Vec<Stat> {
    let fields: Vec<String> = raw
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let mut out = Vec::new();
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
            if let (Some(add), Some(del)) = (num(a), num(d)) {
                if p.is_empty() {
                    let (prev, path) = (
                        fields.get(i + 1).cloned().unwrap_or_default(),
                        fields.get(i + 2).cloned().unwrap_or_default(),
                    );
                    i += 2;
                    out.push(Stat {
                        path,
                        previous: Some(prev),
                        add,
                        del,
                    });
                } else {
                    out.push(Stat {
                        path: p.to_owned(),
                        previous: None,
                        add,
                        del,
                    });
                }
            }
        }
        i += 1;
    }
    out
}
fn stats_json(s: &[Stat]) -> Value {
    json!(s
        .iter()
        .map(
            |f| json!({"path":f.path,"previousPath":f.previous,"additions":f.add,"deletions":f.del})
        )
        .collect::<Vec<_>>())
}

/// A copy of the real index with every untracked file marked intent-to-add, so `git diff` shows them.
struct TempIndex(Option<PathBuf>);
impl Drop for TempIndex {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}
async fn untracked_index(
    s: &State,
    root: &Path,
    pathspecs: &[String],
) -> Result<Option<TempIndex>, Value> {
    let op = "review.untrackedIndex";
    let mut args: Vec<&str> = vec!["ls-files", "-z", "--others", "--exclude-standard", "--"];
    args.extend(pathspecs.iter().map(String::as_str));
    let listed = out(
        op,
        root,
        &args,
        Opts {
            cap: 8 * 1024 * 1024,
            ..Opts::READ
        },
    )
    .await?;
    // A staged deletion can still exist on disk. It is intentionally untracked,
    // not a new file to re-add. Disable rename detection when collecting these
    // paths, otherwise a staged copy can hide the deletion as a rename.
    let mut deleted_args = vec![
        "diff",
        "--cached",
        "--no-renames",
        "--name-only",
        "--diff-filter=D",
        "-z",
        "--",
    ];
    deleted_args.extend(pathspecs.iter().map(String::as_str));
    let deleted = out(
        op,
        root,
        &deleted_args,
        Opts {
            cap: 8 * 1024 * 1024,
            ..Opts::READ
        },
    )
    .await?;
    if !deleted.ok() {
        return Err(git_fail(op, root, &deleted_args, &deleted));
    }
    let removed: std::collections::BTreeSet<_> = deleted
        .stdout
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let files: Vec<String> = listed
        .stdout
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .filter(|f| !removed.contains(f))
        .collect();
    if files.len() > UNTRACKED_MAX {
        return Ok(None);
    }
    let dir = s.dir.join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| simple_fail(op, root, &e.to_string(), None))?;
    let tmp = dir.join(format!("index-{}", random_id().unwrap_or_default()));
    let guard = TempIndex(Some(tmp.clone()));
    let real = out(
        op,
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        Opts::READ,
    )
    .await?;
    let real = PathBuf::from(real.text().trim());
    if real.is_file() {
        std::fs::copy(&real, &tmp).map_err(|e| simple_fail(op, root, &e.to_string(), None))?;
    }
    for chunk in files.chunks(200) {
        let specs: Vec<String> = chunk.iter().map(|f| format!(":(literal){f}")).collect();
        let mut a: Vec<&str> = vec!["add", "-N", "--"];
        a.extend(specs.iter().map(String::as_str));
        let o = proc::git_env(root, &a, Opts::WRITE, &[("GIT_INDEX_FILE", tmp.as_path())])
            .await
            .map_err(|e| run_err(op, root, e))?;
        if !o.ok() {
            return Err(git_fail(op, root, &a, &o));
        }
    }
    Ok(Some(guard))
}

struct Diff {
    patch: String,
    truncated: bool,
    files: Option<Vec<Stat>>,
}
async fn tracked_diff(
    root: &Path,
    base: &[&str],
    reference: Option<&str>,
    specs: &[String],
    env: &[(&str, &Path)],
    limit: usize,
) -> Result<Diff, Value> {
    let op = "review.getDiffPreview";
    let Some(reference) = reference else {
        return Ok(Diff {
            patch: String::new(),
            truncated: false,
            files: Some(vec![]),
        });
    };
    let run = |extra: &[&str], reference: &str, cap: usize, truncate: bool| {
        let mut args: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        args.push(reference.to_owned());
        args.push("--".into());
        args.extend(specs.iter().cloned());
        let env: Vec<(String, PathBuf)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_path_buf()))
            .collect();
        let root = root.to_path_buf();
        async move {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let env_refs: Vec<(&str, &Path)> =
                env.iter().map(|(k, v)| (k.as_str(), v.as_path())).collect();
            proc::git_env(
                &root,
                &refs,
                Opts {
                    timeout: proc::READ_TIMEOUT,
                    cap,
                    allow_truncate: truncate,
                },
                &env_refs,
            )
            .await
            .map_err(|e| run_err(op, &root, e))
            .map(|o| (o, refs.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
        }
    };
    // An unborn HEAD has nothing to compare with: use the empty tree.
    let mut reference = reference.to_owned();
    let (mut stat, mut args) = run(&["--numstat", "-z"], &reference, METADATA_MAX, false).await?;
    if !stat.ok()
        && reference == "HEAD"
        && (stat.stderr.contains("unknown revision")
            || stat.stderr.contains("bad revision")
            || stat.stderr.contains("ambiguous argument"))
    {
        reference = EMPTY_TREE.to_owned();
        (stat, args) = run(&["--numstat", "-z"], &reference, METADATA_MAX, false).await?;
    }
    if !stat.ok() {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        return Err(git_fail(op, root, &a, &stat));
    }
    let files = parse_numstat(&stat.stdout);
    if files.is_empty() {
        return Ok(Diff {
            patch: String::new(),
            truncated: false,
            files: Some(files),
        });
    }
    let (patch, _) = run(&["--patch"], &reference, limit, true).await?;
    if !patch.ok() {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        return Err(git_fail(op, root, &a, &patch));
    }
    let mut text = String::from_utf8_lossy(&patch.stdout).into_owned();
    if patch.truncated {
        text.push_str("\n\n[truncated]");
    }
    Ok(Diff {
        patch: text,
        truncated: patch.truncated,
        files: Some(files),
    })
}

/// (display base ref, merge-base revision). A branch with nothing to compare against reviews HEAD.
async fn merge_base(root: &Path, requested: Option<&str>) -> (Option<String>, String) {
    let current = proc::git(root, &["branch", "--show-current"], Opts::READ)
        .await
        .ok()
        .map(|o| o.text().trim().to_owned())
        .unwrap_or_default();
    let candidates: Vec<String> = match requested {
        Some(r) if crate::vcs::valid_ref_name(r) => vec![r.to_owned()],
        Some(_) => vec![],
        None => match default_branch(root).await {
            Some(d) if d != current => vec![format!("origin/{d}"), d],
            _ => vec![],
        },
    };
    for c in candidates {
        let probe = format!("{c}^{{commit}}");
        if !proc::git(
            root,
            &["rev-parse", "--verify", "--quiet", &probe],
            Opts::READ,
        )
        .await
        .is_ok_and(|o| o.ok())
        {
            continue;
        }
        if let Ok(o) = proc::git(root, &["merge-base", &c, "HEAD"], Opts::READ).await {
            if o.ok() && !o.text().trim().is_empty() {
                return (Some(c), o.text().trim().to_owned());
            }
        }
    }
    (None, "HEAD".into())
}

const BRANCH_COMMIT_LIMIT: usize = 100;
const BRANCH_SUBJECT_LIMIT: usize = 300;
const BRANCH_COMMIT_MAX_OUTPUT: usize = 128 * 1024;

/// `review.getBranchCommits` (nightly 4093): commits on HEAD since its merge base, newest first.
pub async fn branch_commits(s: &State, p: Value) -> Reply {
    let op = "review.getBranchCommits";
    let empty = json!({"commits":[],"truncated":false});
    let cwd = bound(s, op, &p)?;
    let Some(root) = toplevel(&cwd).await else {
        return Ok(empty);
    };
    let fail = |detail: &str| {
        let mut e = simple_fail("GitVcsDriver.reviewCommit", &root, detail, None);
        e["command"] = json!("git cat-file");
        e
    };
    let head = proc::git(
        &root,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        Opts::READ,
    )
    .await
    .map_err(|e| run_err(op, &root, e))?;
    if !head.ok() {
        if head.code == Some(1) && head.stderr.trim().is_empty() {
            return Ok(empty);
        }
        return Err(fail("Could not resolve the current branch commit."));
    }
    let (_, base) = merge_base(&root, p["baseRef"].as_str()).await;
    let range = format!("{base}..HEAD");
    let max = format!("--max-count={}", BRANCH_COMMIT_LIMIT + 1);
    let out = proc::git(
        &root,
        &[
            "log",
            "-z",
            "--no-notes",
            "--no-show-signature",
            "--format=%H%x00%s%x00%cI",
            &max,
            &range,
        ],
        Opts {
            cap: BRANCH_COMMIT_MAX_OUTPUT,
            allow_truncate: true,
            ..Opts::READ
        },
    )
    .await
    .map_err(|e| run_err(op, &root, e))?;
    if !out.ok() && !out.truncated {
        return Err(fail("Could not read branch commit metadata."));
    }
    parse_branch_commits(&out.stdout, out.truncated).map_err(fail)
}

fn parse_branch_commits(raw: &[u8], truncated: bool) -> Result<Value, &'static str> {
    if !truncated && !raw.is_empty() && !raw.ends_with(b"\0") {
        return Err("Could not read branch commit metadata.");
    }
    let text = String::from_utf8_lossy(raw);
    let fields: Vec<&str> = text.split('\0').collect();
    let complete = fields.len() - 1;
    let mut commits = Vec::new();
    let mut i = 0;
    while i + 2 < complete && commits.len() <= BRANCH_COMMIT_LIMIT {
        let (sha, subject, at) = (fields[i], fields[i + 1], fields[i + 2]);
        let hex = |n| sha.len() == n && sha.bytes().all(|b| b.is_ascii_hexdigit());
        if at.is_empty() || !(hex(40) || hex(64)) {
            return Err("Could not read branch commit metadata.");
        }
        let subject: String = subject.chars().take(BRANCH_SUBJECT_LIMIT).collect();
        commits.push(json!({"sha":sha,"subject":subject,"committedAt":at}));
        i += 3;
    }
    if truncated && commits.is_empty() {
        return Err("Branch commit metadata exceeds the response limit.");
    }
    let more = commits.len() > BRANCH_COMMIT_LIMIT;
    commits.truncate(BRANCH_COMMIT_LIMIT);
    Ok(json!({"commits":commits,"truncated":truncated || more}))
}

pub async fn diff_preview(s: &State, p: Value) -> Reply {
    let op = "review.getDiffPreview";
    let cwd = bound(s, op, &p)?;
    let now = iso(now_ms());
    let Some(root) = toplevel(&cwd).await else {
        return Ok(json!({"cwd":p["cwd"],"generatedAt":now,"sources":[]}));
    };
    let file = p["file"].as_object().filter(|f| {
        f.get("path")
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
    });
    let specs: Vec<String> = file
        .map(|f| {
            let mut v = vec![format!(
                ":(top,literal){}",
                f["path"].as_str().unwrap_or("")
            )];
            if let Some(prev) = f.get("previousPath").and_then(Value::as_str) {
                v.push(format!(":(top,literal){prev}"));
            }
            v
        })
        .unwrap_or_default();
    let kind = file.and_then(|f| f["sourceKind"].as_str());
    let limit = if file.is_some() {
        FILE_PATCH_MAX
    } else {
        PATCH_MAX
    };
    let current = proc::git(&root, &["branch", "--show-current"], Opts::READ)
        .await
        .ok()
        .map(|o| o.text().trim().to_owned())
        .filter(|b| !b.is_empty());
    let (base_ref, merge) = if kind == Some("working-tree") {
        (p["baseRef"].as_str().map(str::to_owned), None)
    } else {
        let (b, m) = merge_base(&root, p["baseRef"].as_str()).await;
        (b, Some(m))
    };
    let dirty_ref = if kind == Some("branch-range") {
        None
    } else {
        Some("HEAD".to_owned())
    };
    let mut base: Vec<&str> = vec![
        "diff",
        "--find-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--minimal",
    ];
    if p["ignoreWhitespace"] == true {
        base.push("--ignore-all-space");
    }
    let untracked = untracked_index(s, &root, &specs).await?;
    let (env, complete): (Vec<(&str, &Path)>, bool) =
        match untracked.as_ref().and_then(|t| t.0.as_deref()) {
            Some(path) => (vec![("GIT_INDEX_FILE", path)], true),
            None => (vec![], false),
        };
    let dirty = tracked_diff(&root, &base, dirty_ref.as_deref(), &specs, &env, limit).await?;
    let against = if merge.as_deref() == dirty_ref.as_deref() {
        None
    } else {
        merge.as_deref()
    };
    let range = match (&merge, against) {
        (Some(_), Some(m)) => tracked_diff(&root, &base, Some(m), &specs, &env, limit).await?,
        (Some(_), None) => Diff {
            patch: dirty.patch.clone(),
            truncated: dirty.truncated,
            files: dirty.files.as_ref().map(|f| {
                f.iter()
                    .map(|s| Stat {
                        path: s.path.clone(),
                        previous: s.previous.clone(),
                        add: s.add,
                        del: s.del,
                    })
                    .collect()
            }),
        },
        (None, _) => Diff {
            patch: String::new(),
            truncated: false,
            files: Some(vec![]),
        },
    };
    let source = |id: &str, title: String, base_ref: Option<&str>, head: Option<&str>, d: &Diff| {
        let files = if complete { d.files.as_ref() } else { None };
        let hash = Sha256::digest(
            json!([d.patch, files.map(|f| stats_json(f))])
                .to_string()
                .as_bytes(),
        );
        let mut v = json!({"id":id,"kind":id,"title":title,"baseRef":base_ref,"headRef":head,"diff":d.patch,
            "diffHash":hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),"truncated":d.truncated || !complete});
        if let Some(f) = files {
            v["files"] = stats_json(f);
        }
        v
    };
    let title = base_ref
        .as_ref()
        .map(|b| format!("Changes vs {b}"))
        .unwrap_or_else(|| "Changes".into());
    Ok(json!({"cwd":p["cwd"],"generatedAt":now,"sources":[
        source("working-tree", "Uncommitted".into(), Some("HEAD"), None, &dirty),
        source("branch-range", title, base_ref.as_deref(), Some(current.as_deref().unwrap_or("HEAD")), &range)]}))
}

pub async fn file_contents(s: &State, p: Value) -> Reply {
    let op = "review.getDiffFileContents";
    let cwd = bound(s, op, &p)?;
    let fail = |detail: String| simple_fail(op, &cwd, &detail, None);
    let Some(root) = toplevel(&cwd).await else {
        return Err(fail("Could not resolve the Git repository root.".into()));
    };
    let (old_path, new_path) = (
        p["oldPath"].as_str().unwrap_or(""),
        p["newPath"].as_str().unwrap_or(""),
    );
    let change = p["changeType"].as_str().unwrap_or("change");
    let revision = if p["sourceKind"] == "working-tree" {
        p["baseRef"]
            .as_str()
            .filter(|r| crate::vcs::valid_ref_name(r))
            .unwrap_or("HEAD")
            .to_owned()
    } else {
        merge_base(&root, p["baseRef"].as_str()).await.1
    };
    let old = if change == "new" {
        String::new()
    } else {
        let spec = format!("{revision}:{old_path}");
        let o = out(
            op,
            &root,
            &["show", "--end-of-options", &spec],
            Opts {
                cap: FILE_MAX,
                allow_truncate: false,
                ..Opts::READ
            },
        )
        .await?;
        if !o.ok() {
            return Err(fail(format!("Could not read '{old_path}' at {revision}.")));
        }
        if o.stdout.contains(&0) {
            return Err(fail(format!("Cannot expand binary file '{old_path}'.")));
        }
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let new = if change == "deleted" {
        String::new()
    } else {
        let (root2, rel) = (root.clone(), new_path.to_owned());
        crate::files::blocking(move || read_working_file(&root2, &rel))
            .await?
            .map_err(fail)?
    };
    Ok(json!({"oldContents":old,"newContents":new}))
}

fn read_working_file(root: &Path, rel: &str) -> Result<String, String> {
    use std::io::Read;
    let Some((abs, _)) = paths::resolve_relative(root, rel) else {
        return Err(format!(
            "Diff file '{rel}' resolves outside the review workspace."
        ));
    };
    let real_root =
        std::fs::canonicalize(root).map_err(|_| format!("Could not resolve diff file '{rel}'."))?;
    let real =
        std::fs::canonicalize(&abs).map_err(|_| format!("Could not resolve diff file '{rel}'."))?;
    if !paths::within(&real_root, &real) {
        return Err(format!(
            "Diff file '{rel}' resolves outside the review workspace."
        ));
    }
    let meta =
        std::fs::metadata(&real).map_err(|_| format!("Could not inspect diff file '{rel}'."))?;
    if !meta.is_file() {
        return Err(format!("Diff path '{rel}' is not a file."));
    }
    if meta.len() > FILE_MAX as u64 {
        return Err(format!(
            "Diff file '{rel}' exceeds the 1 MB expansion limit."
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&real)
        .and_then(|f| f.take(FILE_MAX as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| format!("Could not read diff file '{rel}'."))?;
    if bytes.contains(&0) {
        return Err(format!("Cannot expand binary file '{rel}'."));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
