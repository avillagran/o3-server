//! `pullRequests.*` and `git.preparePullRequestThread` through the GitHub CLI.
//!
//! Every call is one or two `gh` invocations with an argument vector (never a shell), run on the
//! project's workspace root. Nothing is cached beyond a short viewer-login lookup. Hosts that are
//! not GitHub answer `PullRequestUnavailableError(provider-unsupported)`; a missing or signed-out
//! `gh` answers `cli-missing` / `cli-unauthenticated`. No data is invented: a field `gh` cannot
//! report is omitted or reported as the schema's "unknown" value.
use crate::{
    events,
    p2::{invalid, str_field, Reply, Ttl},
    proc::{self, Opts},
    store::State,
};
use serde_json::{json, Map, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "pullRequests.reviewGuide"
            | "pullRequests.generateReviewGuide"
            | "pullRequests.list"
            | "pullRequests.detail"
            | "pullRequests.checks"
            | "pullRequests.activity"
            | "pullRequests.routing"
            | "pullRequests.routingIdentity"
            | "pullRequests.filterOptions"
            | "pullRequests.reviewerCandidates"
            | "pullRequests.labelCandidates"
            | "pullRequests.changeBreakdown"
            | "pullRequests.comment"
            | "pullRequests.submitReview"
            | "pullRequests.runAction"
            | "pullRequests.requestReviewers"
            | "pullRequests.setLabels"
            | "git.preparePullRequestThread"
    )
}

pub async fn call(s: &State, tag: &str, p: &Value) -> Reply {
    match tag {
        "pullRequests.reviewGuide" | "pullRequests.generateReviewGuide" => {
            review_guide(s, tag, p).await
        }
        "pullRequests.list" => list(s, p).await,
        "pullRequests.routingIdentity" => routing_identity(p).await,
        "pullRequests.filterOptions" => filter_options(s, p).await,
        "git.preparePullRequestThread" => prepare_thread(s, p).await,
        _ => {
            let pr = Pr::resolve(s, p).await?;
            match tag {
                "pullRequests.detail" => detail(&pr).await,
                "pullRequests.checks" => checks(&pr).await,
                "pullRequests.activity" => activity(&pr).await,
                "pullRequests.routing" => routing(&pr).await,
                "pullRequests.reviewerCandidates" => reviewer_candidates(&pr).await,
                "pullRequests.labelCandidates" => label_candidates(&pr).await,
                "pullRequests.changeBreakdown" => change_breakdown(&pr).await,
                "pullRequests.comment" => comment(&pr, p).await,
                "pullRequests.submitReview" => submit_review(&pr, p).await,
                "pullRequests.runAction" => run_action(s, &pr, p).await,
                "pullRequests.requestReviewers" => request_reviewers(&pr, p).await,
                _ => set_labels(&pr, p).await,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Errors

fn unavailable(reason: &str, requirement: Option<&str>) -> Value {
    let mut v = json!({"_tag":"PullRequestUnavailableError","reason":reason,"provider":"github"});
    if let Some(r) = requirement {
        v["requirement"] = json!(r);
    }
    v
}
fn unsupported() -> Value {
    json!({"_tag":"PullRequestUnavailableError","reason":"provider-unsupported"})
}
fn operation(op: &str, detail: &str, not_found: bool) -> Value {
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        "the GitHub CLI failed"
    } else {
        detail
    };
    let mut v = json!({"_tag":"PullRequestOperationError","operation":op,"detail":detail.chars().take(2000).collect::<String>()});
    if not_found {
        v["reason"] = json!("not-found");
    }
    v
}

// ---------------------------------------------------------------------------------------------
// gh

struct Gh {
    cwd: PathBuf,
    host: String,
}

impl Gh {
    async fn run(&self, op: &str, args: &[&str], stdin: Option<&str>) -> Result<String, Value> {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.args(args)
            .current_dir(&self.cwd)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1")
            .env("GH_PAGER", "cat")
            .env("LC_ALL", "C");
        if self.host != "github.com" {
            cmd.env("GH_HOST", &self.host);
        }
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                unavailable(
                    "cli-missing",
                    Some("Install the GitHub CLI (gh) to browse GitHub pull requests."),
                )
            } else {
                operation(op, &e.to_string(), false)
            }
        })?;
        if let (Some(text), Some(mut input)) = (stdin, child.stdin.take()) {
            let bytes = text.as_bytes().to_vec();
            // The child may exit before reading everything; its exit status tells the story.
            let _ = input.write_all(&bytes).await;
            drop(input);
        }
        let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .map_err(|_| operation(op, "the GitHub CLI timed out", false))?
            .map_err(|e| operation(op, &e.to_string(), false))?;
        if out.stdout.len() > 8 * 1024 * 1024 {
            return Err(operation(
                op,
                "the GitHub CLI answered too much data",
                false,
            ));
        }
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        let low = err.to_lowercase();
        if low.contains("gh auth login")
            || low.contains("not logged in")
            || low.contains("http 401")
            || low.contains("bad credentials")
        {
            return Err(unavailable(
                "cli-unauthenticated",
                Some("Run `gh auth login` on the server to sign in to this host."),
            ));
        }
        let missing = low.contains("could not resolve to a pullrequest")
            || low.contains("no pull requests found")
            || low.contains("http 404")
            || low.contains("not found");
        Err(operation(op, &err, missing))
    }
    async fn json(&self, op: &str, args: &[&str], stdin: Option<&str>) -> Result<Value, Value> {
        let text = self.run(op, args, stdin).await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| operation(op, &format!("unreadable answer: {e}"), false))
    }
}

fn viewers() -> &'static Mutex<Ttl<String>> {
    static CACHE: OnceLock<Mutex<Ttl<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Ttl::new(Duration::from_secs(300), 16)))
}

async fn viewer(gh: &Gh) -> Result<String, Value> {
    let key = format!("{}|{}", gh.cwd.display(), gh.host);
    if let Some(v) = viewers().lock().unwrap().get(&key) {
        return Ok(v);
    }
    let out = gh
        .run("viewer", &["api", "user", "--jq", ".login"], None)
        .await?;
    let login = out.trim().to_owned();
    if login.is_empty() {
        return Err(unavailable("cli-unauthenticated", None));
    }
    viewers().lock().unwrap().put(key, login.clone());
    Ok(login)
}

// ---------------------------------------------------------------------------------------------
// Projects and repositories

#[derive(Clone)]
struct Project {
    id: String,
    title: String,
    root: PathBuf,
}

fn project(s: &State, id: &str) -> Result<Project, Value> {
    let v = events::get(&s.db.lock().unwrap(), "project", id)
        .map_err(|_| operation("project", &format!("unknown project {id}"), true))?;
    if !v["deletedAt"].is_null() {
        return Err(operation("project", &format!("unknown project {id}"), true));
    }
    Ok(Project {
        id: id.into(),
        title: v["title"].as_str().unwrap_or(id).to_owned(),
        root: PathBuf::from(v["workspaceRoot"].as_str().unwrap_or("")),
    })
}

