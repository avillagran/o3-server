//! Nightly 4262 title batches and the shell rows omitted from the main snapshot.
#[path = "support/reference.rs"]
mod reference;
mod support;
use serde_json::{json, Value};

fn fixture() -> (support::Server, t3_protocol::Connection, std::path::PathBuf) {
    let server = support::start(|_| {});
    let client = server.connect();
    let root = server.dir.join("workspace");
    std::fs::create_dir(&root).unwrap();
    client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Read fixtures","workspaceRoot":root})).unwrap();
    for id in [
        "root",
        "fork",
        "child",
        "grandchild",
        "side",
        "archived",
        "deleted",
        "neighbor",
        "__proto__",
    ] {
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":format!("create-{id}"),"threadId":id,"projectId":"p","title":format!("Title {id}"),"modelSelection":{"instanceId":"scripted","model":"fixture","options":[]},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
    }
    // Model source-owned lineage that ordinary create cannot attach, without creating native work.
    {
        let db = server.state.db.lock().unwrap();
        for (id, parent, relationship, root_id) in [
            ("fork", "root", "fork", "root"),
            ("child", "root", "subagent", "root"),
            ("grandchild", "child", "subagent", "root"),
            ("side", "root", "fork", "root"),
            ("archived", "root", "fork", "root"),
            ("deleted", "root", "fork", "root"),
        ] {
            let body: String = db
                .query_row(
                    "SELECT body FROM records WHERE kind='thread' AND id=?",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            let mut row: Value = serde_json::from_str(&body).unwrap();
            row["lineage"] = json!({"parentThreadId":parent,"relationshipToParent":relationship,"rootThreadId":root_id});
            row["lineage"]["sideChat"] = json!(id == "side");
            row["updatedAt"] = json!("2026-11-06T00:00:01.000Z");
            row["worktreePath"] = json!(root.join("linked").to_str().unwrap());
            if id == "archived" {
                row["archivedAt"] = json!("2026-11-06T00:00:02.000Z");
            }
            if id == "deleted" {
                row["deletedAt"] = json!("2026-11-06T00:00:02.000Z");
            }
            db.execute(
                "UPDATE records SET body=? WHERE kind='thread' AND id=?",
                rusqlite::params![row.to_string(), id],
            )
            .unwrap();
        }
    }
    (server, client, root)
}

#[test]
fn nightly_thread_reads_keep_archived_titles_and_active_family_members() {
    let (server, client, root) = fixture();
    let read = server.limited(&client, &["orchestration:read"]);
    assert_eq!(
        read.rpc("orchestration.getThreadTitles", json!({"threadIds":[]}))
            .unwrap(),
        json!({})
    );
    assert_eq!(
        read.rpc(
            "orchestration.getThreadTitles",
            json!({"threadIds":["root","archived","deleted","unknown","root","__proto__"]})
        )
        .unwrap(),
        json!({"root":"Title root","archived":"Title archived","__proto__":"Title __proto__"})
    );
    let ids = |v: &Value| {
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let family = read
        .rpc("orchestration.getThreadFamily", json!({"threadId":"child"}))
        .unwrap();
    assert_eq!(
        ids(&family),
        ["child", "fork", "grandchild", "side", "root"]
    );
    assert!(family["snapshotSequence"].is_u64());
    assert!(family["threads"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["itemCount"].is_u64() && t["status"].is_string()));
    assert_eq!(
        read.rpc(
            "orchestration.getThreadFamily",
            json!({"threadId":"deleted"})
        )
        .unwrap(),
        family,
        "deleted members still identify their family"
    );
    assert!(ids(&read
        .rpc(
            "orchestration.getThreadFamily",
            json!({"threadId":"unknown"})
        )
        .unwrap())
    .is_empty());
    assert_eq!(
        read.rpc(
            "orchestration.getWorktreeThreadIds",
            json!({"worktreePath":root.join("linked")})
        )
        .unwrap(),
        json!({"threadIds":["child","fork","grandchild","side"]})
    );
    assert_eq!(
        read.rpc(
            "orchestration.getWorktreeThreadIds",
            json!({"worktreePath":root.join("missing")})
        )
        .unwrap(),
        json!({"threadIds":[]})
    );
    // A single bound JSON array handles batches larger than SQLite's variable limit.
    let mut many = (0..35000)
        .map(|i| json!(format!("missing-{i}")))
        .collect::<Vec<_>>();
    many.extend([json!("archived"), json!("root")]);
    assert_eq!(
        read.rpc("orchestration.getThreadTitles", json!({"threadIds":many}))
            .unwrap(),
        json!({"archived":"Title archived","root":"Title root"})
    );
    let denied = server.limited(&client, &["terminal:read"]);
    for (tag, payload) in [
        (
            "orchestration.getThreadTitles",
            json!({"threadIds":["root"]}),
        ),
        ("orchestration.getThreadFamily", json!({"threadId":"root"})),
        (
            "orchestration.getWorktreeThreadIds",
            json!({"worktreePath":root}),
        ),
    ] {
        let error = denied.rpc(tag, payload).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing required scope: orchestration:read"),
            "{error}"
        );
    }
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference in rootless Podman"]
fn official_nightly_matches_thread_read_membership_order_and_titles() {
    let (server, client, root) = fixture();
    let (native, process) = reference::start(&server.dir);
    native.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Read fixtures","workspaceRoot":root})).unwrap();
    let originals: Vec<(String, Value)> = {
        let db = server.state.db.lock().unwrap();
        let mut q = db
            .prepare("SELECT id,body FROM records WHERE kind='thread' ORDER BY rowid")
            .unwrap();
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(|r| {
                let (id, body) = r.unwrap();
                (id, serde_json::from_str(&body).unwrap())
            })
            .collect()
    };
    for (id, _) in &originals {
        native.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","createdBy":"user","creationSource":"web","branch":null,"worktreePath":null,"commandId":format!("create-{id}"),"threadId":id,"projectId":"p","title":format!("Title {id}"),"modelSelection":{"instanceId":"codex","model":"gpt-6.1-sol","options":[]},"runtimeMode":"approval-required","interactionMode":"default"})).unwrap();
    }
    // Keep the official runtime's own records; set only the fixture's selection columns.
    let db =
        rusqlite::Connection::open(server.dir.join("reference/userdata/statev2.sqlite")).unwrap();
    for (id, original) in &originals {
        let body: String = db
            .query_row(
                "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id=?",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let mut body: Value = serde_json::from_str(&body).unwrap();
        for key in [
            "lineage",
            "updatedAt",
            "worktreePath",
            "archivedAt",
            "deletedAt",
        ] {
            body[key] = original[key].clone();
        }
        db.execute("UPDATE orchestration_v2_projection_threads SET payload_json=?,updated_at=?,archived_at=?,deleted_at=? WHERE thread_id=?",rusqlite::params![body.to_string(),body["updatedAt"].as_str(),body["archivedAt"].as_str(),body["deletedAt"].as_str(),id]).unwrap();
    }
    drop(db);
    let raw = support::RawClient::pair(&server);
    for (tag, field) in [
        ("orchestration.getThreadTitles", "threadIds"),
        ("orchestration.getThreadFamily", "threadId"),
        ("orchestration.getWorktreeThreadIds", "worktreePath"),
    ] {
        let mut invalids = if field == "threadIds" {
            vec![
                json!({}),
                json!({field:null}),
                json!({field:[2]}),
                json!({field:[" "]}),
                json!({field:["root",null]}),
                json!({field:["root","\u{feff}"]}),
                json!({field:"root"}),
            ]
        } else {
            vec![
                json!({}),
                json!({field:null}),
                json!({field:2}),
                json!({field:[]}),
                json!({field:"\u{feff}"}),
                json!({field:"\n\t"}),
            ]
        };
        invalids.extend([Value::Null, json!([]), json!(5), json!("bad")]);
        for payload in invalids {
            let theirs = process.raw_exit(tag, payload.clone());
            let ours = raw.exit(tag, payload.clone());
            assert_eq!(ours, theirs, "malformed {tag}: {payload}");
        }
    }
    for payload in [
        json!({"threadIds":[]}),
        json!({"threadIds":["root","root","archived","deleted","unknown","__proto__"]}),
        json!({"threadIds":["\u{feff}root\u{feff}","'; SELECT 1;--"]}),
    ] {
        assert_eq!(
            native
                .rpc("orchestration.getThreadTitles", payload.clone())
                .unwrap(),
            client
                .rpc("orchestration.getThreadTitles", payload)
                .unwrap()
        );
    }
    for id in [
        "root", "child", "deleted", "archived", "unknown", "neighbor",
    ] {
        let payload = json!({"threadId":id});
        let ours = client
            .rpc("orchestration.getThreadFamily", payload.clone())
            .unwrap();
        let theirs = native
            .rpc("orchestration.getThreadFamily", payload)
            .unwrap();
        let members = |v: &Value| {
            v["threads"].as_array().unwrap().iter().map(|t|json!({"id":t["id"],"title":t["title"],"lineage":t["lineage"],"archivedAt":t["archivedAt"],"deletedAt":t["deletedAt"]})).collect::<Vec<_>>()
        };
        assert_eq!(members(&ours), members(&theirs), "family of {id}");
        assert!(theirs["snapshotSequence"].is_u64());
    }
    for path in [root.clone(), root.join("linked"), root.join("missing")] {
        let payload = json!({"worktreePath":path});
        assert_eq!(
            client
                .rpc("orchestration.getWorktreeThreadIds", payload.clone())
                .unwrap(),
            native
                .rpc("orchestration.getWorktreeThreadIds", payload)
                .unwrap()
        );
    }
    let issued = std::process::Command::new(std::env::var("T3_REFERENCE_BIN").unwrap())
        .env("T3CODE_HOME", server.dir.join("reference"))
        .args([
            "auth",
            "session",
            "issue",
            "--subject",
            "limited",
            "--ttl",
            "1h",
            "--scope",
            "terminal:read",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(
        issued.status.success(),
        "Reference scoped auth failed: {}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let token = String::from_utf8(issued.stdout).unwrap();
    let native_denied =
        support::RawClient::bearer(native.base_url().trim_end_matches('/'), token.trim());
    let grant = server
        .state
        .issue_pairing_scoped(Some("limited raw"), 60000, "terminal:read", "owner")
        .unwrap();
    let exchanged = server
        .state
        .exchange_pairing(grant["credential"].as_str().unwrap(), &json!({}))
        .unwrap()
        .unwrap();
    let ours_denied =
        support::RawClient::bearer(&server.base, exchanged["access_token"].as_str().unwrap());
    for (tag, valid) in [
        (
            "orchestration.getThreadTitles",
            json!({"threadIds":["root"]}),
        ),
        ("orchestration.getThreadFamily", json!({"threadId":"root"})),
        (
            "orchestration.getWorktreeThreadIds",
            json!({"worktreePath":root}),
        ),
    ] {
        for payload in [valid, json!({})] {
            assert_eq!(
                ours_denied.exit(tag, payload.clone()),
                native_denied.exit(tag, payload.clone()),
                "scope/decode ordering for {tag}: {payload}"
            );
        }
    }
    for (tag, payload) in [
        (
            "server.updateSettings",
            json!({"patch":{"confirmThreadDelete":true}}),
        ),
        (
            "server.updateSettings",
            json!({"patch":{"providerInstances":{}}}),
        ),
        (
            "projects.readFile",
            json!({"cwd":root,"relativePath":"missing.txt"}),
        ),
    ] {
        assert_eq!(
            ours_denied.exit(tag, payload.clone()),
            native_denied.exit(tag, payload.clone()),
            "permission response for {tag}: {payload}"
        );
    }
    println!("OFFICIAL_NIGHTLY_THREAD_READS_PASS title batches, Unicode trim, deleted/archived anchors, descendants, side chats, ties and worktree membership");
}

#[test]
fn malformed_thread_reads_return_decode_defects_before_typed_handlers() {
    let server = support::start(|_| {});
    let raw = support::RawClient::pair(&server);
    for (tag, payload, expected) in [
        (
            "orchestration.getThreadTitles",
            Value::Null,
            "Expected object",
        ),
        (
            "orchestration.getThreadTitles",
            json!({}),
            "Missing key\n  at [\"threadIds\"]",
        ),
        (
            "orchestration.getThreadTitles",
            json!({"threadIds":null}),
            "Expected array\n  at [\"threadIds\"]",
        ),
        (
            "orchestration.getThreadTitles",
            json!({"threadIds":["valid",4]}),
            "Expected string\n  at [\"threadIds\"][1]",
        ),
        (
            "orchestration.getThreadTitles",
            json!({"threadIds":["valid","\u{feff}"]}),
            "Expected a non-blank string\n  at [\"threadIds\"][1]",
        ),
        (
            "orchestration.getThreadFamily",
            json!({"threadId":"\n\t"}),
            "Expected a non-blank string\n  at [\"threadId\"]",
        ),
        (
            "orchestration.getWorktreeThreadIds",
            json!({"worktreePath":false}),
            "Expected string\n  at [\"worktreePath\"]",
        ),
    ] {
        assert_eq!(
            raw.exit(tag, payload),
            json!({"_tag":"Exit","requestId":"r","exit":{"_tag":"Failure","cause":[{"_tag":"Die","defect":expected}]}})
        );
    }
}

#[test]
fn rpc_permission_denials_preserve_granular_and_legacy_names() {
    let server = support::start(|_| {});
    let grant = server
        .state
        .issue_pairing_scoped(Some("permission fixture"), 60000, "terminal:read", "owner")
        .unwrap();
    let session = server
        .state
        .exchange_pairing(grant["credential"].as_str().unwrap(), &json!({}))
        .unwrap()
        .unwrap();
    let raw = support::RawClient::bearer(&server.base, session["access_token"].as_str().unwrap());
    for (tag, payload, permission, legacy) in [
        (
            "projects.readFile",
            json!({"cwd":"/tmp","relativePath":"missing"}),
            "filesystem:read",
            "orchestration:read",
        ),
        (
            "server.updateSettings",
            json!({"patch":{"confirmThreadDelete":true}}),
            "settings:write",
            "orchestration:operate",
        ),
        (
            "server.updateSettings",
            json!({"patch":{"providerInstances":{}}}),
            "providers:manage",
            "orchestration:operate",
        ),
    ] {
        assert_eq!(
            raw.exit(tag, payload),
            json!({"_tag":"Exit","requestId":"r","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"EnvironmentAuthorizationError","message":format!("The authenticated token is missing required scope: {permission}."),"requiredPermission":permission,"requiredScope":legacy}}]}})
        );
    }
}
