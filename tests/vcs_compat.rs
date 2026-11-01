//! Conformance: `vcs.*`, `git.runStackedAction` and `review.*` against real git repositories.
mod common;
use common::*;
use serde_json::{json, Value};
use std::path::Path;

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn seeded(e: &Env, name: &str) -> std::path::PathBuf {
    let r = e.dir(name);
    repo(&r);
    write(&r.join("a.txt"), "one\ntwo\n");
    write(&r.join("b.txt"), "bee\n");
    commit_all(&r, "init");
    r
}
/// A bare "origin" with the repo's main branch pushed and tracked.
fn with_origin(e: &Env, r: &Path) -> std::path::PathBuf {
    let bare = e.scratch.join("origin.git");
    git(
        &e.scratch,
        &["init", "-q", "--bare", "-b", "main", s(&bare)],
    );
    git(r, &["remote", "add", "origin", s(&bare)]);
    git(r, &["push", "-q", "-u", "origin", "main"]);
    bare
}

#[test]
fn init_status_refs_and_branch_operations() {
    let e = setup("vcs");
    let c = &e.conn;
    let dir = e.dir("fresh");
    // Non-repositories and missing directories are answered, not errors.
    let st = c.rpc("vcs.refreshStatus", json!({"cwd":s(&dir)})).unwrap();
    assert_eq!(
        (st["isRepo"].as_bool(), st["refName"].is_null()),
        (Some(false), true)
    );
    assert_eq!(
        c.rpc(
            "vcs.refreshStatus",
            json!({"cwd":s(&e.scratch.join("none"))})
        )
        .unwrap()["isRepo"],
        false
    );
    let refs = c.rpc("vcs.listRefs", json!({"cwd":s(&dir)})).unwrap();
    assert_eq!(
        (
            refs["isRepo"].as_bool(),
            refs["refs"].as_array().unwrap().len(),
            refs["nextCursor"].is_null()
        ),
        (Some(false), 0, true)
    );
    assert!(c
        .rpc("vcs.init", json!({"cwd":s(&e.scratch.join("none"))}))
        .is_err());
    assert!(c
        .rpc("vcs.init", json!({"cwd":s(&dir),"kind":"jj"}))
        .is_err());
    assert!(c
        .rpc("vcs.init", json!({"cwd":s(&dir),"kind":"git"}))
        .unwrap()
        .is_null());
    assert!(dir.join(".git").is_dir());
    assert_eq!(
        c.rpc("vcs.refreshStatus", json!({"cwd":s(&dir)})).unwrap()["isRepo"],
        true
    );
    // Status of a working tree: modified, staged, untracked, with line counts.
    let r = seeded(&e, "repo");
    let clean = c.rpc("vcs.refreshStatus", json!({"cwd":s(&r)})).unwrap();
    assert_eq!(
        (
            clean["refName"].as_str(),
            clean["isDefaultRef"].as_bool(),
            clean["hasWorkingTreeChanges"].as_bool()
        ),
        (Some("main"), Some(true), Some(false))
    );
    assert_eq!(
        (
            clean["hasUpstream"].as_bool(),
            clean["aheadCount"].as_u64(),
            clean["pr"].is_null()
        ),
        (Some(false), Some(0), true)
    );
    write(&r.join("a.txt"), "one\nTWO\nthree\n");
    write(&r.join("new.txt"), "x\ny\nz\n");
    git(&r, &["add", "b.txt"]);
    std::fs::remove_file(r.join("b.txt")).unwrap();
    let dirty = c.rpc("vcs.refreshStatus", json!({"cwd":s(&r)})).unwrap();
    assert_eq!(dirty["hasWorkingTreeChanges"], true);
    let files: Vec<(String, u64, u64)> = dirty["workingTree"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_owned(),
                f["insertions"].as_u64().unwrap(),
                f["deletions"].as_u64().unwrap(),
            )
        })
        .collect();
    assert!(files.contains(&("a.txt".into(), 2, 1)), "{files:?}");
    assert!(files.contains(&("new.txt".into(), 3, 0)), "{files:?}");
    assert!(files.contains(&("b.txt".into(), 0, 1)), "{files:?}");
    assert_eq!(
        (
            dirty["workingTree"]["insertions"].as_u64(),
            dirty["workingTree"]["deletions"].as_u64()
        ),
        (Some(5), Some(2))
    );
    commit_all(&r, "work");
    // Refs: create, switch, list; failures are typed and leave the repo intact.
    assert_eq!(
        c.rpc("vcs.createRef", json!({"cwd":s(&r),"refName":"feature/x"}))
            .unwrap()["refName"],
        "feature/x"
    );
    let msg = err_text(c.rpc("vcs.createRef", json!({"cwd":s(&r),"refName":"feature/x"})));
    assert!(msg.contains("already exists"), "{msg}");
    for bad in ["-rf", "a b", "x..y", "", "refs/heads/x.lock", "q~1"] {
        assert!(
            c.rpc("vcs.createRef", json!({"cwd":s(&r),"refName":bad}))
                .is_err(),
            "{bad:?}"
        );
    }
    assert_eq!(
        c.rpc(
            "vcs.createRef",
            json!({"cwd":s(&r),"refName":"topic","switchRef":true})
        )
        .unwrap()["refName"],
        "topic"
    );
    assert_eq!(git(&r, &["branch", "--show-current"]).trim(), "topic");
    assert_eq!(
        c.rpc("vcs.switchRef", json!({"cwd":s(&r),"refName":"main"}))
            .unwrap()["refName"],
        "main"
    );
    assert!(err_text(c.rpc(
        "vcs.switchRef",
        json!({"cwd":s(&r),"refName":"does-not-exist"})
    ))
    .contains("not found"));
    let listed = c.rpc("vcs.listRefs", json!({"cwd":s(&r)})).unwrap();
    let names: Vec<&str> = listed["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(names[0], "main", "the current ref comes first");
    assert!(names.contains(&"feature/x") && names.contains(&"topic"));
    assert_eq!(
        (
            listed["refs"][0]["current"].as_bool(),
            listed["refs"][0]["isDefault"].as_bool(),
            listed["totalCount"].as_u64()
        ),
        (Some(true), Some(true), Some(3))
    );
    assert_eq!(
        listed["refs"][0]["worktreePath"]
            .as_str()
            .map(|p| std::fs::canonicalize(p).unwrap()),
        Some(std::fs::canonicalize(&r).unwrap())
    );
    let page = c
        .rpc("vcs.listRefs", json!({"cwd":s(&r),"limit":2}))
        .unwrap();
    assert_eq!(
        (
            page["refs"].as_array().unwrap().len(),
            page["nextCursor"].as_u64()
        ),
        (2, Some(2))
    );
    let rest = c
        .rpc("vcs.listRefs", json!({"cwd":s(&r),"limit":2,"cursor":2}))
        .unwrap();
    assert_eq!(
        (
            rest["refs"].as_array().unwrap().len(),
            rest["nextCursor"].is_null()
        ),
        (1, true)
    );
    let q = c
        .rpc("vcs.listRefs", json!({"cwd":s(&r),"query":"TOP"}))
        .unwrap();
    assert_eq!(q["refs"].as_array().unwrap().len(), 1);
    assert_eq!(
        c.rpc("vcs.listRefs", json!({"cwd":s(&r),"refKind":"remote"}))
            .unwrap()["refs"],
        json!([])
    );
    // Concurrent mutations on one repository are serialized: none trips over index.lock.
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let conn = {
                let grant = e
                    .conn
                    .request_json("POST", "/api/auth/pairing-token", Some(&json!({})), false)
                    .unwrap();
                t3_protocol::pair_bearer(
                    &e.base,
                    grant["credential"].as_str().unwrap(),
                    None,
                    &t3_protocol::PairingClient::default(),
                    &t3_protocol::ConnectionOptions::default(),
                )
                .unwrap()
                .connection(&t3_protocol::ConnectionOptions::default())
                .unwrap()
            };
            let cwd = s(&r).to_owned();
            std::thread::spawn(move || {
                conn.rpc(
                    "vcs.createRef",
                    json!({"cwd":cwd,"refName":format!("par-{i}"),"switchRef":true}),
                )
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
    assert_eq!(
        c.rpc(
            "vcs.listRefs",
            json!({"cwd":s(&r),"query":"par-","limit":50})
        )
        .unwrap()["totalCount"],
        6
    );
}

#[test]
fn pull_reports_state_and_branch_changes_against_the_default_branch() {
    let e = setup("pull");
    let c = &e.conn;
    let r = seeded(&e, "repo");
    // Without an upstream there is nothing to pull.
    assert!(err_text(c.rpc("vcs.pull", json!({"cwd":s(&r)}))).contains("no upstream"));
    let bare = with_origin(&e, &r);
    let up = c.rpc("vcs.pull", json!({"cwd":s(&r)})).unwrap();
    assert_eq!(
        (
            up["status"].as_str(),
            up["refName"].as_str(),
            up["upstreamRef"].as_str()
        ),
        (
            Some("skipped_up_to_date"),
            Some("main"),
            Some("origin/main")
        )
    );
    // A second clone pushes; pulling fast-forwards.
    let other = e.scratch.join("other");
    git(&e.scratch, &["clone", "-q", s(&bare), s(&other)]);
    write(&other.join("c.txt"), "from other\n");
    commit_all(&other, "other");
    git(&other, &["push", "-q", "origin", "main"]);
    git(&r, &["fetch", "-q"]);
    let behind = c.rpc("vcs.refreshStatus", json!({"cwd":s(&r)})).unwrap();
    assert_eq!(
        (
            behind["hasUpstream"].as_bool(),
            behind["behindCount"].as_u64(),
            behind["aheadCount"].as_u64(),
            behind["hasPrimaryRemote"].as_bool()
        ),
        (Some(true), Some(1), Some(0), Some(true))
    );
    assert_eq!(
        c.rpc("vcs.pull", json!({"cwd":s(&r)})).unwrap()["status"],
        "pulled"
    );
    assert!(r.join("c.txt").exists());
    // Divergent history is not merged behind the user's back.
    write(&r.join("d.txt"), "local\n");
    commit_all(&r, "local");
    write(&other.join("e.txt"), "remote\n");
    commit_all(&other, "remote2");
    git(&other, &["push", "-q", "origin", "main"]);
    let msg = err_text(c.rpc("vcs.pull", json!({"cwd":s(&r)})));
    assert!(!msg.is_empty());
    assert!(!r.join("e.txt").exists());
    // On a feature branch the status carries the totals against the default branch, and refs list remotes.
    git(&r, &["reset", "-q", "--hard", "origin/main"]);
    c.rpc(
        "vcs.createRef",
        json!({"cwd":s(&r),"refName":"feat","switchRef":true}),
    )
    .unwrap();
    write(&r.join("f.txt"), "1\n2\n");
    commit_all(&r, "feat work");
    let st = c.rpc("vcs.refreshStatus", json!({"cwd":s(&r)})).unwrap();
    assert_eq!(
        (
            st["refName"].as_str(),
            st["isDefaultRef"].as_bool(),
            st["branchChanges"]["baseRef"].as_str(),
            st["branchChanges"]["insertions"].as_u64(),
            st["aheadOfDefaultCount"].as_u64()
        ),
        (Some("feat"), Some(false), Some("main"), Some(2), Some(1))
    );
    let remote_refs = c
        .rpc(
            "vcs.listRefs",
            json!({"cwd":s(&r),"refKind":"remote","includeMatchingRemoteRefs":true}),
        )
        .unwrap();
    assert_eq!(remote_refs["refs"][0]["remoteName"], "origin");
    // Switching to a remote-only branch creates a tracking branch.
    git(&other, &["switch", "-q", "-c", "remote-only"]);
    git(&other, &["push", "-q", "origin", "remote-only"]);
    git(&r, &["fetch", "-q"]);
    assert_eq!(
        c.rpc(
            "vcs.switchRef",
            json!({"cwd":s(&r),"refName":"origin/remote-only"})
        )
        .unwrap()["refName"],
        "remote-only"
    );
}

fn run_action(e: &Env, p: Value) -> Vec<Value> {
    collect(&e.conn, "git.runStackedAction", p, |ev| {
        matches!(
            ev["kind"].as_str(),
            Some("action_finished" | "action_failed")
        )
    })
}
fn kinds(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn stacked_action_streams_commit_push_and_pull_request_progress() {
    let e = setup("stacked");
    let r = seeded(&e, "repo");
    let bare = with_origin(&e, &r);
    let cwd = s(&r);
    // commit
    write(&r.join("a.txt"), "changed\n");
    write(&r.join("only.txt"), "only\n");
    let ev = run_action(
        &e,
        json!({"actionId":"act-1","cwd":cwd,"action":"commit","commitMessage":"Change a\n\nbody"}),
    );
    assert_eq!(
        kinds(&ev),
        ["action_started", "phase_started", "action_finished"]
    );
    assert_eq!(
        (
            ev[0]["actionId"].as_str(),
            ev[0]["action"].as_str(),
            ev[0]["phases"].clone()
        ),
        (Some("act-1"), Some("commit"), json!(["commit"]))
    );
    let res = &ev[2]["result"];
    assert_eq!(
        (
            res["commit"]["status"].as_str(),
            res["commit"]["subject"].as_str()
        ),
        (Some("created"), Some("Change a"))
    );
    assert_eq!(
        res["commit"]["commitSha"].as_str().unwrap(),
        git(&r, &["rev-parse", "HEAD"]).trim()
    );
    assert_eq!(
        (
            res["branch"]["status"].as_str(),
            res["push"]["status"].as_str(),
            res["pr"]["status"].as_str()
        ),
        (
            Some("skipped_not_requested"),
            Some("skipped_not_requested"),
            Some("skipped_not_requested")
        )
    );
    assert_eq!(res["toast"]["cta"]["kind"], "run_action");
    assert!(git(&r, &["status", "--porcelain"]).is_empty());
    // Nothing to commit is reported, not failed.
    let ev = run_action(&e, json!({"actionId":"act-2","cwd":cwd,"action":"commit"}));
    assert_eq!(
        ev.last().unwrap()["result"]["commit"]["status"],
        "skipped_no_changes"
    );
    // commit_push with only some files; the rest stay uncommitted.
    write(&r.join("p1.txt"), "1\n");
    write(&r.join("p2.txt"), "2\n");
    let ev = run_action(
        &e,
        json!({"actionId":"act-3","cwd":cwd,"action":"commit_push","commitMessage":"just p1","filePaths":["p1.txt"]}),
    );
    assert_eq!(
        kinds(&ev),
        [
            "action_started",
            "phase_started",
            "phase_started",
            "action_finished"
        ]
    );
    assert_eq!(ev[0]["phases"], json!(["commit", "push"]));
    assert_eq!(
        (
            ev[3]["result"]["push"]["status"].as_str(),
            ev[3]["result"]["push"]["upstreamBranch"].as_str()
        ),
        (Some("pushed"), Some("origin/main"))
    );
    assert_eq!(git(&r, &["status", "--porcelain"]).trim(), "?? p2.txt");
    assert_eq!(
        git(&bare, &["log", "--format=%s", "-1", "main"]).trim(),
        "just p1"
    );
    // Pushing again with nothing new is up to date.
    let ev = run_action(&e, json!({"actionId":"act-4","cwd":cwd,"action":"push"}));
    assert_eq!(
        ev.last().unwrap()["result"]["push"]["status"],
        "skipped_up_to_date"
    );
    // A feature branch gets its upstream set on first push; gh creates the PR.
    let ev = run_action(
        &e,
        json!({"actionId":"act-5","cwd":cwd,"action":"commit_push_pr","featureBranch":true,"commitMessage":"Add p2 thing"}),
    );
    assert_eq!(ev[0]["phases"], json!(["branch", "commit", "push", "pr"]));
    assert_eq!(kinds(&ev).last().unwrap(), "action_finished", "{ev:?}");
    let res = &ev.last().unwrap()["result"];
    let branch = res["branch"]["name"].as_str().unwrap();
    assert!(branch.starts_with("feature/add-p2-thing-"), "{branch}");
    assert_eq!(
        (
            res["push"]["setUpstream"].as_bool(),
            res["pr"]["status"].as_str(),
            res["pr"]["url"].as_str(),
            res["pr"]["number"].as_u64()
        ),
        (
            Some(true),
            Some("created"),
            Some("https://github.com/o/r/pull/7"),
            Some(7)
        )
    );
    assert_eq!(
        (
            res["toast"]["cta"]["kind"].as_str(),
            res["toast"]["cta"]["url"].as_str()
        ),
        (Some("open_pr"), Some("https://github.com/o/r/pull/7"))
    );
    assert_eq!(git(&r, &["branch", "--show-current"]).trim(), branch);
    assert!(git(&bare, &["branch", "--list", branch]).contains(branch));
    let gh = std::fs::read_to_string(bin_dir().join("gh.log")).unwrap();
    assert!(
        gh.contains("pr create") && gh.contains("--head ") && gh.contains("--base main"),
        "{gh}"
    );
    // Failure: the stream says which phase failed and ends with exactly one terminal event.
    let lonely = seeded(&e, "lonely");
    write(&lonely.join("n.txt"), "n\n");
    let ev = run_action(
        &e,
        json!({"actionId":"act-6","cwd":s(&lonely),"action":"commit_push","commitMessage":"no remote"}),
    );
    assert_eq!(
        kinds(&ev),
        [
            "action_started",
            "phase_started",
            "phase_started",
            "action_failed"
        ]
    );
    assert_eq!(
        (
            ev[3]["phase"].as_str(),
            ev[3]["message"].as_str().unwrap().contains("remote")
        ),
        (Some("push"), true)
    );
    assert_eq!(
        git(&lonely, &["log", "--format=%s", "-1"]).trim(),
        "no remote",
        "work done before the failure is kept"
    );
    // Invalid requests are refused before a stream starts.
    for bad in [
        json!({"actionId":"x","cwd":cwd,"action":"nope"}),
        json!({"actionId":"x","cwd":cwd,"action":"commit","commitMessage":"x".repeat(10_001)}),
        json!({"actionId":"x","cwd":cwd,"action":"commit","filePaths":[]}),
        json!({"actionId":"","cwd":cwd,"action":"commit"}),
    ] {
        assert!(collect(&e.conn, "git.runStackedAction", bad, |_| true).is_empty());
    }
    // File paths cannot reach outside the repository.
    write(&r.join("q.txt"), "q\n");
    let ev = run_action(
        &e,
        json!({"actionId":"act-7","cwd":cwd,"action":"commit","commitMessage":"escape","filePaths":["../outside.txt"]}),
    );
    assert_eq!(kinds(&ev).last().unwrap(), "action_failed");
    assert!(ev.last().unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("outside the repository"));
    // Source-control writes need their scope.
    let grant = e
        .conn
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let ro = t3_protocol::pair_bearer(
        &e.base,
        grant["credential"].as_str().unwrap(),
        None,
        &t3_protocol::PairingClient::default(),
        &t3_protocol::ConnectionOptions::default(),
    )
    .unwrap()
    .connection(&t3_protocol::ConnectionOptions::default())
    .unwrap();
    assert!(
        err_text(ro.rpc("vcs.createRef", json!({"cwd":cwd,"refName":"nope"})))
            .contains("source-control:write")
    );
    assert!(ro.rpc("vcs.listRefs", json!({"cwd":cwd})).is_ok());
}

#[test]
fn git_commands_are_argv_only_and_hostile_names_are_inert() {
    let e = setup("argv");
    let r = seeded(&e, "repo");
    let canary = e.scratch.join("pwned");
    for name in [
        format!("x;touch {}", s(&canary)),
        format!("$(touch {})", s(&canary)),
        "`touch pwned`".to_owned(),
        "a\nb".to_owned(),
    ] {
        let _ = e
            .conn
            .rpc("vcs.createRef", json!({"cwd":s(&r),"refName":name}));
        let _ = e
            .conn
            .rpc("vcs.switchRef", json!({"cwd":s(&r),"refName":name}));
        assert!(!canary.exists());
    }
    // A cwd that is not a directory, or carries odd characters, is a plain failure.
    assert!(e
        .conn
        .rpc("vcs.pull", json!({"cwd":"relative/dir"}))
        .is_err());
    let weird = e.dir("weird dir; echo hi");
    repo(&weird);
    assert_eq!(
        e.conn
            .rpc("vcs.refreshStatus", json!({"cwd":s(&weird)}))
            .unwrap()["isRepo"],
        true
    );
    // Repository-local config cannot run programs during read-only inspection.
    git(
        &r,
        &["config", "core.fsmonitor", &format!("touch {}", s(&canary))],
    );
    e.conn
        .rpc("vcs.refreshStatus", json!({"cwd":s(&r)}))
        .unwrap();
    assert!(!canary.exists());
}

#[test]
fn review_diffs_include_untracked_files_and_stay_in_known_workspaces() {
    let e = setup("review");
    let r = seeded(&e, "repo");
    e.project("p1", &r);
    git(&r, &["switch", "-q", "-c", "work"]);
    write(&r.join("a.txt"), "one\ntwo\nthree\n");
    commit_all(&r, "committed on work");
    write(&r.join("b.txt"), "bee\nbuzz\n");
    write(&r.join("new.txt"), "fresh\n");
    let prev = e
        .conn
        .rpc("review.getDiffPreview", json!({"cwd":s(&r)}))
        .unwrap();
    let sources = prev["sources"].as_array().unwrap();
    let ids: Vec<&str> = sources.iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["working-tree", "branch-range"]);
    let wt = &sources[0];
    let paths: Vec<&str> = wt["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"b.txt") && paths.contains(&"new.txt"),
        "{paths:?}"
    );
    assert!(
        wt["diff"].as_str().unwrap().contains("+fresh")
            && wt["diff"].as_str().unwrap().contains("--- a/b.txt")
    );
    assert_eq!(
        (
            wt["title"].as_str(),
            wt["baseRef"].as_str(),
            wt["truncated"].as_bool()
        ),
        (Some("Uncommitted"), Some("HEAD"), Some(false))
    );
    let br = &sources[1];
    assert_eq!(
        (
            br["title"].as_str(),
            br["baseRef"].as_str(),
            br["headRef"].as_str()
        ),
        (Some("Changes vs main"), Some("main"), Some("work"))
    );
    assert!(br["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "a.txt" && f["additions"] == 1));
    assert!(
        br["diff"].as_str().unwrap().contains("+three")
            && br["diff"].as_str().unwrap().contains("+fresh")
    );
    assert_eq!(wt["diffHash"].as_str().unwrap().len(), 64);
    // The real index was not touched by the untracked-file handling.
    assert!(git(&r, &["status", "--porcelain"]).contains("?? new.txt"));
    // Single-file requests and whitespace handling.
    let one = e.conn.rpc("review.getDiffPreview", json!({"cwd":s(&r),"file":{"path":"b.txt","previousPath":null,"sourceKind":"working-tree"}})).unwrap();
    assert_eq!(one["sources"][0]["files"].as_array().unwrap().len(), 1);
    write(&r.join("ws.txt"), "a  b\n");
    commit_all(&r, "ws");
    write(&r.join("ws.txt"), "a b\n");
    let strict = e
        .conn
        .rpc("review.getDiffPreview", json!({"cwd":s(&r)}))
        .unwrap();
    let loose = e
        .conn
        .rpc(
            "review.getDiffPreview",
            json!({"cwd":s(&r),"ignoreWhitespace":true}),
        )
        .unwrap();
    assert!(strict["sources"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("ws.txt"));
    assert!(!loose["sources"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("ws.txt"));
    // Large patches are cut at the limit and flagged.
    write(&r.join("huge.txt"), "line of text\n".repeat(20_000));
    let huge = e
        .conn
        .rpc("review.getDiffPreview", json!({"cwd":s(&r)}))
        .unwrap();
    assert_eq!(huge["sources"][0]["truncated"], true);
    assert!(huge["sources"][0]["diff"].as_str().unwrap().len() < 130_000);
    // File contents for expanding unchanged regions.
    write(&r.join("b.txt"), "bee\nbuzz\nmore\n");
    let fc = e.conn.rpc("review.getDiffFileContents", json!({"cwd":s(&r),"sourceKind":"working-tree","changeType":"change","baseRef":null,"headRef":null,"oldPath":"b.txt","newPath":"b.txt"})).unwrap();
    assert_eq!(
        (fc["oldContents"].as_str(), fc["newContents"].as_str()),
        (Some("bee\nbuzz\n"), Some("bee\nbuzz\nmore\n"))
    );
    let fresh = e.conn.rpc("review.getDiffFileContents", json!({"cwd":s(&r),"sourceKind":"working-tree","changeType":"new","baseRef":null,"headRef":null,"oldPath":"new.txt","newPath":"new.txt"})).unwrap();
    assert_eq!(
        (fresh["oldContents"].as_str(), fresh["newContents"].as_str()),
        (Some(""), Some("fresh\n"))
    );
    let range = e.conn.rpc("review.getDiffFileContents", json!({"cwd":s(&r),"sourceKind":"branch-range","changeType":"change","baseRef":"main","headRef":null,"oldPath":"a.txt","newPath":"a.txt"})).unwrap();
    assert_eq!(range["oldContents"], "one\ntwo\n");
    // Escapes: paths outside the repo, binary files, and repos nobody registered.
    let secret = e.scratch.join("secret.txt");
    write(&secret, "SECRET");
    std::os::unix::fs::symlink(&secret, r.join("link.txt")).unwrap();
    for bad in ["../secret.txt", "link.txt", "/etc/hostname"] {
        let m = err_text(e.conn.rpc("review.getDiffFileContents", json!({"cwd":s(&r),"sourceKind":"working-tree","changeType":"new","baseRef":null,"headRef":null,"oldPath":bad,"newPath":bad})));
        assert!(
            m.contains("outside the review workspace") || m.contains("Could not"),
            "{bad}: {m}"
        );
    }
    write(&r.join("bin.dat"), [0u8, 1, 2]);
    assert!(err_text(e.conn.rpc("review.getDiffFileContents", json!({"cwd":s(&r),"sourceKind":"working-tree","changeType":"new","baseRef":null,"headRef":null,"oldPath":"bin.dat","newPath":"bin.dat"}))).contains("binary"));
    let stranger = seeded(&e, "stranger");
    for method in ["review.getDiffPreview", "review.getDiffFileContents"] {
        let m = err_text(e.conn.rpc(method, json!({"cwd":s(&stranger),"sourceKind":"working-tree","changeType":"change","baseRef":null,"headRef":null,"oldPath":"a.txt","newPath":"a.txt"})));
        assert!(m.contains("configured workspace root"), "{method}: {m}");
    }
    assert!(e
        .conn
        .rpc("review.getDiffPreview", json!({"cwd":"/"}))
        .is_err());
    // A non-repository inside a known workspace has no sources.
    let empty_root = e.dir("emptyroot");
    e.project("p2", &empty_root);
    assert_eq!(
        e.conn
            .rpc("review.getDiffPreview", json!({"cwd":s(&empty_root)}))
            .unwrap()["sources"],
        json!([])
    );
}

#[test]
fn z_status_reports_host_and_pull_request_and_local_only_skips_host() {
    let e = setup("host-status");
    let root = e.dir("repo");
    repo(&root);
    write(&root.join("a"), "a\n");
    commit_all(&root, "initial");
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    let script = bin_dir().join("gh");
    let previous = std::fs::read(&script).unwrap();
    std::fs::write(&script,b"#!/bin/sh\necho '[{\"number\":7,\"title\":\"Proof\",\"url\":\"https://github.com/owner/repo/pull/7\",\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"state\":\"OPEN\",\"isDraft\":false,\"updatedAt\":null}]'\n").unwrap();
    let r = e
        .conn
        .rpc("vcs.refreshStatus", json!({"cwd":root}))
        .unwrap();
    std::fs::write(&script, previous).unwrap();
    assert_eq!(r["sourceControlProvider"]["kind"], "github");
    assert_eq!(r["pr"]["number"], 7);
    assert_eq!(r["pr"]["state"], "open");
    let r = e
        .conn
        .rpc("vcs.refreshStatus", json!({"cwd":root,"localOnly":true}))
        .unwrap();
    assert!(r["pr"].is_null());
    assert_eq!(r["sourceControlProvider"]["name"], "GitHub");
}
