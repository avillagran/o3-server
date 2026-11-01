//! Conformance: thread search (FTS5), archived shell, turn items, projections and checkpoint diffs.
mod common;
use common::*;
use rusqlite::params;
use serde_json::json;
use std::path::Path;

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn archive(e: &Env, t: &str) {
    e.conn
        .rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.archive","commandId":format!("arch-{t}"),"threadId":t}),
        )
        .unwrap();
}

#[test]
fn search_threads_uses_fts_and_respects_thread_state() {
    let e = setup("search");
    let root = e.dir("p");
    e.project("p1", &root);
    e.project("p2", &e.dir("q"));
    for t in ["t1", "t2", "t3"] {
        e.thread(t, if t == "t3" { "p2" } else { "p1" });
    }
    e.message(
        "t1",
        "m1",
        "How do I configure the quokka scheduler for nightly runs?",
    );
    e.message("t1", "m2", "The quokka also needs a retry policy");
    e.message("t2", "m3", "Unrelated talk about gardening");
    e.message("t3", "m4", "Quokka in the other project");
    let search = |p: serde_json::Value| e.conn.rpc("orchestration.searchThreads", p).unwrap();
    let r = search(json!({"query":"quokka"}));
    let hits: Vec<(&str, i64)> = r["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["threadId"].as_str().unwrap(), m["hits"].as_i64().unwrap()))
        .collect();
    assert_eq!(hits.len(), 2, "{r}");
    assert!(
        hits.contains(&("t1", 2)) && hits.contains(&("t3", 1)),
        "{hits:?}"
    );
    let m = r["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["threadId"] == "t1")
        .unwrap();
    assert_eq!(
        (m["source"].as_str(), m["projectId"].as_str()),
        (Some("user"), Some("p1"))
    );
    assert!(
        m["snippet"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("quokka")
            && m["snippet"].as_str().unwrap().chars().count() <= 240
    );
    assert!(
        m["messageId"].is_string() && m["messageCreatedAt"].is_string() && m["quality"].is_number()
    );
    assert_eq!(
        search(json!({"query":"quok"}))["matches"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "prefix match"
    );
    assert_eq!(
        search(json!({"query":"quokka","limit":1}))["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        search(json!({"query":"quokka","projectIds":["p2"]}))["matches"][0]["threadId"],
        "t3"
    );
    // FTS syntax in user text is inert.
    for q in [
        "\"unbalanced",
        "quokka OR",
        "NEAR(",
        "a* b*",
        "col:val",
        "--",
        "'; DROP TABLE records;--",
    ] {
        assert!(
            e.conn
                .rpc("orchestration.searchThreads", json!({"query":q}))
                .is_ok(),
            "{q}"
        );
    }
    assert!(
        e.conn
            .rpc("orchestration.searchThreads", json!({"query":"a"}))
            .is_err(),
        "queries need two characters"
    );
    // Titles.
    let titles = search(json!({"query":"thread t2","titlesOnly":true}));
    assert_eq!(titles["matches"][0]["threadId"], "t2");
    // Renamed threads are found by title; archived and deleted threads drop out.
    e.conn.rpc("orchestration.dispatchCommand", json!({"type":"thread.metadata.update","commandId":"ren","threadId":"t2","title":"Zebra planning"})).unwrap();
    assert_eq!(
        search(json!({"query":"zebra","titlesOnly":true}))["matches"][0]["threadId"],
        "t2"
    );
    archive(&e, "t1");
    assert_eq!(
        search(json!({"query":"quokka"}))["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    e.conn
        .rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.delete","commandId":"del","threadId":"t3"}),
        )
        .unwrap();
    assert_eq!(search(json!({"query":"quokka"}))["matches"], json!([]));
    // The index is rebuilt from the stored messages when it is missing (an older profile, a restart).
    e.db().execute("DROP TABLE thread_search", []).unwrap();
    let reopened = o3_server::store::State::open(&e.state).unwrap();
    let rebuilt =
        o3_server::threadq::search_threads(&reopened, &json!({"query":"gardening"})).unwrap();
    assert_eq!(rebuilt["matches"][0]["threadId"], "t2");
}

#[test]
fn archived_shell_turn_items_and_projection() {
    let e = setup("reads");
    e.project("p1", &e.dir("p"));
    e.thread("t1", "p1");
    e.thread("t2", "p1");
    e.message("t1", "m1", "hello projection");
    archive(&e, "t2");
    let a = e
        .conn
        .rpc("orchestration.getArchivedShellSnapshot", json!({}))
        .unwrap();
    assert_eq!(
        a["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["t2"]
    );
    assert_eq!(a["projects"][0]["id"], "p1");
    assert!(a["snapshotSequence"].as_i64().unwrap() > 0 && a["schemaVersion"].is_number());
    let live = e
        .conn
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(
        live["threads"].as_array().unwrap().len(),
        1,
        "archived threads are not in the active shell"
    );
    // Turn items.
    let item = e
        .conn
        .rpc(
            "orchestration.getTurnItem",
            json!({"threadId":"t1","itemId":"item-m1"}),
        )
        .unwrap();
    assert_eq!(
        (item["item"]["type"].as_str(), item["item"]["text"].as_str()),
        (Some("user_message"), Some("hello projection"))
    );
    assert!(e
        .conn
        .rpc(
            "orchestration.getTurnItem",
            json!({"threadId":"t1","itemId":"nope"})
        )
        .unwrap()["item"]
        .is_null());
    assert!(
        e.conn
            .rpc(
                "orchestration.getTurnItem",
                json!({"threadId":"t2","itemId":"item-m1"})
            )
            .unwrap()["item"]
            .is_null(),
        "an item is only visible through its own thread"
    );
    // Thread projection.
    let p = e
        .conn
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"t1"}),
        )
        .unwrap();
    assert_eq!(p["thread"]["id"], "t1");
    assert_eq!(p["messages"][0]["text"], "hello projection");
    assert!(p["visibleTurnItems"].is_array() && p["checkpoints"].is_array());
    assert!(err_text(e.conn.rpc(
        "orchestration.getThreadProjection",
        json!({"threadId":"missing"})
    ))
    .contains("missing"));
    // Read scope is enough for all of them.
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
    assert!(ro
        .rpc("orchestration.getArchivedShellSnapshot", json!({}))
        .is_ok());
    assert!(ro
        .rpc("orchestration.searchThreads", json!({"query":"hello"}))
        .is_ok());
}

fn seed(db: &rusqlite::Connection, kind: &str, id: &str, tid: &str, body: serde_json::Value) {
    db.execute(
        "INSERT OR REPLACE INTO records VALUES(?,?,?,?)",
        params![kind, id, tid, body.to_string()],
    )
    .unwrap();
}

#[test]
fn checkpoint_diffs_come_from_recorded_refs() {
    let e = setup("diffs");
    let r = e.dir("repo");
    repo(&r);
    write(&r.join("f.txt"), "base\n");
    commit_all(&r, "base");
    git(&r, &["update-ref", "refs/t3/checkpoints/t1/turn/0", "HEAD"]);
    write(&r.join("f.txt"), "base\nturn one\n");
    commit_all(&r, "t1");
    git(&r, &["update-ref", "refs/t3/checkpoints/t1/turn/1", "HEAD"]);
    write(&r.join("g.txt"), "turn two\n");
    write(&r.join("f.txt"), "base\nturn one\n  spaced\n");
    commit_all(&r, "t2");
    git(&r, &["update-ref", "refs/t3/checkpoints/t1/turn/2", "HEAD"]);
    e.project("p1", &r);
    e.thread("t1", "p1");
    let db = e.db();
    for (id, st) in [
        ("run1", "completed"),
        ("run2", "completed"),
        ("run3", "running"),
    ] {
        seed(&db, "runs", id, "t1", json!({"id":id,"status":st}));
    }
    seed(
        &db,
        "checkpointScopes",
        "scope1",
        "t1",
        json!({"id":"scope1","kind":"root_run","cwd":s(&r)}),
    );
    let cp = |id: &str,
              ordinal: u64,
              app: Option<u64>,
              run: Option<&str>,
              git_ref: &str,
              status: &str| {
        seed(
            &db,
            "checkpoints",
            id,
            "t1",
            json!({"id":id,"scopeId":"scope1","ordinalWithinScope":ordinal,"appRunOrdinal":app,"runId":run,"ref":git_ref,"status":status}),
        );
    };
    cp(
        "cp0",
        0,
        None,
        None,
        "refs/t3/checkpoints/t1/turn/0",
        "ready",
    );
    cp(
        "cp1",
        1,
        Some(1),
        Some("run1"),
        "refs/t3/checkpoints/t1/turn/1",
        "ready",
    );
    cp(
        "cp2",
        2,
        Some(2),
        Some("run2"),
        "refs/t3/checkpoints/t1/turn/2",
        "ready",
    );
    cp(
        "cp3",
        3,
        Some(3),
        Some("run3"),
        "refs/t3/checkpoints/t1/turn/2",
        "ready",
    );
    let c = &e.conn;
    let one = c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":0,"toTurnCount":1}),
        )
        .unwrap();
    assert_eq!(
        (one["fromTurnCount"].as_u64(), one["toTurnCount"].as_u64()),
        (Some(0), Some(1))
    );
    let d = one["diff"].as_str().unwrap();
    assert!(
        d.contains("diff --git a/f.txt b/f.txt")
            && d.contains("+turn one")
            && !d.contains("turn two"),
        "{d}"
    );
    let two = c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":1,"toTurnCount":2}),
        )
        .unwrap()["diff"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        two.contains("+turn two") && two.contains("+  spaced"),
        "{two}"
    );
    let ws = c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":1,"toTurnCount":2,"ignoreWhitespace":false}),
        )
        .unwrap()["diff"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(ws.contains("+  spaced"));
    let full = c
        .rpc(
            "orchestration.getFullThreadDiff",
            json!({"threadId":"t1","toTurnCount":2}),
        )
        .unwrap();
    let d = full["diff"].as_str().unwrap();
    assert!(
        d.contains("+turn one") && d.contains("+turn two") && full["fromTurnCount"] == 0,
        "{d}"
    );
    assert_eq!(
        c.rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":2,"toTurnCount":2})
        )
        .unwrap()["diff"],
        ""
    );
    assert_eq!(
        c.rpc(
            "orchestration.getFullThreadDiff",
            json!({"threadId":"t1","toTurnCount":0})
        )
        .unwrap()["diff"],
        ""
    );
    // Unavailable ranges are errors, not empty diffs: turn 3 belongs to a run that has not completed.
    assert!(c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":0,"toTurnCount":3})
        )
        .is_err());
    assert!(c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":2,"toTurnCount":1})
        )
        .is_err());
    assert!(c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"nope","fromTurnCount":0,"toTurnCount":1})
        )
        .is_err());
    assert!(c
        .rpc(
            "orchestration.getFullThreadDiff",
            json!({"threadId":"t1","toTurnCount":9})
        )
        .is_err());
    // A recorded ref that is not a ref (an option, a path) never reaches git.
    cp(
        "cp1",
        1,
        Some(1),
        Some("run1"),
        "--output=/var/tmp/o3-build/pwned",
        "ready",
    );
    assert!(c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":0,"toTurnCount":1})
        )
        .is_err());
    assert!(!Path::new("/var/tmp/o3-build/pwned").exists());
    // Checkpoints whose workspace is not a known one are not diffed.
    cp(
        "cp1",
        1,
        Some(1),
        Some("run1"),
        "refs/t3/checkpoints/t1/turn/1",
        "ready",
    );
    seed(
        &db,
        "checkpointScopes",
        "scope1",
        "t1",
        json!({"id":"scope1","kind":"root_run","cwd":s(&e.dir("elsewhere"))}),
    );
    assert!(c
        .rpc(
            "orchestration.getTurnDiff",
            json!({"threadId":"t1","fromTurnCount":0,"toTurnCount":1})
        )
        .is_err());
}