fn live_projects(s: &State) -> Vec<Project> {
    let db = s.db.lock().unwrap();
    let Ok(mut q) = db.prepare("SELECT body FROM records WHERE kind='project' ORDER BY rowid")
    else {
        return Vec::new();
    };
    let Ok(rows) = q.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|b| serde_json::from_str::<Value>(&b).ok())
        .filter(|v| v["deletedAt"].is_null())
        .filter_map(|v| {
            Some(Project {
                id: v["id"].as_str()?.to_owned(),
                title: v["title"].as_str().unwrap_or("").to_owned(),
                root: PathBuf::from(v["workspaceRoot"].as_str()?),
            })
        })
        .collect()
}

/// `(host, owner/name)` of the `origin` remote.
async fn remote(root: &Path) -> Option<(String, String)> {
    // The configured URL, not the `insteadOf`-rewritten one: the host the user named is the account host.
    let out = proc::git(root, &["config", "--get", "remote.origin.url"], Opts::READ)
        .await
        .ok()?;
    if !out.ok() {
        return None;
    }
    parse_remote(out.text().trim())
}

pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_owned(), path.to_owned())
    } else {
        let rest = url.rsplit_once('@').map(|(_, r)| r).unwrap_or(url);
        let (host, path) = rest.split_once(':')?;
        (host.to_owned(), path.to_owned())
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if host.is_empty() || owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((host, format!("{owner}/{name}")))
}

fn is_github(host: &str) -> bool {
    let h = host.to_lowercase();
    !(h.contains("gitlab") || h.contains("bitbucket") || h.contains("azure") || h.contains("gitea"))
}

struct Pr {
    project: Project,
    gh: Gh,
    repo: String,
    number: u64,
}

impl Pr {
    async fn resolve(s: &State, p: &Value) -> Result<Pr, Value> {
        let project = project(s, str_field(p, "projectId")?)?;
        let repo = str_field(p, "repository")?.to_owned();
        let number = p["number"]
            .as_u64()
            .filter(|n| *n >= 1)
            .ok_or_else(|| invalid("number is required"))?;
        let host = match p["host"].as_str().filter(|h| !h.trim().is_empty()) {
            Some(h) => h.to_owned(),
            None => remote(&project.root)
                .await
                .map(|(h, _)| h)
                .unwrap_or_else(|| "github.com".into()),
        };
        if !is_github(&host) || repo.starts_with('-') || repo.split('/').count() != 2 {
            return Err(if repo.split('/').count() != 2 {
                invalid("repository must be owner/name")
            } else {
                unsupported()
            });
        }
        let gh = Gh {
            cwd: project.root.clone(),
            host,
        };
        if let Some(expected) = p["expectedAccountId"].as_str() {
            let id = account_id(&gh.host, &viewer(&gh).await?);
            if id != expected {
                return Err(operation(
                    "account",
                    "The signed-in account differs from the expected one.",
                    false,
                ));
            }
        }
        Ok(Pr {
            project,
            gh,
            repo,
            number,
        })
    }
    fn n(&self) -> String {
        self.number.to_string()
    }
}

fn account_id(host: &str, login: &str) -> String {
    format!("github:{host}:{login}")
}

// ---------------------------------------------------------------------------------------------
// Mapping gh JSON to T3 shapes

fn actor(host: &str, a: &Value) -> Value {
    match a["login"].as_str().filter(|l| !l.is_empty()) {
        None => Value::Null,
        Some(login) => {
            let bot = a["is_bot"] == true || a["type"] == "Bot" || login.starts_with("app/");
            json!({"isBot":bot,"login":login,"name":a["name"].as_str().filter(|n|!n.is_empty()),"avatarUrl":format!("https://{host}/{}.png",login.trim_start_matches("app/"))})
        }
    }
}
fn label(l: &Value) -> Option<Value> {
    let name = l["name"].as_str().filter(|n| !n.is_empty())?;
    Some(json!({"name":name,"color":l["color"].as_str().filter(|c|!c.is_empty())}))
}
fn labels(v: &Value) -> Vec<Value> {
    v.as_array()
        .map(|a| a.iter().filter_map(label).collect())
        .unwrap_or_default()
}
fn lower(v: &Value) -> String {
    v.as_str().unwrap_or("").to_lowercase()
}
fn state(v: &Value) -> &'static str {
    match lower(v).as_str() {
        "merged" => "merged",
        "closed" => "closed",
        _ => "open",
    }
}
fn mergeability(v: &Value) -> &'static str {
    match lower(v).as_str() {
        "mergeable" => "mergeable",
        "conflicting" => "conflicting",
        _ => "unknown",
    }
}
fn review_decision(v: &Value) -> Option<&'static str> {
    match lower(v).as_str() {
        "approved" => Some("approved"),
        "changes_requested" => Some("changes-requested"),
        "review_required" => Some("review-required"),
        _ => None,
    }
}

/// One entry of `statusCheckRollup` as a T3 check.
fn check(c: &Value) -> Option<Value> {
    let name = c["name"]
        .as_str()
        .or_else(|| c["context"].as_str())
        .filter(|n| !n.trim().is_empty())?;
    let status = if c["__typename"] == "StatusContext" || c.get("state").is_some() {
        match lower(&c["state"]).as_str() {
            "success" => "success",
            "failure" | "error" => "failure",
            _ => "pending",
        }
    } else if lower(&c["status"]) != "completed" {
        "pending"
    } else {
        match lower(&c["conclusion"]).as_str() {
            "success" => "success",
            "failure" | "timed_out" | "startup_failure" => "failure",
            "cancelled" => "cancelled",
            "skipped" => "skipped",
            "neutral" | "stale" => "neutral",
            "action_required" => "action-required",
            _ => "pending",
        }
    };
    Some(json!({
        "name":name,"status":status,
        "description":c["description"].as_str().filter(|d|!d.is_empty()),
        "url":c["detailsUrl"].as_str().or_else(||c["targetUrl"].as_str()).filter(|u|!u.is_empty()),
    }))
}
fn checks_of(rollup: &Value) -> Vec<Value> {
    rollup
        .as_array()
        .map(|a| a.iter().filter_map(check).collect())
        .unwrap_or_default()
}
fn checks_state(checks: &[Value]) -> Option<&'static str> {
    if checks.is_empty() {
        return None;
    }
    let any = |f: fn(&str) -> bool| checks.iter().any(|c| f(c["status"].as_str().unwrap_or("")));
    Some(
        if any(|s| matches!(s, "failure" | "cancelled" | "action-required")) {
            "failing"
        } else if any(|s| s == "pending") {
            "pending"
        } else {
            "passing"
        },
    )
}

const DETAIL_FIELDS: &str = "number,title,body,url,author,state,isDraft,mergeable,mergeStateStatus,additions,deletions,changedFiles,headRefName,headRefOid,headRepository,headRepositoryOwner,baseRefName,createdAt,updatedAt,mergedAt,closedAt,reviewRequests,labels,statusCheckRollup,autoMergeRequest,reviewDecision";

async fn view(pr: &Pr, fields: &str) -> Result<Value, Value> {
    pr.gh
        .json(
            "view",
            &["pr", "view", &pr.n(), "--repo", &pr.repo, "--json", fields],
            None,
        )
        .await
}

