mod common;
mod support;
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
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
fn repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("README.md"), "test\n").unwrap();
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-m", "First"]);
}
struct Mcp<'a> {
    server: &'a support::Server,
    token: String,
    id: String,
}
impl<'a> Mcp<'a> {
    fn new(server: &'a support::Server, access: &str) -> Self {
        let token = server
            .state
            .issue_mcp_session(access, 60_000, "workspace test")
            .unwrap();
        let auth = format!("Bearer {token}");
        let (code,h,body)=common::http(&server.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json")],json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"workspace-test","version":"1"}}}).to_string().as_bytes());
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
        let id = h
            .lines()
            .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
            .unwrap()
            .into();
        Self { server, token, id }
    }
    fn reply(&self, name: &str, args: Value) -> Value {
        let auth = format!("Bearer {}", self.token);
        let (code,_,body)=common::http(&self.server.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json"),("mcp-session-id",&self.id)],json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string().as_bytes());
        assert_eq!(code, 200);
        serde_json::from_slice(&body).unwrap()
    }
    fn ok(&self, name: &str, args: Value) -> Value {
        let v = self.reply(name, args);
        assert_eq!(v["result"]["isError"], false, "{v}");
        v["result"]["structuredContent"].clone()
    }
    fn denied(&self, name: &str, args: Value, code: &str) {
        let v = self.reply(name, args);
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["structuredContent"]["code"], code, "{v}");
    }
}
#[test]
fn project_crud_controls_persist_real_settings_and_keep_files() {
    let s = support::start(|_| {});
    let m = Mcp::new(&s, "full-access");
    let root = s.dir.join("workspace");
    m.denied(
        "t3_project_create",
        json!({"title":"Missing","workspaceRoot":root}),
        "invalid_request",
    );
    assert!(!root.exists());
    let p = m.ok(
        "t3_project_create",
        json!({"title":"  Registered  ","workspaceRoot":root,"createWorkspaceRootIfMissing":true,"autoPull":true}),
    );
    assert!(root.is_dir());
    assert_eq!(p["title"], "Registered");
    assert_eq!(
        p["autoPull"], false,
        "unadvertised create fields are discarded by the contract"
    );
    let id = p["id"].as_str().unwrap();
    m.denied(
        "t3_project_create",
        json!({"title":"Duplicate","workspaceRoot":root}),
        "invalid_request",
    );
    let scripts = json!([{"id":"test","name":"Test","command":"cargo test","icon":"test","runOnWorktreeCreate":false,"runOnSettle":true,"async":true}]);
    let selection = json!({"instanceId":"codex","model":"gpt-5.3-codex","options":[]});
    let updated=m.ok("t3_project_update",json!({"projectId":id,"title":"Updated","defaultModelSelection":selection,"scripts":scripts,"projectIcon":{"kind":"emoji","emoji":"🦀"},"defaultThreadEnvMode":"worktree","pinned":true,"archived":true,"autoPull":true}));
    assert_eq!(updated["scripts"], scripts);
    assert_eq!(updated["defaultModelSelection"], selection);
    assert_eq!(updated["projectIcon"]["emoji"], "🦀");
    assert!(updated["pinnedAt"].is_string());
    assert!(updated["archivedAt"].is_string());
    m.denied(
        "t3_project_update",
        json!({"projectId":id,"projectIcon":{"kind":"monogram","text":"ABC","color":"blue"}}),
        "invalid_request",
    );
    let composed=m.ok("t3_project_update",json!({"projectId":id,"projectIcon":{"kind":"monogram","text":"A\u{0301}B","color":"blue"}}));
    assert_eq!(composed["projectIcon"]["text"], "A\u{0301}B");
    m.denied("t3_project_update",json!({"projectId":id,"scripts":[{"id":"INVALID","name":"Invalid","command":"true","icon":"play","runOnWorktreeCreate":false}]}),"invalid_request");
    let owner = s.connect();
    let snapshot = owner
        .request_json("GET", "/api/projects", None, false)
        .unwrap();
    assert_eq!(snapshot["projects"][0]["scripts"], scripts);
    let read = m.ok("t3_project_read", json!({"projectId":id}));
    assert_eq!(read["title"], "Updated");
    owner.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"t","projectId":id,"title":"Thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    m.denied(
        "t3_project_delete",
        json!({"projectId":id}),
        "invalid_request",
    );
    let deleted = m.ok("t3_project_delete", json!({"projectId":id,"force":true}));
    assert!(deleted["deletedAt"].is_string());
    assert!(root.is_dir());
    m.denied(
        "t3_project_update",
        json!({"projectId":id,"title":"resurrect"}),
        "invalid_request",
    );
    m.denied(
        "t3_project_read",
        json!({"projectId":id}),
        "invalid_request",
    );
    assert_eq!(m.ok("t3_project_list", json!({}))["projects"], json!([]));
}
#[test]
fn project_authority_precedes_disk_effects_and_schema_validation_is_real() {
    let s = support::start(|_| {});
    for access in [
        "read-only",
        "approval-required",
        "auto-accept-edits",
        "auto",
    ] {
        let m = Mcp::new(&s, access);
        let root = s.dir.join(access);
        m.denied(
            "t3_project_create",
            json!({"title":"Denied","workspaceRoot":root,"createWorkspaceRootIfMissing":true}),
            "capability_denied",
        );
        m.denied(
            "t3_project_clone",
            json!({"remoteUrl":"/does/not/exist","destinationPath":root}),
            "capability_denied",
        );
        assert!(!root.exists());
    }
    let m = Mcp::new(&s, "full-access");
    let foreign = s.dir.join("foreign");
    m.denied("t3_project_create",json!({"environmentId":"foreign","title":"Foreign","workspaceRoot":foreign,"createWorkspaceRootIfMissing":true}),"capability_denied");
    assert!(!foreign.exists());
    for args in [
        json!({"projectId":"p","scripts":[{"command":"x"}]}),
        json!({"projectId":"p","projectIcon":{"kind":"lucide","name":"BAD NAME","color":"blue"}}),
        json!({"projectId":"p","defaultThreadEnvMode":"remote"}),
    ] {
        assert_eq!(m.reply("t3_project_update", args)["error"]["code"], -32602);
    }
}
#[test]
fn clone_is_independent_of_registration_and_preserves_occupied_destinations() {
    let s = support::start(|_| {});
    let m = Mcp::new(&s, "full-access");
    let source = s.dir.join("source");
    repo(&source);
    let dest = s.dir.join("clone");
    let cloned = m.ok(
        "t3_project_clone",
        json!({"remoteUrl":source,"destinationPath":dest}),
    );
    assert_eq!(cloned["cwd"], dest.to_string_lossy().as_ref());
    assert!(cloned["repository"].is_null());
    assert_eq!(
        git(&dest, &["rev-parse", "HEAD"]),
        git(&source, &["rev-parse", "HEAD"])
    );
    assert_eq!(m.ok("t3_project_list", json!({}))["projects"], json!([]));
    m.denied(
        "t3_project_clone",
        json!({"remoteUrl":source,"destinationPath":dest}),
        "orchestration_error",
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("README.md")).unwrap(),
        "test\n"
    );
    let p = m.ok(
        "t3_project_create",
        json!({"title":"Clone","workspaceRoot":dest}),
    );
    assert_eq!(p["workspaceRoot"], dest.to_string_lossy().as_ref());
    let direct = s.dir.join("rpc-clone");
    let c = s.connect();
    let rpc = c
        .rpc(
            "sourceControl.cloneRepository",
            json!({"remoteUrl":source,"destinationPath":direct}),
        )
        .unwrap();
    assert_eq!(rpc["cwd"], direct.to_string_lossy().as_ref());
}
#[test]
fn named_project_scaffolds_and_registers_repository() {
    let s = support::start(|_| {});
    let m = Mcp::new(&s, "full-access");
    m.denied(
        "t3_project_create",
        json!({"title":"Invalid","scripts":[]}),
        "invalid_request",
    );
    let named = m.ok(
        "t3_project_create",
        json!({"title":format!("MCP {}",s.dir.file_name().unwrap().to_string_lossy())}),
    );
    let root = Path::new(named["workspaceRoot"].as_str().unwrap());
    assert!(root.join(".git").is_dir());
    assert!(root.join("README.md").is_file());
    assert!(root.join("assets/icon.svg").is_file());
    if named.get("commitError").is_none() {
        assert!(!git(root, &["rev-parse", "HEAD"]).is_empty());
    }
    m.ok("t3_project_delete", json!({"projectId":named["id"]}));
    assert!(root.exists());
    // Fixture trees are retained for private container teardown.
}
#[test]
fn worktree_readers_list_real_checkouts_and_fall_back_after_cleanup() {
    let s = support::start(|_| {});
    let full = Mcp::new(&s, "full-access");
    let read = Mcp::new(&s, "read-only");
    let root = s.dir.join("repo");
    repo(&root);
    let wt = s.dir.join("worktree");
    git(
        &root,
        &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
    );
    let p = full.ok(
        "t3_project_create",
        json!({"title":"Worktrees","workspaceRoot":root}),
    );
    let c = s.connect();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"t","projectId":p["id"],"title":"Thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","branch":"topic","worktreePath":wt})).unwrap();
    let listed = read.ok(
        "t3_worktree_list",
        json!({"threadId":"t","refKind":"local","query":"topic"}),
    );
    assert_eq!(
        listed["refs"][0]["worktreePath"],
        wt.to_string_lossy().as_ref()
    );
    read.denied(
        "t3_worktree_status",
        json!({}),
        "thread_credential_required",
    );
    read.denied("t3_worktree_list", json!({}), "target_required");
    // Simulate a missing worktree while retaining its entire fixture contents.
    std::fs::rename(&wt, root.parent().unwrap().join("retained-topic-worktree")).unwrap();
    let listed = read.ok("t3_worktree_list", json!({"threadId":"t"}));
    assert_eq!(listed["isRepo"], true);
    assert!(listed["refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "main"));
}
