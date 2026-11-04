mod common;
mod support;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
use t3_protocol::Connection;
const WAIT: Duration = Duration::from_secs(15);
fn git(at: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(at)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}
fn rpc(c: &Connection, p: Value) -> Value {
    c.rpc("orchestration.dispatchCommand", p).unwrap()
}
fn projection(c: &Connection) -> Value {
    t3_protocol::api::orchestration::thread_projection(c, "t").unwrap()
}
fn count(s: &support::Server, sql: &str) -> i64 {
    s.state
        .db
        .lock()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}
fn request(
    base: &str,
    auth: &str,
    sid: Option<&str>,
    method: &str,
    params: Value,
) -> (String, Value) {
    let mut headers = vec![
        ("authorization", auth),
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    if let Some(sid) = sid {
        headers.push(("mcp-session-id", sid));
    }
    let (code, h, b) = common::http(
        base,
        "POST",
        "/mcp",
        &headers,
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&b));
    (h, serde_json::from_slice(&b).unwrap())
}
fn session(base: &str, auth: &str) -> String {
    request(base,auth,None,"initialize",json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"handoff-test","version":"1"},"capabilities":{}})).0.lines().find_map(|l|l.strip_prefix("mcp-session-id:").map(str::trim)).unwrap().into()
}
fn call(base: &str, auth: &str, sid: &str, p: Value) -> Value {
    request(
        base,
        auth,
        Some(sid),
        "tools/call",
        json!({"name":"t3_worktree_handoff","arguments":p}),
    )
    .1
}
fn ok(reply: Value) -> Value {
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    reply["result"]["structuredContent"].clone()
}
fn denied(reply: Value, code: &str) -> Value {
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let v = reply["result"]["structuredContent"].clone();
    assert_eq!(v["code"], code, "{v}");
    v
}
struct Fixture {
    s: support::Server,
    c: Connection,
    root: PathBuf,
    auth: String,
    sid: String,
    claude: Arc<o3_server::claude::ClaudeProvider>,
    codex: Arc<o3_server::codex::CodexProvider>,
}
impl Fixture {
    fn new(instance: &str) -> Self {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let s = support::start(|s| {
            s.streams.terminals.set_shell("/bin/sh");
            std::fs::write(s.dir.join("settings.json"),json!({"enableAgentBrowserAccess":false,"worktreesDirectory":s.dir.join("managed")}).to_string()).unwrap();
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let root = s.dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        std::fs::write(root.join("README.md"), "committed\n").unwrap();
        git(&root, &["add", "README.md"]);
        git(&root, &["commit", "-m", "initial"]);
        let c = s.connect();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":root})).unwrap();
        rpc(
            &c,
            json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"t","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default"}),
        );
        rpc(
            &c,
            json!({"type":"message.dispatch","commandId":"first","threadId":"t","messageId":"first","text":"wait-target","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        );
        assert!(support::wait_for(WAIT, || projection(&c)
            ["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["status"] == "pending")));
        let observed: Value =
            serde_json::from_slice(&std::fs::read(root.join("observe-wait-target.json")).unwrap())
                .unwrap();
        let auth: String = observed["config"]["headers"]["Authorization"]
            .as_str()
            .or(observed["config"]["http_headers"]["Authorization"].as_str())
            .unwrap()
            .into();
        let sid = session(&s.base, &auth);
        Self {
            s,
            c,
            root,
            auth,
            sid,
            claude,
            codex,
        }
    }
    fn call(&self, p: Value) -> Value {
        call(&self.s.base, &self.auth, &self.sid, p)
    }
    fn settings(&self, p: Value) {
        let file = self.s.dir.join("settings.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        for (k, vv) in p.as_object().unwrap() {
            v[k] = vv.clone();
        }
        std::fs::write(file, v.to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            self.claude.shutdown().await;
            self.codex.shutdown().await;
        });
    }
}
fn checkout_gate(f: &Fixture) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let entered = f.s.dir.join("hook-entered");
    let release = f.s.dir.join("hook-release");
    let hook = f.root.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n: > '{}'\nwhile [ ! -f '{}' ]; do sleep .02; done\n",
            entered.display(),
            release.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    (entered, release)
}

#[test]
fn handoff_runs_setup_in_visible_terminal_and_continues_same_native_conversation() {
    for instance in ["claudeAgent", "codex"] {
        let f = Fixture::new(instance);
        f.settings(json!({"projectSettingsOverrides":{"p":{"newWorktreesStartFromOrigin":false,"defaultProjectScripts":[{"id":"install","name":"Install","icon":"configure","command":"printf '%s|%s|%s' \"$T3CODE_PROJECT_ROOT\" \"$T3CODE_WORKTREE_PATH\" \"$PWD\" > setup.txt; echo setup-completed","runOnWorktreeCreate":true,"async":false}]}}}));
        std::fs::write(f.root.join("README.md"), "uncommitted user changes\n").unwrap();
        let original =
            projection(&f.c)["providerThreads"][0]["nativeThreadRef"]["nativeId"].clone();
        let supplied = f.s.dir.join("ignored-user-path");
        let result=ok(f.call(json!({"branch":" feature/live ","continuationPrompt":" idle-continuation ","path":supplied})));
        let work = PathBuf::from(result["worktreePath"].as_str().unwrap());
        assert_eq!(work, f.s.dir.join("managed/repo/feature-live"));
        assert!(!supplied.exists());
        assert_eq!(result["branch"], "feature/live");
        assert_eq!(result["baseRef"], "main");
        assert_eq!(result["startedFromOrigin"], false);
        assert_eq!(
            result["setupScript"],
            json!({"status":"started","scriptName":"Install","terminalId":"setup-install"})
        );
        assert_eq!(
            result["continuation"],
            json!({"status":"scheduled","delivery":"queued"})
        );
        assert!(support::wait_for(WAIT, || work.join("setup.txt").exists()
            && work.join("observe-idle-continuation.json").exists()
            && projection(&f.c)["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["status"] == "completed")));
        assert_eq!(
            std::fs::read_to_string(work.join("README.md")).unwrap(),
            "committed\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.root.join("README.md")).unwrap(),
            "uncommitted user changes\n"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("setup.txt")).unwrap(),
            format!("{}|{}|{}", f.root.display(), work.display(), work.display())
        );
        let observed: Value = serde_json::from_slice(
            &std::fs::read(work.join("observe-idle-continuation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(observed["native"], original);
        let messages = projection(&f.c)["messages"].clone();
        let continuation = messages
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["text"] == "idle-continuation")
            .unwrap();
        assert_eq!(continuation["createdBy"], "agent");
        assert_eq!(continuation["creationSource"], "mcp");
        let mut terminal = support::subscribe(
            f.s.connect(),
            "terminal.observe",
            json!({"threadId":"t","terminalId":"setup-install"}),
        );
        let snap = terminal.next(WAIT);
        assert_eq!(snap["snapshot"]["cwd"], json!(work));
        rpc(
            &f.c,
            json!({"type":"thread.delete","commandId":"delete","threadId":"t"}),
        );
        assert!(support::wait_for(WAIT, || count(
            &f.s,
            "SELECT count(*) FROM deletion_effects WHERE completed_at IS NULL"
        ) == 0));
        assert!(work.join(".git").is_file());
    }
}

#[test]
fn handoff_resolves_fetched_upstream_alias_and_preserves_remote_comparison_base() {
    let f = Fixture::new("codex");
    git(&f.root, &["branch", "-m", "stable"]);
    let remote = f.s.dir.join("remote.git");
    git(&f.root, &["init", "--bare", remote.to_str().unwrap()]);
    git(
        &f.root,
        &["remote", "add", "upstream/team", remote.to_str().unwrap()],
    );
    git(&f.root, &["push", "-u", "upstream/team", "stable:release"]);
    let expected = git(&f.root, &["rev-parse", "HEAD"]);
    git(&f.root, &["commit", "--allow-empty", "-m", "local only"]);
    f.settings(json!({"projectSettingsOverrides":{"p":{"newWorktreesStartFromOrigin":true}}}));
    let result =
        ok(f.call(json!({"branch":"feature/remote","baseRef":"stable","runSetupScript":false})));
    let work = Path::new(result["worktreePath"].as_str().unwrap());
    assert_eq!(git(work, &["rev-parse", "HEAD"]), expected);
    assert_eq!(result["startedFromOrigin"], true);
    assert_eq!(result["continuation"]["status"], "skipped");
    assert_eq!(result["setupScript"]["status"], "skipped");
    assert_eq!(
        git(work, &["config", "branch.feature/remote.t3-merge-base"]),
        "release"
    );
    assert_eq!(
        git(work, &["config", "branch.feature/remote.t3-merge-base-ref"]),
        "refs/remotes/upstream/team/release"
    );
}

#[test]
fn handoff_rejects_bad_inputs_and_uses_local_base_if_remote_branch_was_deleted() {
    let f = Fixture::new("codex");
    let external =
        f.s.state
            .issue_mcp_session("full-access", 60000, "outside")
            .unwrap();
    let external = format!("Bearer {external}");
    let sid = session(&f.s.base, &external);
    denied(
        call(&f.s.base, &external, &sid, json!({"branch":"outside"})),
        "thread_credential_required",
    );
    let existing = denied(f.call(json!({"branch":"main"})), "invalid_request");
    assert!(existing["message"]
        .as_str()
        .unwrap()
        .contains(f.root.to_str().unwrap()));
    denied(f.call(json!({"branch":"../escape"})), "invalid_request");
    git(&f.root, &["checkout", "--detach"]);
    denied(f.call(json!({"branch":"detached"})), "invalid_request");
    git(&f.root, &["checkout", "main"]);
    let remote = f.s.dir.join("empty.git");
    git(&f.root, &["init", "--bare", remote.to_str().unwrap()]);
    git(
        &f.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let head = git(&f.root, &["rev-parse", "HEAD"]);
    git(&f.root, &["update-ref", "refs/remotes/origin/main", &head]);
    let result = ok(f.call(json!({"branch":"main-new","startFromOrigin":true})));
    assert_eq!(result["startedFromOrigin"], false);
    assert_eq!(result["setupScript"]["status"], "no-script");
    assert_eq!(
        git(
            Path::new(result["worktreePath"].as_str().unwrap()),
            &["rev-parse", "HEAD"]
        ),
        head
    );
}

#[test]
fn binding_failure_rolls_back_continuation_and_owned_git_resources_but_partial_followups_are_reported(
) {
    let f = Fixture::new("codex");
    f.s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_handoff BEFORE INSERT ON receipts WHEN NEW.command_id LIKE 'mcp:worktree-handoff:%' AND NEW.command_id NOT LIKE '%:continue' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    denied(
        f.call(json!({"branch":"feature/retry","continuationPrompt":"idle-continuation"})),
        "operation_failed",
    );
    assert!(projection(&f.c)["thread"]["worktreePath"].is_null());
    assert_eq!(projection(&f.c)["runs"][0]["status"], "waiting");
    assert_eq!(
        count(&f.s, "SELECT count(*) FROM records WHERE kind='messages'"),
        1
    );
    assert_eq!(count(&f.s, "SELECT count(*) FROM workspace_detaches"), 0);
    assert!(!f.s.dir.join("managed/repo/feature-retry").exists());
    assert!(!git(&f.root, &["branch", "--list", "feature/retry"]).contains("feature/retry"));
    f.s.state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_handoff")
        .unwrap();
    *f.s.state.provider.lock().unwrap() = None;
    f.settings(json!({"projectSettingsOverrides":{"p":{"defaultProjectScripts":[{"id":"x".repeat(150),"name":"Legacy setup","command":"echo unexpected","runOnWorktreeCreate":true}]}}}));
    let result =
        ok(f.call(json!({"branch":"feature/retry","continuationPrompt":"idle-continuation"})));
    assert_eq!(result["continuation"]["status"], "failed");
    assert_eq!(result["setupScript"]["status"], "failed");
    assert!(Path::new(result["worktreePath"].as_str().unwrap()).exists());
    assert_eq!(
        count(&f.s, "SELECT count(*) FROM records WHERE kind='messages'"),
        1
    );
}

#[test]
fn concurrent_handoff_is_serialized_and_ui_binding_race_rolls_back_only_the_created_worktree() {
    let f = Fixture::new("codex");
    let (entered, release) = checkout_gate(&f);
    let (base, auth, sid) = (f.s.base.clone(), f.auth.clone(), f.sid.clone());
    let pending = std::thread::spawn(move || {
        call(
            &base,
            &auth,
            &sid,
            json!({"branch":"feature/race","continuationPrompt":"idle-continuation"}),
        )
    });
    assert!(support::wait_for(WAIT, || entered.exists()));
    denied(
        f.call(json!({"branch":"feature/concurrent"})),
        "handoff_in_progress",
    );
    let other = f.s.dir.join("other-worktree");
    std::fs::create_dir_all(&other).unwrap();
    rpc(
        &f.c,
        json!({"type":"thread.metadata.update","commandId":"ui-bind","threadId":"t","expectedWorktreePath":null,"worktreePath":other}),
    );
    std::fs::write(release, "").unwrap();
    denied(pending.join().unwrap(), "already_in_worktree");
    assert_eq!(projection(&f.c)["thread"]["worktreePath"], json!(other));
    assert!(other.exists());
    assert!(!f.s.dir.join("managed/repo/feature-race").exists());
    assert!(git(&f.root, &["branch", "--list", "feature/race"]).is_empty());
    assert_eq!(
        count(&f.s, "SELECT count(*) FROM records WHERE kind='messages'"),
        1
    );
}

#[test]
fn native_authority_is_rechecked_after_checkout_before_binding_or_continuation() {
    let f = Fixture::new("codex");
    let (entered, release) = checkout_gate(&f);
    let (base, auth, sid) = (f.s.base.clone(), f.auth.clone(), f.sid.clone());
    let pending = std::thread::spawn(move || {
        call(
            &base,
            &auth,
            &sid,
            json!({"branch":"feature/stale","continuationPrompt":"idle-continuation"}),
        )
    });
    assert!(support::wait_for(WAIT, || entered.exists()));
    let observed: Value =
        serde_json::from_slice(&std::fs::read(f.root.join("observe-wait-target.json")).unwrap())
            .unwrap();
    unsafe {
        libc::kill(observed["pid"].as_i64().unwrap() as i32, libc::SIGKILL);
    }
    assert!(support::wait_for(WAIT, || projection(&f.c)["runs"][0]
        ["status"]
        == "failed"));
    std::fs::write(release, "").unwrap();
    denied(pending.join().unwrap(), "capability_denied");
    assert!(projection(&f.c)["thread"]["worktreePath"].is_null());
    assert!(!f.s.dir.join("managed/repo/feature-stale").exists());
    assert!(git(&f.root, &["branch", "--list", "feature/stale"]).is_empty());
    assert_eq!(
        count(&f.s, "SELECT count(*) FROM records WHERE kind='messages'"),
        1
    );
}

#[test]
fn caller_transport_disconnect_during_checkout_does_not_lose_binding_or_continuation() {
    use std::io::Write;
    let f = Fixture::new("codex");
    let (entered, release) = checkout_gate(&f);
    let body=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"t3_worktree_handoff","arguments":{"branch":"feature/disconnected","runSetupScript":false,"continuationPrompt":"idle-continuation"}}}).to_string();
    let addr = f.s.base.trim_start_matches("http://");
    let mut transport = std::net::TcpStream::connect(addr).unwrap();
    write!(transport,"POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {}\r\nMcp-Session-Id: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n\r\n{body}",f.auth,f.sid,body.len()).unwrap();
    assert!(support::wait_for(WAIT, || entered.exists()));
    drop(transport);
    std::fs::write(release, "").unwrap();
    let work = f.s.dir.join("managed/repo/feature-disconnected");
    assert!(support::wait_for(WAIT, || work
        .join("observe-idle-continuation.json")
        .exists()
        && projection(&f.c)["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["status"] == "completed")));
    assert_eq!(projection(&f.c)["thread"]["worktreePath"], json!(work));
    assert_eq!(
        count(&f.s, "SELECT count(*) FROM records WHERE kind='messages'"),
        2
    );
}