fn capabilities() -> Value {
    json!({
        "diff":false,"comment":true,
        "actions":["merge","ready","draft","close","reopen","delete-branch","update-branch","enable-auto-merge","disable-auto-merge"],
        "mergeMethods":["merge","squash","rebase"],
        "deleteBranch":true,"updateMethods":["merge","rebase"],"search":true,
        "review":{"inlineComment":true,"rangeComment":true,"reply":false,"resolve":false,"verdicts":["comment","approve","request-changes"]},
        "reviewers":{"request":true,"listCandidates":true},
        "edit":{"changeRequest":false,"comment":false},
        "labels":true
    })
}
fn permissions() -> Value {
    json!({
        "actions":["merge","ready","draft","close","reopen","delete-branch","update-branch","enable-auto-merge","disable-auto-merge"],
        "comment":true,"resolve":false,"verdicts":["comment","approve","request-changes"],
        "requestReviewers":true,"updateMethods":["merge","rebase"],"labels":true
    })
}

async fn detail(pr: &Pr) -> Reply {
    let repo_args = [
        "repo",
        "view",
        pr.repo.as_str(),
        "--json",
        "mergeCommitAllowed,squashMergeAllowed,rebaseMergeAllowed",
    ];
    let (v, repo) = tokio::join!(
        view(pr, DETAIL_FIELDS),
        pr.gh.json("repo", &repo_args, None,)
    );
    let (v, repo) = (v?, repo?);
    let host = &pr.gh.host;
    let checks = checks_of(&v["statusCheckRollup"]);
    let head_repo = match (
        v["headRepositoryOwner"]["login"].as_str(),
        v["headRepository"]["name"].as_str(),
    ) {
        (Some(o), Some(n)) => json!(format!("{o}/{n}")),
        _ => Value::Null,
    };
    let viewer = viewer(&pr.gh).await.ok();
    let mut out = json!({
        "provider":"github","capabilities":capabilities(),"viewerPermissions":permissions(),
        "projectId":pr.project.id,"projectTitle":pr.project.title,"workspaceRoot":pr.project.root,
        "repository":pr.repo,"number":pr.number,
        "title":v["title"],"body":v["body"].as_str().unwrap_or(""),"url":v["url"],
        "author":actor(host,&v["author"]),"state":state(&v["state"]),"isDraft":v["isDraft"]==true,
        "mergeability":mergeability(&v["mergeable"]),
        "additions":v["additions"].as_u64().unwrap_or(0),"deletions":v["deletions"].as_u64().unwrap_or(0),
        "changedFiles":v["changedFiles"].as_u64().unwrap_or(0),
        "headBranch":v["headRefName"],"headRepositoryNameWithOwner":head_repo,
        "baseBranch":v["baseRefName"],
        "createdAt":v["createdAt"],"updatedAt":v["updatedAt"],
        "observedAt":crate::store::now_ms(),
        "mergedAt":v["mergedAt"].as_str().filter(|d|!d.starts_with("0001")),
        "closedAt":v["closedAt"].as_str().filter(|d|!d.starts_with("0001")),
        "reviewers":v["reviewRequests"].as_array().map(|a|a.iter().filter(|r|r["__typename"]!="Team").map(|r|actor(host,r)).filter(|a|!a.is_null()).collect::<Vec<_>>()).unwrap_or_default(),
        "labels":labels(&v["labels"]),"checks":checks,
        "mergeCapabilities":{"merge":repo["mergeCommitAllowed"]==true,"squash":repo["squashMergeAllowed"]==true,"rebase":repo["rebaseMergeAllowed"]==true},
        "baseComparison":match lower(&v["mergeStateStatus"]).as_str(){"behind"=>"behind","unknown"|""=>"unknown",_=>"up-to-date"},
        "autoMergeEnabled":v["autoMergeRequest"].is_object(),
    });
    if let Some(sha) = v["headRefOid"].as_str().filter(|s| !s.is_empty()) {
        out["headSha"] = json!(sha);
    }
    if let Some(viewer) = viewer {
        out["viewer"] = json!(viewer);
    }
    if let Some(m) = v["autoMergeRequest"]["mergeMethod"].as_str() {
        out["autoMergeMethod"] = json!(m.to_lowercase());
    }
    Ok(out)
}

async fn checks(pr: &Pr) -> Reply {
    let v = view(pr, "state,statusCheckRollup").await?;
    Ok(
        json!({"observedAt":crate::store::now_ms(),"state":state(&v["state"]),"checks":checks_of(&v["statusCheckRollup"])}),
    )
}

async fn routing_identity(p: &Value) -> Reply {
    let host = str_field(p, "host")?;
    if !is_github(host) {
        return Err(unsupported());
    }
    let gh = Gh {
        cwd: std::env::current_dir().unwrap_or_else(|_| "/".into()),
        host: host.into(),
    };
    let login = viewer(&gh).await?;
    Ok(json!({"accountId":account_id(host,&login),"host":host,"provider":"github","viewer":login}))
}

async fn routing(pr: &Pr) -> Reply {
    let login = viewer(&pr.gh).await?;
    Ok(
        json!({"accountId":account_id(&pr.gh.host,&login),"host":pr.gh.host,"provider":"github","viewer":login,
        "projectTitle":pr.project.title,"workspaceRoot":pr.project.root}),
    )
}

// ---- activity ------------------------------------------------------------------------------

const GRAPHQL_THREADS: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:50){totalCount nodes{id isResolved isOutdated path line originalLine diffSide viewerCanResolve comments(first:20){totalCount nodes{id body createdAt lastEditedAt url viewerCanUpdate author{login avatarUrl}}}}}}}}";

fn gql_actor(a: &Value) -> Value {
    match a["login"].as_str() {
        Some(l) => json!({"login":l,"name":null,"avatarUrl":a["avatarUrl"].as_str()}),
        None => Value::Null,
    }
}

