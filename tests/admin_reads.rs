//! Small admin reads, project helpers and the typed "not available here" answers. One test: it sets
//! process environment (git identity, CLAUDE_CONFIG_DIR) before any server thread exists.
mod common;
mod support;
use common::{commit_all, git, repo, write};
use serde_json::{json, Value};
use std::{io::Write, time::Duration};
use support::{raw_exit, start, subscribe};

fn status_server(codes: &'static [(&'static str, u16)]) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = [0u8; 4096];
            let n = std::io::Read::read(&mut s, &mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let code = codes
                .iter()
                .find(|(p, _)| head.contains(&format!("POST {p} ")))
                .map(|c| c.1)
                .unwrap_or(404);
            let _ = write!(
                s,
                "HTTP/1.1 {code} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
        }
    });
    base
}

fn error_of(frame: &Value) -> &Value {
    &frame["exit"]["cause"][0]["error"]
}

#[test]
fn admin_reads_project_helpers_and_unavailable_surfaces() {
    let claude = std::env::temp_dir().join(format!("o3-admin-claude-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&claude);
    // SAFETY: before any server thread of this process exists; this file has one test.
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        for (k, v) in [
            ("AUTHOR_NAME", "Test"),
            ("AUTHOR_EMAIL", "t@example.invalid"),
            ("COMMITTER_NAME", "Test"),
            ("COMMITTER_EMAIL", "t@example.invalid"),
        ] {
            std::env::set_var(format!("GIT_{k}"), v);
        }
    }
    let server = start(|_| {});
    // Named projects live in <base>/projects, and this harness's base is the shared temp dir.
    let _ = std::fs::remove_dir_all(server.dir.parent().unwrap().join("projects"));
    let c = server.connect();
    let scratch = server.dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    // Host resources come from /proc.
    let host = c.rpc("server.getHostResources", json!({})).unwrap();
    assert!(
        host["totalMemoryBytes"].as_u64().unwrap() > 0 && host["cpuCount"].as_u64().unwrap() >= 1
    );
    assert!(
        host["availableMemoryBytes"].as_u64().unwrap()
            <= host["totalMemoryBytes"].as_u64().unwrap()
    );
    assert!(
        host["cpuUtilization"].is_null()
            || (0.0..=1.0).contains(&host["cpuUtilization"].as_f64().unwrap())
    );

    // OTLP check: 2xx accepted, other statuses rejected, nothing listening unreachable.
    let base = status_server(&[("/ok", 200), ("/bad", 503)]);
    let ok = c
        .rpc(
            "server.checkOtlpEndpoint",
            json!({"signal":"traces","url":format!("{base}/ok")}),
        )
        .unwrap();
    assert_eq!(ok["_tag"], "Accepted");
    assert!(ok["latencyMs"].is_number());
    assert_eq!(
        c.rpc(
            "server.checkOtlpEndpoint",
            json!({"signal":"logs","url":format!("{base}/bad")})
        )
        .unwrap(),
        json!({"_tag":"Rejected","status":503})
    );
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert_eq!(
        c.rpc(
            "server.checkOtlpEndpoint",
            json!({"signal":"metrics","url":format!("http://{closed}/v1/metrics")})
        )
        .unwrap(),
        json!({"_tag":"Unreachable","timedOut":false})
    );
    assert!(c
        .rpc(
            "server.checkOtlpEndpoint",
            json!({"signal":"traces","url":"file:///etc/passwd"})
        )
        .is_err());
    assert_eq!(
        c.rpc("server.refreshUsageRates", json!({})).unwrap()["status"],
        "unavailable"
    );

    // Ignored files: only files, folders of ignored files collapse away.
    let project = scratch.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    repo(&project);
    write(&project.join(".gitignore"), ".env\nnode_modules/\n*.log\n");
    write(&project.join("a.txt"), "a");
    commit_all(&project, "init");
    write(&project.join(".env"), "SECRET=1");
    write(&project.join("debug.log"), "xx");
    write(&project.join("node_modules/pkg/index.js"), "x");
    let ignored = c
        .rpc("projects.listIgnoredFiles", json!({"cwd":project}))
        .unwrap();
    assert_eq!(
        ignored,
        json!({"files":[{"path":".env","byteLength":8},{"path":"debug.log","byteLength":2}],"truncated":false})
    );
    let missing = c
        .rpc(
            "projects.listIgnoredFiles",
            json!({"cwd":scratch.join("plain")}),
        )
        .unwrap_err()
        .to_string();
    assert!(missing.contains("does not exist"), "{missing}");
    let notgit = scratch.join("notgit");
    std::fs::create_dir_all(&notgit).unwrap();
    assert_eq!(
        c.rpc("projects.listIgnoredFiles", json!({"cwd":notgit}))
            .unwrap(),
        json!({"files":[],"truncated":false})
    );

    // New named project: slug folder, README, icon and an initial commit; names never collide.
    let made = c
        .rpc("projects.createNew", json!({"name":"Mi Proyecto Ñandú!"}))
        .unwrap();
    let root = std::path::PathBuf::from(made["workspaceRoot"].as_str().unwrap());
    assert_eq!(root.file_name().unwrap(), "mi-proyecto-nandu");
    assert_eq!(
        root.parent().unwrap(),
        server.dir.parent().unwrap().join("projects")
    );
    assert!(made.get("commitError").is_none(), "{made}");
    assert_eq!(git(&root, &["log", "--format=%s"]).trim(), "Initial commit");
    assert!(std::fs::read_to_string(root.join("README.md"))
        .unwrap()
        .contains("# Mi Proyecto Ñandú!"));
    assert!(std::fs::read_to_string(root.join("assets/icon.svg"))
        .unwrap()
        .contains(">MP<"));
    let again = c
        .rpc("projects.createNew", json!({"name":"Mi Proyecto Ñandú!"}))
        .unwrap();
    assert!(again["workspaceRoot"]
        .as_str()
        .unwrap()
        .ends_with("mi-proyecto-nandu-2"));
    let rows: i64 = rusqlite::Connection::open(server.dir.join("o3.sqlite"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE kind='project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 2);

    // Clone: a local origin stands in for the remote; failures leave nothing behind.
    let origin = scratch.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    repo(&origin);
    write(&origin.join("f.txt"), "hello");
    commit_all(&origin, "first");
    let dest = scratch.join("clones/copy");
    let cloned = c.rpc("projectClone.start", json!({"projectId":"clone-1","title":"Copy","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":origin.to_str().unwrap(),"destinationPath":dest})).unwrap();
    assert_eq!(
        (cloned["projectId"].as_str(), cloned["cwd"].as_str()),
        (Some("clone-1"), dest.to_str())
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("f.txt")).unwrap(),
        "hello"
    );
    let busy = c.rpc("projectClone.start", json!({"projectId":"clone-2","title":"Copy","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":origin.to_str().unwrap(),"destinationPath":dest})).unwrap_err().to_string();
    assert!(busy.contains("not empty"), "{busy}");
    let bad_dest = scratch.join("clones/bad");
    let missing = c.rpc("projectClone.start", json!({"projectId":"clone-3","title":"Bad","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":scratch.join("no-such-origin").to_str().unwrap(),"destinationPath":bad_dest}));
    assert!(missing.is_err() && !bad_dest.exists());
    assert!(c.rpc("projectClone.start", json!({"projectId":"c4","title":"x","createdAt":"2026-01-01T00:00:00.000Z","remoteUrl":"--upload-pack=touch /tmp/x","destinationPath":scratch.join("clones/evil")})).is_err());

    // Worktree setup is never staged: nothing to cancel, the stream says null.
    assert_eq!(
        c.rpc("worktreeSetup.cancel", json!({"threadId":"t"}))
            .unwrap(),
        json!({"cancelled":false})
    );
    let mut setup = subscribe(
        server.connect(),
        "subscribeWorktreeSetup",
        json!({"threadId":"t"}),
    );
    assert_eq!(setup.next(Duration::from_secs(10)), Value::Null);

    // Older settled threads: counts with limit 0, pages newest settle first, cursor and project filter.
    c.rpc("projects.mutate", json!({"type":"project.create","commandId":"pc-s","projectId":"ps","title":"S","workspaceRoot":project,"scripts":[]})).unwrap();
    for id in ["s1", "s2", "s3"] {
        c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":format!("tc-{id}"),"threadId":id,"projectId":"ps","title":id,"modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
        c.rpc(
            "orchestration.dispatchCommand",
            json!({"type":"thread.settle","commandId":format!("st-{id}"),"threadId":id}),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let far = "2999-01-01T00:00:00.000Z";
    let counts = c
        .rpc(
            "orchestration.getOlderSettledThreads",
            json!({"before":{"settledAt":far},"limit":0}),
        )
        .unwrap();
    assert_eq!(counts["counts"], json!([{"projectId":"ps","count":3}]));
    assert!(counts["threads"].as_array().unwrap().is_empty());
    let page = c
        .rpc(
            "orchestration.getOlderSettledThreads",
            json!({"before":{"settledAt":far},"limit":2}),
        )
        .unwrap();
    let ids: Vec<_> = page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s3", "s2"], "newest settle first");
    let cursor = page["threads"][1]["settledAt"].clone();
    let next = c
        .rpc(
            "orchestration.getOlderSettledThreads",
            json!({"before":{"settledAt":cursor},"limit":5}),
        )
        .unwrap();
    assert_eq!(
        next["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["s1"]
    );
    let other = c
        .rpc(
            "orchestration.getOlderSettledThreads",
            json!({"before":{"settledAt":far},"projectIds":["nope"],"limit":0}),
        )
        .unwrap();
    assert!(other["counts"].as_array().unwrap().is_empty());

    // Workflow scripts: only .js files below $CLAUDE_CONFIG_DIR/projects.
    let script = claude.join("projects/p/wf.js");
    write(&script, "export default 1;\n");
    write(&claude.join("projects/p/wf.txt"), "x");
    write(&scratch.join("outside.js"), "x");
    let got = c
        .rpc(
            "orchestration.getWorkflowScript",
            json!({"threadId":"s1","scriptPath":script}),
        )
        .unwrap();
    assert_eq!(
        (got["contents"].as_str(), got["truncated"].as_bool()),
        (Some("export default 1;\n"), Some(false))
    );
    for (path, reason) in [
        (scratch.join("outside.js"), "outside-root"),
        (claude.join("projects/p/missing.js"), "not-found"),
        (claude.join("projects/p/wf.txt"), "invalid-path"),
    ] {
        let e = raw_exit(
            &server,
            "orchestration.getWorkflowScript",
            json!({"threadId":"s1","scriptPath":path}),
        );
        assert_eq!(
            (
                error_of(&e)["_tag"].as_str(),
                error_of(&e)["reason"].as_str()
            ),
            (Some("OrchestrationGetWorkflowScriptError"), Some(reason)),
            "{e}"
        );
    }
    let e = raw_exit(
        &server,
        "orchestration.getWorkflowScript",
        json!({"threadId":"s1","scriptPath":"relative.js"}),
    );
    assert_eq!(error_of(&e)["reason"], "invalid-path");

    // Surfaces that need a runtime this server does not have answer T3's typed errors.
    for (tag, payload, error, reason) in [
        (
            "device.detail",
            json!({"hostId":"local","deviceId":"d"}),
            "DeviceToolUnavailableError",
            None,
        ),
        (
            "device.easAccount",
            json!({}),
            "DeviceEasAccountError",
            None,
        ),
        (
            "guestShare.create",
            json!({"threadId":"s1"}),
            "GuestShareCreateError",
            Some("disabled"),
        ),
        (
            "mcpApps.callTool",
            json!({"threadId":"s1"}),
            "McpAppRequestError",
            Some("provider-unsupported"),
        ),
        (
            "orchestration.stopWorkflow",
            json!({"threadId":"s1","subagentId":"n"}),
            "OrchestrationV2StopWorkflowError",
            Some("unsupported"),
        ),
    ] {
        let e = raw_exit(&server, tag, payload);
        assert_eq!(error_of(&e)["_tag"], error, "{tag}: {e}");
        if let Some(reason) = reason {
            assert_eq!(error_of(&e)["reason"], reason, "{tag}");
        }
    }
    let _ = std::fs::remove_dir_all(&claude);
}
