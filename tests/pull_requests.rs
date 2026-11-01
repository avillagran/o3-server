//! `pullRequests.*` and `git.preparePullRequestThread` through the real client. `gh` is a scripted
//! stand-in on PATH that logs its argv, one line per invocation, so the tests assert both answers
//! and the exact argument vectors (no shell is ever involved).
mod common;
use common::*;
use serde_json::{json, Value};

/// `gh` is one executable per process: tests take turns.
static GH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn install_gh(body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let p = bin_dir().join("gh");
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}/gh.argv\n{body}\n",
            bin_dir().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn argv() -> String {
    std::fs::read_to_string(bin_dir().join("gh.argv")).unwrap_or_default()
}

const PR: &str = r#"{"number":7,"title":"Add thing","body":"Body","url":"https://github.com/o/r/pull/7","author":{"login":"octo","is_bot":false},"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"BEHIND","additions":10,"deletions":2,"changedFiles":3,"headRefName":"feat","headRefOid":"abc123","headRepository":{"name":"r"},"headRepositoryOwner":{"login":"o"},"baseRefName":"main","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","mergedAt":"0001-01-01T00:00:00Z","closedAt":"0001-01-01T00:00:00Z","reviewRequests":[{"login":"me"}],"labels":[{"name":"bug","color":"ff0000"}],"statusCheckRollup":[{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://ci/1"},{"__typename":"StatusContext","context":"lint","state":"PENDING"}],"autoMergeRequest":null,"reviewDecision":"REVIEW_REQUIRED","comments":[{"id":"IC_1","author":{"login":"octo"},"body":"nice","createdAt":"2026-01-01T01:00:00Z","updatedAt":"2026-01-01T01:00:00Z","url":"https://github.com/o/r/pull/7#c1"}],"reviews":[{"id":"R_1","author":{"login":"rev"},"body":"","state":"APPROVED","submittedAt":"2026-01-01T02:00:00Z"},{"id":"R_2","author":{"login":"rev"},"body":"","state":"COMMENTED","submittedAt":"2026-01-01T03:00:00Z"}],"commits":[{"oid":"abc123","messageHeadline":"feat","committedDate":"2026-01-01T00:30:00Z","authors":[{"login":"octo"}]}],"files":[{"path":"src/a.rs","additions":5,"deletions":1},{"path":"tests/a.rs","additions":3,"deletions":0},{"path":"Cargo.lock","additions":2,"deletions":1}]}"#;

fn script() -> String {
    format!(
        r#"case "$1 $2" in
"api user") echo me;;
"pr list") echo '[{pr}]';;
"pr view") echo '{pr}';;
"repo view") echo '{{"mergeCommitAllowed":true,"squashMergeAllowed":true,"rebaseMergeAllowed":false}}';;
"label list") echo '[{{"name":"bug","color":"ff0000","description":""}},{{"name":"docs","color":"0000ff","description":""}}]';;
"api graphql") echo '{{"data":{{"repository":{{"pullRequest":{{"reviewThreads":{{"totalCount":1,"nodes":[{{"id":"T_1","isResolved":false,"isOutdated":false,"path":"src/a.rs","line":3,"diffSide":"RIGHT","viewerCanResolve":true,"comments":{{"totalCount":1,"nodes":[{{"id":"TC_1","body":"fix","createdAt":"2026-01-01T04:00:00Z","lastEditedAt":null,"url":"u","viewerCanUpdate":false,"author":{{"login":"rev","avatarUrl":"a"}}}}]}}}}]}}}}}}}}}}';;
"api repos/o/r/pulls/7/comments?per_page=100") echo '[{{"node_id":"RC_1","id":9,"user":{{"login":"rev","type":"User"}},"body":"inline","created_at":"2026-01-01T05:00:00Z","html_url":"h","path":"src/a.rs"}}]';;
"api repos/o/r/assignees?per_page=100") echo '[{{"login":"octo","type":"User","avatar_url":"x"}},{{"login":"rev","type":"User","avatar_url":"y"}},{{"login":"me","type":"User"}}]';;
"api -X") cat >> {d}/gh.stdin;;
"pr review") cat >> {d}/gh.stdin;;
"pr comment") cat >> {d}/gh.stdin;;
"pr merge"|"pr ready"|"pr close"|"pr reopen"|"pr edit"|"pr update-branch") ;;
*) echo "unexpected $*" >&2; exit 1;;
esac"#,
        pr = PR,
        d = bin_dir().display()
    )
}