async fn activity(pr: &Pr) -> Reply {
    let host = &pr.gh.host;
    let (owner, name) = pr.repo.split_once('/').unwrap_or((&pr.repo, ""));
    let number = pr.n();
    let threads_args = [
        "api",
        "graphql",
        "-f",
        &format!("query={GRAPHQL_THREADS}"),
        "-F",
        &format!("owner={owner}"),
        "-F",
        &format!("name={name}"),
        "-F",
        &format!("number={number}"),
    ];
    let comments_path = format!(
        "repos/{}/pulls/{}/comments?per_page=100",
        pr.repo, pr.number
    );
    let comments_args = ["api", comments_path.as_str()];
    let (v, review_comments, threads) = tokio::join!(
        view(pr, "author,comments,reviews,commits,reviewRequests"),
        pr.gh.json("review-comments", &comments_args, None),
        pr.gh.json("review-threads", &threads_args, None)
    );
    let v = v?;
    let mut comments: Vec<Value> = Vec::new();
    for c in v["comments"].as_array().into_iter().flatten() {
        comments.push(json!({
            "id":c["id"],"kind":"issue-comment","author":actor(host,&c["author"]),"body":c["body"].as_str().unwrap_or(""),
            "createdAt":c["createdAt"],"editedAt":c["updatedAt"].as_str().filter(|u|*u!=c["createdAt"].as_str().unwrap_or("")),
            "url":c["url"].as_str(),"path":null,"reviewState":null,
        }));
    }
    for r in v["reviews"].as_array().into_iter().flatten() {
        let body = r["body"].as_str().unwrap_or("");
        let state = r["state"].as_str().unwrap_or("");
        if body.is_empty() && state == "COMMENTED" {
            continue;
        }
        comments.push(json!({
            "id":r["id"],"kind":"review","author":actor(host,&r["author"]),"body":body,
            "createdAt":r["submittedAt"].as_str().unwrap_or(""),"url":null,"path":null,"reviewState":state.to_lowercase(),
        }));
    }
    // Review comments are optional garnish: a failure here must not hide the rest of the page.
    for c in review_comments
        .ok()
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        comments.push(json!({
            "id":c["node_id"].as_str().map(str::to_owned).unwrap_or_else(||c["id"].to_string()),
            "kind":"review-comment","author":actor(host,&json!({"login":c["user"]["login"],"type":c["user"]["type"]})),
            "body":c["body"].as_str().unwrap_or(""),"createdAt":c["created_at"],"url":c["html_url"].as_str(),
            "path":c["path"].as_str(),"reviewState":null,
        }));
    }
    comments.retain(|c| c["createdAt"].as_str().is_some_and(|d| !d.is_empty()));
    comments.sort_by(|a, b| a["createdAt"].as_str().cmp(&b["createdAt"].as_str()));
    let total = comments.len();
    let truncated = total > 200;
    comments.truncate(200);
    let mut review_threads = Vec::new();
    let mut threads_truncated = false;
    if let Ok(t) = &threads {
        let node = &t["data"]["repository"]["pullRequest"]["reviewThreads"];
        threads_truncated = node["totalCount"].as_u64().unwrap_or(0) > 50;
        for th in node["nodes"].as_array().into_iter().flatten() {
            let list: Vec<Value> = th["comments"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    json!({"id":c["id"],"author":gql_actor(&c["author"]),"body":c["body"].as_str().unwrap_or(""),
                    "createdAt":c["createdAt"],"editedAt":c["lastEditedAt"],"url":c["url"].as_str(),"canEdit":c["viewerCanUpdate"]==true})
                })
                .collect();
            let Some(path) = th["path"].as_str().filter(|p| !p.is_empty()) else {
                continue;
            };
            review_threads.push(json!({
                "id":th["id"],"path":path,"line":th["line"].as_u64().or_else(||th["originalLine"].as_u64()).filter(|l|*l>0),
                "side":if th["diffSide"]=="LEFT"{"left"}else{"right"},
                "isResolved":th["isResolved"]==true,"isOutdated":th["isOutdated"]==true,
                "comments":list,"commentCount":th["comments"]["totalCount"],
                "canResolve":th["viewerCanResolve"]==true,
            }));
        }
    }
    let commits: Vec<Value> = v["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            json!({"oid":c["oid"],"messageHeadline":c["messageHeadline"].as_str().unwrap_or(""),"committedDate":c["committedDate"],
            "authors":c["authors"].as_array().map(|a|a.iter().map(|x|actor(host,x)).filter(|x|!x.is_null()).collect::<Vec<_>>()).unwrap_or_default()})
        })
        .collect();
    Ok(json!({
        "author":actor(host,&v["author"]),
        "reviewers":v["reviewRequests"].as_array().map(|a|a.iter().map(|r|actor(host,r)).filter(|a|!a.is_null()).collect::<Vec<_>>()).unwrap_or_default(),
        "comments":comments,"commentCount":total,"commentsTruncated":truncated,
        "reviewThreadsTruncated":threads_truncated,"reviewThreads":review_threads,"commits":commits,
    }))
}

// ---- change breakdown ----------------------------------------------------------------------

fn category(path: &str) -> &'static str {
    let p = path.to_lowercase();
    let file = p.rsplit('/').next().unwrap_or("");
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if p.contains("/__mocks__/")
        || p.starts_with("__mocks__/")
        || file.contains(".mock.")
        || p.contains("/fixtures/")
        || p.contains("/mocks/")
    {
        "mocks"
    } else if p.contains("/test/")
        || p.contains("/tests/")
        || p.starts_with("test/")
        || p.starts_with("tests/")
        || p.contains("/__tests__/")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.ends_with("_test.go")
        || file.ends_with("_test.rs")
        || file.starts_with("test_")
    {
        "tests"
    } else if file.ends_with(".lock")
        || file == "package-lock.json"
        || file == "pnpm-lock.yaml"
        || file == "cargo.lock"
        || file.ends_with(".min.js")
        || file.ends_with(".generated.ts")
        || file.contains(".gen.")
        || p.contains("/generated/")
        || p.contains("/dist/")
        || ext == "snap"
    {
        "generated"
    } else if matches!(ext, "md" | "mdx" | "rst" | "txt" | "adoc")
        || p.starts_with("docs/")
        || p.contains("/docs/")
    {
        "docs"
    } else {
        "code"
    }
}

async fn change_breakdown(pr: &Pr) -> Reply {
    let v = view(pr, "files").await?;
    let files = v["files"].as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        return Ok(Value::Null);
    }
    let mut totals = Map::new();
    for key in ["code", "tests", "mocks", "generated", "docs"] {
        totals.insert(key.into(), json!({"additions":0,"deletions":0}));
    }
    for f in &files {
        let c = totals
            .get_mut(category(f["path"].as_str().unwrap_or("")))
            .unwrap();
        c["additions"] =
            json!(c["additions"].as_u64().unwrap_or(0) + f["additions"].as_u64().unwrap_or(0));
        c["deletions"] =
            json!(c["deletions"].as_u64().unwrap_or(0) + f["deletions"].as_u64().unwrap_or(0));
    }
    Ok(Value::Object(totals))
}

// ---- candidates ----------------------------------------------------------------------------