#[test]
fn project_identity_refresh_replaces_shell_without_losing_threads() {
    let e = setup("repository-identity");
    let root = e.dir("repo");
    repo(&root);
    git(
        &root,
        &["remote", "add", "origin", "git@github.com:Owner/Repo.git"],
    );
    let p = e.project("p", &root);
    assert_eq!(
        p["repositoryIdentity"]["canonicalKey"],
        "github.com/owner/repo"
    );
    e.thread("t", "p");
    let seq = e
        .conn
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap()["snapshotSequence"]
        .clone();
    git(
        &root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://gitlab.com/group/new.git",
        ],
    );
    let state = o3_server::store::State::open(&e.state).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(o3_server::projects::refresh_identities(&state))
        .unwrap();
    let items = collect(
        &e.conn,
        "orchestration.subscribeShell",
        json!({"afterSequence":seq}),
        |v| v["resolvedRepositoryIdentityRoots"].is_array(),
    );
    let replacement = items.last().unwrap();
    assert_eq!(
        replacement["snapshot"]["projects"][0]["repositoryIdentity"]["canonicalKey"],
        "gitlab.com/group/new"
    );
    assert_eq!(replacement["snapshot"]["threads"][0]["id"], "t");
    assert_eq!(
        replacement["resolvedRepositoryIdentityRoots"][0],
        root.to_str().unwrap()
    );
}

#[test]
fn forced_project_delete_removes_thread_shells_atomically() {
    let e = setup("force-project-delete");
    e.project("p", &e.dir("repo"));
    e.thread("t", "p");
    e.conn
        .rpc(
            "projects.mutate",
            json!({"type":"project.delete","commandId":"delete","projectId":"p","force":true}),
        )
        .unwrap();
    let shell = e
        .conn
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert!(shell["projects"].as_array().unwrap().is_empty());
    assert!(shell["threads"].as_array().unwrap().is_empty());
}