#[test]
fn github_pull_requests_through_gh_argv() {
    let _turn = GH.lock().unwrap_or_else(|p| p.into_inner());
    let e = setup("prs");
    install_gh(&script());
    let c = &e.conn;
    let r = e.dir("repo");
    repo(&r);
    write(&r.join("a"), "x");
    commit_all(&r, "init");
    git(
        &r,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    e.project("p1", &r);

    let list = c
        .rpc(
            "pullRequests.list",
            json!({"state":"open","projectId":"p1"}),
        )
        .unwrap();
    assert_eq!(list["viewers"]["github.com"], "me");
    assert_eq!(list["providers"][0]["kind"], "github");
    let entry = &list["entries"][0];
    assert_eq!(entry["repository"], "o/r");
    assert_eq!(entry["number"], 7);
    assert_eq!(entry["state"], "open");
    assert_eq!(entry["viewerReviewRequested"], true);
    assert_eq!(entry["reviewDecision"], "review-required");
    assert_eq!(entry["checksState"], "pending");
    assert_eq!(entry["labels"][0]["name"], "bug");
    assert!(argv().contains("pr list --repo o/r --state open --limit 51 --json"));

    let rf = json!({"projectId":"p1","repository":"o/r","number":7});
    let d = c.rpc("pullRequests.detail", rf.clone()).unwrap();
    assert_eq!(
        (
            d["provider"].as_str(),
            d["state"].as_str(),
            d["baseComparison"].as_str()
        ),
        (Some("github"), Some("open"), Some("behind"))
    );
    assert_eq!(d["headSha"], "abc123");
    assert_eq!(
        d["mergeCapabilities"],
        json!({"merge":true,"squash":true,"rebase":false})
    );
    assert!(
        d["mergedAt"].is_null(),
        "zero dates are not reported as merge times"
    );
    // Honest capabilities: nothing GitHub cannot do through this CLI is advertised.
    assert_eq!(d["capabilities"]["diff"], false);
    assert_eq!(d["capabilities"]["review"]["resolve"], false);

    let ch = c.rpc("pullRequests.checks", rf.clone()).unwrap();
    let names: Vec<_> = ch["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_owned(),
                c["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("ci".into(), "success".into()),
            ("lint".into(), "pending".into())
        ]
    );

    let act = c.rpc("pullRequests.activity", rf.clone()).unwrap();
    let kinds: Vec<_> = act["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    // The empty COMMENTED review is dropped; the rest is ordered by time.
    assert_eq!(kinds, ["issue-comment", "review", "review-comment"]);
    assert_eq!(act["reviewThreads"][0]["path"], "src/a.rs");
    assert_eq!(act["reviewThreads"][0]["canResolve"], true);
    assert_eq!(act["commits"][0]["oid"], "abc123");
    let cb = c.rpc("pullRequests.changeBreakdown", rf.clone()).unwrap();
    assert_eq!(
        (
            cb["code"]["additions"].as_u64(),
            cb["tests"]["additions"].as_u64(),
            cb["generated"]["deletions"].as_u64()
        ),
        (Some(5), Some(3), Some(1))
    );
    let rc = c
        .rpc("pullRequests.reviewerCandidates", rf.clone())
        .unwrap();
    let logins: Vec<_> = rc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["login"].as_str().unwrap())
        .collect();
    assert_eq!(logins, ["rev", "me"], "the author is not a candidate");
    assert_eq!(rc["candidates"][1]["isRequested"], true);
    let rt = c.rpc("pullRequests.routing", rf.clone()).unwrap();
    assert_eq!(
        (rt["viewer"].as_str(), rt["host"].as_str()),
        (Some("me"), Some("github.com"))
    );
    let ri = c
        .rpc("pullRequests.routingIdentity", json!({"host":"github.com"}))
        .unwrap();
    assert_eq!(
        (ri["viewer"].as_str(), ri["provider"].as_str()),
        (Some("me"), Some("github"))
    );
    let fo = c
        .rpc(
            "pullRequests.filterOptions",
            json!({"kind":"labels","projectId":"p1"}),
        )
        .unwrap();
    assert!(fo["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["name"] == "bug"));
    let lc = c.rpc("pullRequests.labelCandidates", rf.clone()).unwrap();
    assert!(lc.to_string().contains("docs"));

    // Mutations: body travels on stdin, never in argv.
    let with = |extra: Value| {
        let mut v = rf.clone();
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    c.rpc(
        "pullRequests.comment",
        with(json!({"body":"hello; rm -rf /"})),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(bin_dir().join("gh.stdin")).unwrap(),
        "hello; rm -rf /"
    );
    assert!(!argv().contains("rm -rf"));
    std::fs::write(bin_dir().join("gh.stdin"), "").unwrap();
    c.rpc("pullRequests.submitReview", with(json!({"verdict":"comment","body":"overall","comments":[{"path":"src/a.rs","body":"here","position":{"kind":"added","newLine":3}}]}))).unwrap();
    let sent: Value =
        serde_json::from_str(&std::fs::read_to_string(bin_dir().join("gh.stdin")).unwrap())
            .unwrap();
    assert_eq!(
        (
            sent["event"].as_str(),
            sent["comments"][0]["side"].as_str(),
            sent["comments"][0]["line"].as_u64()
        ),
        (Some("COMMENT"), Some("RIGHT"), Some(3))
    );
    c.rpc(
        "pullRequests.submitReview",
        with(json!({"verdict":"approve","body":""})),
    )
    .unwrap();
    assert!(argv().contains("pr review 7 --repo o/r --approve --body-file -"));
    assert!(c
        .rpc(
            "pullRequests.submitReview",
            with(json!({"verdict":"request-changes","body":""}))
        )
        .is_err());
    c.rpc(
        "pullRequests.requestReviewers",
        with(json!({"reviewers":[{"id":"rev","kind":"user"}],"requested":true})),
    )
    .unwrap();
    assert!(argv().contains("api -X POST repos/o/r/pulls/7/requested_reviewers --input -"));
    c.rpc(
        "pullRequests.runAction",
        with(json!({"action":"merge","mergeMethod":"squash"})),
    )
    .unwrap();
    assert!(argv().contains("pr merge 7 --repo o/r --squash"));
    c.rpc("pullRequests.runAction", with(json!({"action":"draft"})))
        .unwrap();
    assert!(argv().contains("pr ready 7 --repo o/r --undo"));
    c.rpc(
        "pullRequests.setLabels",
        with(json!({"labels":["docs"],"applied":true})),
    )
    .unwrap();
    assert!(argv().contains("pr edit 7 --repo o/r --add-label docs"));
    assert!(c
        .rpc("pullRequests.runAction", with(json!({"action":"revert"})))
        .is_err());
    assert!(c
        .rpc(
            "pullRequests.runAction",
            with(json!({"action":"merge","mergeMethod":"zap"}))
        )
        .is_err());
    assert!(c
        .rpc("pullRequests.comment", with(json!({"body":""})))
        .is_err());
    // Repository names that could be read as flags are refused before any process starts.
    let before = argv().lines().count();
    assert!(c
        .rpc(
            "pullRequests.detail",
            json!({"projectId":"p1","repository":"-x/y","number":7})
        )
        .is_err());
    assert_eq!(argv().lines().count(), before);
    // Unknown project.
    let err = c
        .rpc(
            "pullRequests.detail",
            json!({"projectId":"nope","repository":"o/r","number":7}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("PullRequestOperationError") || err.contains("unknown project"),
        "{err}"
    );

    // Not logged in -> typed unavailable error with the cli-unauthenticated reason.
    install_gh("echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4");
    let other = e.dir("repo2");
    repo(&other);
    write(&other.join("a"), "x");
    commit_all(&other, "init");
    git(
        &other,
        &["remote", "add", "origin", "git@github.com:o/two.git"],
    );
    e.project("p2", &other);
    let err = c
        .rpc(
            "pullRequests.list",
            json!({"state":"open","projectId":"p2"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("PullRequestUnavailableError"), "{err}");
    // The client reports only the tag; the typed body is read in-process from the same profile.
    let direct = |tag: &str, p: Value| {
        let st = std::sync::Arc::new(o3_server::store::State::open(&e.state).unwrap());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(o3_server::ext::call(&st, tag, p))
            .unwrap_err()
    };
    let body = direct(
        "pullRequests.list",
        json!({"state":"open","projectId":"p2"}),
    );
    assert_eq!(
        (body["_tag"].as_str(), body["reason"].as_str()),
        (
            Some("PullRequestUnavailableError"),
            Some("cli-unauthenticated")
        )
    );
    assert!(body["requirement"]
        .as_str()
        .unwrap()
        .contains("gh auth login"));

    // A non-GitHub host is unsupported, never faked.
    let lab = e.dir("repo3");
    repo(&lab);
    write(&lab.join("a"), "x");
    commit_all(&lab, "init");
    git(
        &lab,
        &["remote", "add", "origin", "https://gitlab.com/o/three.git"],
    );
    e.project("p3", &lab);
    let err = c
        .rpc(
            "pullRequests.detail",
            json!({"projectId":"p3","repository":"o/three","number":1,"host":"gitlab.com"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("PullRequestUnavailableError"), "{err}");
    let body = direct(
        "pullRequests.detail",
        json!({"projectId":"p3","repository":"o/three","number":1,"host":"gitlab.com"}),
    );
    assert_eq!(body["reason"], "provider-unsupported");
}

#[test]
fn prepare_pull_request_thread_fetches_pull_head_without_touching_dirty_trees() {
    let _turn = GH.lock().unwrap_or_else(|p| p.into_inner());
    let e = setup("prep");
    install_gh(&script());
    let c = &e.conn;
    let bare = e.scratch.join("origin.git");
    git(
        &e.scratch,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    let r = e.dir("clone");
    repo(&r);
    write(&r.join("a"), "base\n");
    commit_all(&r, "init");
    git(
        &r,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    git(
        &r,
        &[
            "config",
            &format!("url.{}.insteadOf", bare.display()),
            "https://github.com/o/r.git",
        ],
    );
    git(&r, &["push", "-q", "origin", "main"]);
    // The pull head, as GitHub publishes it.
    git(&r, &["checkout", "-q", "-b", "feat"]);
    write(&r.join("a"), "base\nfeature\n");
    commit_all(&r, "feature");
    let head = git(&r, &["rev-parse", "HEAD"]).trim().to_owned();
    git(
        &bare,
        &[
            "fetch",
            "-q",
            r.to_str().unwrap(),
            &format!("{head}:refs/pull/7/head"),
        ],
    );
    git(&r, &["checkout", "-q", "main"]);

    // Local mode refuses to switch a dirty tree.
    write(&r.join("a"), "dirty\n");
    let err = c
        .rpc(
            "git.preparePullRequestThread",
            json!({"cwd":r.to_str().unwrap(),"reference":"7","mode":"local"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("local changes"), "{err}");
    assert_eq!(std::fs::read_to_string(r.join("a")).unwrap(), "dirty\n");
    git(&r, &["checkout", "--", "a"]);
    let local = c
        .rpc(
            "git.preparePullRequestThread",
            json!({"cwd":r.to_str().unwrap(),"reference":"#7","mode":"local"}),
        )
        .unwrap();
    assert_eq!(local["isOnPullRequestHead"], true);
    assert_eq!(git(&r, &["branch", "--show-current"]).trim(), "pr/7");
    git(&r, &["checkout", "-q", "main"]);

    let prepared = c.rpc("git.preparePullRequestThread", json!({"cwd":r.to_str().unwrap(),"reference":"https://github.com/o/r/pull/7","mode":"worktree","threadId":"th-1"})).unwrap();
    assert_eq!(prepared["branch"], "pr/7");
    assert_eq!(prepared["pullRequest"]["number"], 7);
    let wt = prepared["worktreePath"].as_str().unwrap();
    assert_eq!(
        git(std::path::Path::new(wt), &["rev-parse", "HEAD"]).trim(),
        head
    );
    assert_eq!(
        std::fs::read_to_string(r.join("a")).unwrap(),
        "base\n",
        "the root checkout is untouched"
    );
}