async fn label_candidates(pr: &Pr) -> Reply {
    let label_args = [
        "label",
        "list",
        "--repo",
        pr.repo.as_str(),
        "--json",
        "name,color,description",
        "--limit",
        "100",
    ];
    let (all, mine) = tokio::join!(pr.gh.json("labels", &label_args, None), view(pr, "labels"));
    let (all, mine) = (all?, mine?);
    let applied: Vec<&str> = mine["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str())
        .collect();
    let list = all.as_array().cloned().unwrap_or_default();
    let candidates: Vec<Value> = list
        .iter()
        .filter_map(|l| {
            let name = l["name"].as_str()?;
            Some(json!({"name":name,"color":l["color"].as_str().filter(|c|!c.is_empty()),
            "description":l["description"].as_str().filter(|c|!c.is_empty()),"isApplied":applied.contains(&name)}))
        })
        .collect();
    Ok(json!({"candidates":candidates,"truncated":list.len()>=100}))
}

async fn reviewer_candidates(pr: &Pr) -> Reply {
    let path = format!("repos/{}/assignees?per_page=100", pr.repo);
    let user_args = ["api", path.as_str()];
    let (users, mine) = tokio::join!(
        pr.gh.json("reviewers", &user_args, None),
        view(pr, "reviewRequests,author")
    );
    let (users, mine) = (users?, mine?);
    let requested: Vec<&str> = mine["reviewRequests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["login"].as_str())
        .collect();
    let author = mine["author"]["login"].as_str().unwrap_or("");
    let list = users.as_array().cloned().unwrap_or_default();
    let candidates: Vec<Value> = list
        .iter()
        .filter_map(|u| {
            let login = u["login"].as_str().filter(|l| *l != author)?;
            Some(json!({"login":login,"name":null,"avatarUrl":u["avatar_url"].as_str(),
            "isBot":u["type"]=="Bot","id":login,"kind":"user","isRequested":requested.contains(&login)}))
        })
        .collect();
    Ok(json!({"candidates":candidates,"truncated":list.len()>=100}))
}

// ---- mutations -----------------------------------------------------------------------------

fn body_of<'a>(p: &'a Value, key: &str, allow_empty: bool) -> Result<&'a str, Value> {
    let b = p[key]
        .as_str()
        .ok_or_else(|| invalid(format!("{key} is required")))?;
    if (b.is_empty() && !allow_empty) || b.chars().count() > 65_536 {
        return Err(invalid(format!("{key} is out of range")));
    }
    Ok(b)
}

async fn comment(pr: &Pr, p: &Value) -> Reply {
    let body = body_of(p, "body", false)?;
    pr.gh
        .run(
            "comment",
            &[
                "pr",
                "comment",
                &pr.n(),
                "--repo",
                &pr.repo,
                "--body-file",
                "-",
            ],
            Some(body),
        )
        .await?;
    Ok(Value::Null)
}

fn review_location(pos: &Value) -> Result<(u64, &'static str), Value> {
    let line = |k: &str| {
        pos[k]
            .as_u64()
            .filter(|l| *l >= 1)
            .ok_or_else(|| invalid("comment position is invalid"))
    };
    Ok(match pos["kind"].as_str() {
        Some("added") => (line("newLine")?, "RIGHT"),
        Some("deleted") => (line("oldLine")?, "LEFT"),
        Some("context") => {
            if pos["side"] == "left" {
                (line("oldLine")?, "LEFT")
            } else {
                (line("newLine")?, "RIGHT")
            }
        }
        _ => return Err(invalid("comment position is invalid")),
    })
}

async fn submit_review(pr: &Pr, p: &Value) -> Reply {
    let body = body_of(p, "body", true)?;
    let event = match p["verdict"].as_str() {
        Some("approve") => "APPROVE",
        Some("request-changes") => "REQUEST_CHANGES",
        Some("comment") => "COMMENT",
        _ => return Err(invalid("verdict is invalid")),
    };
    let drafts = p["comments"].as_array().cloned().unwrap_or_default();
    if drafts.is_empty() {
        let flag = match event {
            "APPROVE" => "--approve",
            "REQUEST_CHANGES" => "--request-changes",
            _ => "--comment",
        };
        if event != "APPROVE" && body.is_empty() {
            return Err(invalid("body is required for this verdict"));
        }
        pr.gh
            .run(
                "submit-review",
                &[
                    "pr",
                    "review",
                    &pr.n(),
                    "--repo",
                    &pr.repo,
                    flag,
                    "--body-file",
                    "-",
                ],
                Some(body),
            )
            .await?;
        return Ok(Value::Null);
    }
    let mut comments = Vec::new();
    for d in &drafts {
        let path = d["path"]
            .as_str()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| invalid("comment path is required"))?;
        let text = body_of(d, "body", false)?;
        let (line, side) = review_location(&d["position"])?;
        let mut c = json!({"path":path,"body":text,"line":line,"side":side});
        if d.get("startPosition").is_some_and(|s| !s.is_null()) {
            let (start, start_side) = review_location(&d["startPosition"])?;
            if start != line || start_side != side {
                c["start_line"] = json!(start);
                c["start_side"] = json!(start_side);
            }
        }
        comments.push(c);
    }
    let payload = json!({"event":event,"body":body,"comments":comments}).to_string();
    let path = format!("repos/{}/pulls/{}/reviews", pr.repo, pr.number);
    pr.gh
        .run(
            "submit-review",
            &["api", "-X", "POST", &path, "--input", "-"],
            Some(&payload),
        )
        .await?;
    Ok(Value::Null)
}

async fn run_action(s: &State, pr: &Pr, p: &Value) -> Reply {
    if p.get("stackNumber").is_some_and(|v| !v.is_null()) {
        return Err(operation(
            "run-action",
            "Stacked pull request actions are not supported.",
            false,
        ));
    }
    let n = pr.n();
    let repo = pr.repo.as_str();
    let action = p["action"].as_str().unwrap_or("");
    let method = match p["mergeMethod"].as_str() {
        Some("merge") => Some("--merge"),
        Some("squash") => Some("--squash"),
        Some("rebase") => Some("--rebase"),
        Some(_) => return Err(invalid("mergeMethod is invalid")),
        None => None,
    };
    let mut args: Vec<&str> = vec!["pr"];
    let mut merge_text = None;
    if matches!(action, "merge" | "enable-auto-merge")
        && crate::source_control::effective(s, &pr.gh.cwd)["removeAgentCreditsOnMerge"] == true
    {
        if method == Some("--rebase") {
            return Err(operation(
                "run-action",
                "Removing credits requires a merge or squash commit.",
                false,
            ));
        }
        let v = view(pr, "title,body").await?;
        merge_text = Some((
            strip_agent_credits(v["title"].as_str().unwrap_or("")),
            strip_agent_credits(v["body"].as_str().unwrap_or("")),
        ));
    }
    match action {
        "merge" => {
            args.extend(["merge", &n, "--repo", repo, method.unwrap_or("--merge")]);
        }
        "enable-auto-merge" => {
            args.extend([
                "merge",
                &n,
                "--repo",
                repo,
                "--auto",
                method.unwrap_or("--merge"),
            ]);
        }
        "disable-auto-merge" => args.extend(["merge", &n, "--repo", repo, "--disable-auto"]),
        "ready" => args.extend(["ready", &n, "--repo", repo]),
        "draft" => args.extend(["ready", &n, "--repo", repo, "--undo"]),
        "close" => args.extend(["close", &n, "--repo", repo]),
        "reopen" => args.extend(["reopen", &n, "--repo", repo]),
        "update-branch" => {
            args.extend(["update-branch", &n, "--repo", repo]);
            if p["updateMethod"] == "rebase" {
                args.push("--rebase");
            }
        }
        "delete-branch" => {
            let v = view(pr, "headRefName,headRepository,headRepositoryOwner,state").await?;
            if lower(&v["state"]) == "open" {
                return Err(operation(
                    "run-action",
                    "Close or merge the pull request before deleting its branch.",
                    false,
                ));
            }
            let branch = v["headRefName"]
                .as_str()
                .filter(|b| !b.is_empty())
                .ok_or_else(|| operation("run-action", "The head branch is unknown.", true))?;
            let head = match (
                v["headRepositoryOwner"]["login"].as_str(),
                v["headRepository"]["name"].as_str(),
            ) {
                (Some(o), Some(nm)) => format!("{o}/{nm}"),
                _ => {
                    return Err(operation(
                        "run-action",
                        "The head repository was deleted.",
                        true,
                    ))
                }
            };
            let path = format!("repos/{head}/git/refs/heads/{branch}");
            pr.gh
                .run("run-action", &["api", "-X", "DELETE", &path], None)
                .await?;
            return Ok(Value::Null);
        }
        "revert" | "approve-workflows" => {
            return Err(operation(
                "run-action",
                &format!("The {action} action is not supported."),
                false,
            ))
        }
        _ => return Err(invalid("action is invalid")),
    }
    if let Some((subject, body)) = &merge_text {
        args.extend(["--subject", subject, "--body", body]);
    }
    pr.gh.run("run-action", &args, None).await?;
    Ok(Value::Null)
}

async fn request_reviewers(pr: &Pr, p: &Value) -> Reply {
    let list = p["reviewers"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 25)
        .ok_or_else(|| invalid("reviewers is out of range"))?;
    let (mut users, mut teams) = (Vec::new(), Vec::new());
    for r in list {
        let id = r["id"]
            .as_str()
            .filter(|i| !i.is_empty())
            .ok_or_else(|| invalid("reviewer id is required"))?;
        match r["kind"].as_str() {
            Some("user") => users.push(id.to_owned()),
            Some("team") => teams.push(id.rsplit('/').next().unwrap_or(id).to_owned()),
            _ => return Err(invalid("reviewer kind is invalid")),
        }
    }
    let method = if p["requested"] == true {
        "POST"
    } else {
        "DELETE"
    };
    let path = format!("repos/{}/pulls/{}/requested_reviewers", pr.repo, pr.number);
    let payload = json!({"reviewers":users,"team_reviewers":teams}).to_string();
    pr.gh
        .run(
            "request-reviewers",
            &["api", "-X", method, &path, "--input", "-"],
            Some(&payload),
        )
        .await?;
    Ok(Value::Null)
}

async fn set_labels(pr: &Pr, p: &Value) -> Reply {
    let list = p["labels"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 25)
        .ok_or_else(|| invalid("labels is out of range"))?;
    let names: Vec<&str> = list
        .iter()
        .filter_map(Value::as_str)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if names.len() != list.len() {
        return Err(invalid("labels must be non-empty strings"));
    }
    let joined = names.join(",");
    let flag = if p["applied"] == true {
        "--add-label"
    } else {
        "--remove-label"
    };
    pr.gh
        .run(
            "set-labels",
            &["pr", "edit", &pr.n(), "--repo", &pr.repo, flag, &joined],
            None,
        )
        .await?;
    Ok(Value::Null)
}

// ---- list and filter options ---------------------------------------------------------------

const LIST_FIELDS: &str = "number,title,url,author,headRefName,baseRefName,state,isDraft,mergeable,additions,deletions,createdAt,updatedAt,labels,reviewDecision,statusCheckRollup,reviewRequests";

fn requested_projects(s: &State, p: &Value) -> Vec<Project> {
    let mut wanted: Vec<String> = p["projectIds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Some(id) = p["projectId"].as_str() {
        wanted.push(id.into());
    }
    let all = live_projects(s);
    if wanted.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|pr| wanted.contains(&pr.id))
            .collect()
    }
}

fn list_search_terms(p: &Value, involvement: &str) -> Vec<String> {
    let mut terms = search_terms(p);
    if involvement == "reviewing" {
        terms.insert(0, "review-requested:@me".into());
    }
    terms
}
fn search_terms(p: &Value) -> Vec<String> {
    let mut terms = Vec::new();
    let f = &p["filters"];
    match f["draft"].as_str() {
        Some("only") => terms.push("draft:true".to_owned()),
        Some("hide") => terms.push("draft:false".to_owned()),
        _ => {}
    }
    match f["review"].as_str() {
        Some("none") => terms.push("review:none".into()),
        Some(r @ ("approved" | "changes-requested" | "review-required")) => {
            terms.push(format!("review:{}", r.replace('-', "_")))
        }
        _ => {}
    }
    match f["checks"].as_str() {
        Some("passing") => terms.push("status:success".into()),
        Some("failing") => terms.push("status:failure".into()),
        _ => {}
    }
    for group in f["labels"].as_array().into_iter().flatten() {
        let names: Vec<String> = group
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|l| format!("\"{}\"", l.replace('"', "")))
            .collect();
        if !names.is_empty() {
            terms.push(format!("label:{}", names.join(",")));
        }
    }
    for l in f["excludedLabels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        terms.push(format!("-label:\"{}\"", l.replace('"', "")));
    }
    if let Some(q) = p["query"].as_str().filter(|q| !q.trim().is_empty()) {
        terms.push(q.trim().to_owned());
    }
    terms
}

async fn list(s: &State, p: &Value) -> Reply {
    let state = p["state"]
        .as_str()
        .ok_or_else(|| invalid("state is required"))?;
    if !matches!(state, "all" | "open" | "closed" | "merged") {
        return Err(invalid("state is invalid"));
    }
    let limit = p["limit"].as_u64().unwrap_or(50).clamp(1, 500);
    let involvement = p["involvement"].as_str().unwrap_or("all");
    let projects = requested_projects(s, p);
    let wanted_host = p["host"].as_str();
    let mut entries: Vec<Value> = Vec::new();
    let mut errors = Vec::new();
    let mut viewers = Map::new();
    let mut hosts: Vec<(String, usize)> = Vec::new();
    let mut seen_repos: Vec<(String, String)> = Vec::new();
    let mut truncated = false;
    let mut first_error: Option<Value> = None;
    for project in projects {
        let Some((host, repo)) = remote(&project.root).await else {
            continue;
        };
        if wanted_host.is_some_and(|h| h != host) {
            continue;
        }
        if !is_github(&host) {
            errors.push(json!({"projectId":project.id,"projectTitle":project.title,"message":"Change requests cannot be browsed for this project's host yet."}));
            continue;
        }
        match hosts.iter_mut().find(|(h, _)| *h == host) {
            Some((_, n)) => *n += 1,
            None => hosts.push((host.clone(), 1)),
        }
        if seen_repos.contains(&(host.clone(), repo.clone())) {
            continue;
        }
        seen_repos.push((host.clone(), repo.clone()));
        let gh = Gh {
            cwd: project.root.clone(),
            host: host.clone(),
        };
        let login = match viewer(&gh).await {
            Ok(l) => l,
            Err(e) => {
                errors.push(json!({"projectId":project.id,"projectTitle":project.title,"message":e["requirement"].as_str().or(e["detail"].as_str()).unwrap_or("The GitHub CLI is not available.")}));
                first_error.get_or_insert(e);
                continue;
            }
        };
        viewers.insert(host.clone(), json!(login));
        let limit_text = (limit + 1).to_string();
        let mut args: Vec<String> = [
            "pr",
            "list",
            "--repo",
            &repo,
            "--state",
            state,
            "--limit",
            &limit_text,
            "--json",
            LIST_FIELDS,
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();
        if involvement == "authored" {
            args.extend(["--author".into(), "@me".into()]);
        }
        if let Some(author) = p["filters"]["author"]
            .as_str()
            .filter(|a| !a.trim().is_empty())
        {
            let author = if author
                .trim()
                .trim_start_matches('@')
                .eq_ignore_ascii_case("me")
            {
                login.clone()
            } else {
                author.trim().to_owned()
            };
            args.extend(["--author".into(), author]);
        }
        let terms = list_search_terms(p, involvement);
        if !terms.is_empty() {
            args.extend(["--search".into(), terms.join(" ")]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match gh.json("list", &refs, None).await {
            Ok(v) => {
                let rows = v.as_array().cloned().unwrap_or_default();
                if rows.len() as u64 > limit {
                    truncated = true;
                }
                for r in rows.iter().take(limit as usize) {
                    let checks = checks_of(&r["statusCheckRollup"]);
                    let requested = r["reviewRequests"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|x| x["login"] == login.as_str());
                    let mut e = json!({
                        "provider":"github","host":host,"projectId":project.id,"projectTitle":project.title,"repository":repo,
                        "number":r["number"],"title":r["title"],"url":r["url"],"author":actor(&host,&r["author"]),
                        "headBranch":r["headRefName"],"baseBranch":r["baseRefName"],"state":state_of(&r["state"]),"isDraft":r["isDraft"]==true,
                        "mergeability":mergeability(&r["mergeable"]),"additions":r["additions"].as_u64().unwrap_or(0),
                        "deletions":r["deletions"].as_u64().unwrap_or(0),"createdAt":r["createdAt"],"updatedAt":r["updatedAt"],
                        "observedAt":crate::store::now_ms(),"viewerReviewRequested":requested,"labels":labels(&r["labels"]),
                    });
                    if let Some(d) = review_decision(&r["reviewDecision"]) {
                        e["reviewDecision"] = json!(d);
                    }
                    if let Some(c) = checks_state(&checks) {
                        e["checksState"] = json!(c);
                    }
                    entries.push(e);
                }
            }
            Err(e) => {
                errors.push(json!({"projectId":project.id,"projectTitle":project.title,"message":e["detail"].as_str().or(e["requirement"].as_str()).unwrap_or("The listing failed.")}));
                first_error.get_or_insert(e);
            }
        }
    }
    if entries.is_empty() && !errors.is_empty() {
        if let Some(e) = first_error.filter(|e| e["_tag"] == "PullRequestUnavailableError") {
            return Err(e);
        }
    }
    entries.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
    if entries.len() as u64 > limit {
        entries.truncate(limit as usize);
        truncated = true;
    }
    let providers: Vec<Value> = hosts
        .iter()
        .map(|(h, n)| json!({"host":h,"kind":"github","searchesOnHost":false,"projectCount":n,"configured":viewers.contains_key(h),"detail":null}))
        .collect();
    Ok(
        json!({"viewers":viewers,"providers":providers,"entries":entries,"errors":errors,"truncated":truncated,"nextCursors":{}}),
    )
}

fn state_of(v: &Value) -> &'static str {
    state(v)
}

async fn filter_options(s: &State, p: &Value) -> Reply {
    let kind = p["kind"]
        .as_str()
        .ok_or_else(|| invalid("kind is required"))?;
    if !matches!(kind, "labels" | "authors") {
        return Err(invalid("kind is invalid"));
    }
    let query = p["query"].as_str().unwrap_or("").to_lowercase();
    let (mut label_out, mut author_out): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
    let mut truncated = false;
    let mut seen = Vec::new();
    let mut first_error = None;
    for project in requested_projects(s, p) {
        let Some((host, repo)) = remote(&project.root).await else {
            continue;
        };
        if !is_github(&host)
            || p["host"].as_str().is_some_and(|h| h != host)
            || seen.contains(&(host.clone(), repo.clone()))
        {
            continue;
        }
        seen.push((host.clone(), repo.clone()));
        let gh = Gh {
            cwd: project.root.clone(),
            host: host.clone(),
        };
        let result = if kind == "labels" {
            gh.json(
                "filter-options",
                &[
                    "label",
                    "list",
                    "--repo",
                    &repo,
                    "--json",
                    "name,color",
                    "--limit",
                    "100",
                ],
                None,
            )
            .await
        } else {
            gh.json(
                "filter-options",
                &[
                    "pr", "list", "--repo", &repo, "--state", "all", "--limit", "100", "--json",
                    "author",
                ],
                None,
            )
            .await
        };
        match result {
            Ok(v) => {
                let rows = v.as_array().cloned().unwrap_or_default();
                truncated |= rows.len() >= 100;
                for r in rows {
                    if kind == "labels" {
                        if let Some(l) = label(&r).filter(|l| {
                            l["name"]
                                .as_str()
                                .is_some_and(|n| n.to_lowercase().contains(&query))
                        }) {
                            if !label_out.iter().any(|x| x["name"] == l["name"]) {
                                label_out.push(l);
                            }
                        }
                    } else {
                        let a = actor(&host, &r["author"]);
                        if a["login"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(&query))
                            && !author_out.iter().any(|x| x["login"] == a["login"])
                        {
                            author_out.push(a);
                        }
                    }
                }
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if label_out.is_empty() && author_out.is_empty() {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    Ok(json!({"labels":label_out,"authors":author_out,"truncated":truncated}))
}

// ---- git.preparePullRequestThread ----------------------------------------------------------

async fn prepare_thread(s: &State, p: &Value) -> Reply {
    let cwd = PathBuf::from(str_field(p, "cwd")?);
    let reference = str_field(p, "reference")?.trim();
    let mode = p["mode"]
        .as_str()
        .ok_or_else(|| invalid("mode is required"))?;
    if !matches!(mode, "local" | "worktree") {
        return Err(invalid("mode is invalid"));
    }
    let git_error = |detail: &str| json!({"_tag":"GitManagerError","operation":"git.preparePullRequestThread","cwd":cwd,"detail":detail,"message":detail});
    // `123`, `#123` or a pull request URL.
    let number: u64 = reference
        .trim_start_matches('#')
        .parse()
        .ok()
        .or_else(|| reference.rsplit('/').next().and_then(|n| n.parse().ok()))
        .filter(|n| *n >= 1)
        .ok_or_else(|| git_error("The pull request reference is not a number or URL."))?;
    let (host, repo) = remote(&cwd)
        .await
        .ok_or_else(|| git_error("The repository has no origin remote."))?;
    if !is_github(&host) {
        return Err(git_error(
            "Pull requests are only supported for GitHub remotes.",
        ));
    }
    let gh = Gh {
        cwd: cwd.clone(),
        host,
    };
    let n = number.to_string();
    let v = gh
        .json("resolve", &["pr", "view", &n, "--repo", &repo, "--json", "number,title,url,baseRefName,headRefName,state,isCrossRepository,headRepositoryOwner"], None)
        .await
        .map_err(|e| git_error(e["detail"].as_str().or(e["requirement"].as_str()).unwrap_or("The pull request could not be read.")))?;
    let head = v["headRefName"]
        .as_str()
        .filter(|b| !b.is_empty() && !b.starts_with('-'))
        .ok_or_else(|| git_error("The pull request has no head branch."))?
        .to_owned();
    let local = format!("pr/{number}");
    let refspec = format!("pull/{number}/head:refs/o3/pull/{number}");
    let fetch = proc::git(
        &cwd,
        &["fetch", "--no-tags", "origin", &refspec, "--force"],
        Opts::NETWORK,
    )
    .await;
    match fetch {
        Ok(o) if o.ok() => {}
        Ok(o) => {
            return Err(git_error(&format!(
                "Could not fetch the pull request: {}",
                o.stderr
            )))
        }
        Err(e) => {
            return Err(git_error(&format!(
                "Could not fetch the pull request: {e:?}"
            )))
        }
    }
    let fetched = format!("refs/o3/pull/{number}");
    let resolved = json!({"number":number,"title":v["title"],"url":v["url"],"baseBranch":v["baseRefName"],"headBranch":head,"state":state(&v["state"])});
    let mut worktree = Value::Null;
    let mut on_head = true;
    if mode == "local" {
        // Never disturb local work: only switch when the tree is clean.
        let dirty = proc::git(&cwd, &["status", "--porcelain"], Opts::READ)
            .await
            .map(|o| !o.text().trim().is_empty())
            .unwrap_or(true);
        if dirty {
            return Err(git_error("The working tree has local changes; commit or stash them before checking out a pull request."));
        }
        let ok = proc::git(&cwd, &["checkout", "-B", &local, &fetched], Opts::WRITE).await;
        if !matches!(ok, Ok(ref o) if o.ok()) {
            return Err(git_error("Could not check out the pull request branch."));
        }
    } else {
        let directory = crate::settings::read(s)
            .ok()
            .and_then(|v| {
                v["worktreesDirectory"]
                    .as_str()
                    .filter(|d| !d.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| s.dir.parent().unwrap_or(&s.dir).join("worktrees"));
        let name = p["threadId"]
            .as_str()
            .map(|t| t.bytes().map(|b| format!("{b:02x}")).collect::<String>())
            .unwrap_or_else(|| format!("pr-{number}"));
        let path = directory.join(name);
        std::fs::create_dir_all(&directory).map_err(|e| git_error(&e.to_string()))?;
        let path_text = path.to_string_lossy().into_owned();
        if path.exists() {
            let dirty = proc::git(&path, &["status", "--porcelain"], Opts::READ)
                .await
                .map(|o| !o.text().trim().is_empty())
                .unwrap_or(true);
            let moved = !dirty
                && matches!(proc::git(&path, &["checkout", "-B", &local, &fetched], Opts::WRITE).await, Ok(ref o) if o.ok());
            on_head = moved;
        } else {
            let added = proc::git(
                &cwd,
                &["worktree", "add", "-B", &local, "--", &path_text, &fetched],
                Opts::WRITE,
            )
            .await;
            if !matches!(added, Ok(ref o) if o.ok()) {
                return Err(git_error("Could not create the pull request worktree."));
            }
        }
        worktree = json!(path_text);
    }
    Ok(
        json!({"pullRequest":resolved,"branch":local,"worktreePath":worktree,"isOnPullRequestHead":on_head}),
    )
}

async fn review_guide(s: &State, tag: &str, p: &Value) -> Reply {
    let pr = Pr::resolve(s, p).await?;
    let key = json!([p["projectId"], pr.gh.host, pr.repo, pr.number]).to_string();
    if tag == "pullRequests.reviewGuide" {
        return Ok(events::get(&s.db.lock().unwrap(), "reviewGuides", &key).unwrap_or(Value::Null));
    }
    let settings = crate::source_control::effective(s, &pr.project.root);
    if settings["reviewGuidesEnabled"] != true {
        return Err(operation(tag, "Review guides are disabled", false));
    }
    let diff = pr
        .gh
        .run(
            "review-guide",
            &["pr", "diff", &pr.n(), "--repo", &pr.repo],
            None,
        )
        .await?;
    crate::title::check_settings(s, &settings, "reviewGuideModelSelection")
        .map_err(|message| operation(tag, &message, false))?;
    let text = crate::source_control::generate(s, &pr.project.root, "review", &diff)
        .await
        .ok_or_else(|| operation(tag, "Review generation failed or requested tools", false))?;
    let value = json!({"id":key,"projectId":p["projectId"],"repository":pr.repo,"number":pr.number,"text":text,"updatedAt":crate::store::iso(crate::store::now_ms())});
    events::put(&s.db.lock().unwrap(), "reviewGuides", &key, "", &value)
        .map_err(|e| operation(tag, &e, false))?;
    Ok(value)
}

fn strip_agent_credits(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let l = line.trim().to_ascii_lowercase();
            !(l.starts_with("co-authored-by:")
                && ["claude", "codex", "copilot", "noreply@anthropic"]
                    .iter()
                    .any(|name| l.contains(name))
                || l.starts_with("generated with ")
                || l.starts_with("generated-with:")
                || l.starts_with("🤖 generated"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviewing_and_filters_share_one_search_expression() {
        assert_eq!(
            list_search_terms(
                &json!({"filters":{"draft":"hide"},"query":"label:bug"}),
                "reviewing"
            ),
            ["review-requested:@me", "draft:false", "label:bug"]
        );
    }
    #[test]
    #[ignore = "read-only query with installed authenticated gh and GH_DEBUG=api"]
    fn live_gh_merged_search_reaches_the_real_api() {
        let terms = list_search_terms(&json!({"filters":{"draft":"hide"}}), "reviewing").join(" ");
        let binary = crate::executable::resolve(std::path::Path::new("gh")).unwrap();
        let output = std::process::Command::new(binary)
            .args([
                "pr",
                "list",
                "--repo",
                "avillagran/o3-server",
                "--search",
                &terms,
                "--limit",
                "1",
                "--json",
                LIST_FIELDS,
            ])
            .env("GH_DEBUG", "api")
            .output()
            .unwrap();
        let debug = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "gh query failed");
        assert!(
            debug.contains("review-requested:@me") && debug.contains("draft:false"),
            "both qualifiers must reach GitHub"
        );
        assert!(serde_json::from_slice::<Value>(&output.stdout)
            .unwrap()
            .is_array());
    }
    #[test]
    fn remotes_parse() {
        for (u, h, r) in [
            ("git@github.com:o/r.git", "github.com", "o/r"),
            ("https://github.com/o/r", "github.com", "o/r"),
            (
                "ssh://git@ghe.example.com:2222/o/r.git",
                "ghe.example.com",
                "o/r",
            ),
        ] {
            assert_eq!(parse_remote(u), Some((h.into(), r.into())), "{u}");
        }
        assert_eq!(parse_remote("/local/path"), None);
    }
    #[test]
    fn paths_are_classified() {
        assert_eq!(category("src/a.rs"), "code");
        assert_eq!(category("tests/a.rs"), "tests");
        assert_eq!(category("web/a.test.ts"), "tests");
        assert_eq!(category("Cargo.lock"), "generated");
        assert_eq!(category("docs/x.md"), "docs");
        assert_eq!(category("a/__mocks__/b.ts"), "mocks");
    }
}
