mod common;
#[path = "support/worktree_removal.rs"]
pub mod retained_worktrees;
mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
const WAIT: Duration = Duration::from_secs(20);
struct Catalog;
impl Provider for Catalog {
    fn config(&self) -> Vec<Value> {
        vec![
            json!({"instanceId":"claudeAgent","driver":"claudeAgent","enabled":true,"installed":true,"status":"ready","auth":{"status":"authenticated"},"supportedRuntimeModes":["approval-required","auto-accept-edits","full-access"],"usageLimits":{"windows":[{"id":"session","usedPercent":100}]}}),
            json!({"instanceId":"codex","driver":"codex","enabled":true,"installed":true,"status":"ready","auth":{"status":"authenticated"},"supportedRuntimeModes":["approval-required","full-access"]}),
        ]
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        controls: tokio::sync::mpsc::Receiver<ProviderControl>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        o3_server::provider::ScriptedProvider.execute(input, out, controls, cancel)
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
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
fn project(s: &support::Server) -> PathBuf {
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["commit", "--allow-empty", "-m", "first"]);
    s.connect().rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Project","workspaceRoot":root})).unwrap();
    root
}
struct Mcp<'a> {
    s: &'a support::Server,
    token: String,
    session: String,
}
impl<'a> Mcp<'a> {
    fn new(s: &'a support::Server, access: &str) -> Self {
        let token = s
            .state
            .issue_mcp_session(access, 120_000, "launch test")
            .unwrap();
        let auth = format!("Bearer {token}");
        let (code,headers,body)=common::http(&s.base,"POST","/mcp",&[("authorization",&auth),("content-type","application/json"),("accept","application/json")],json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"launch-test","version":"1"}}}).to_string().as_bytes());
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
        let session = headers
            .lines()
            .find_map(|l| l.strip_prefix("mcp-session-id:").map(str::trim))
            .unwrap()
            .into();
        Self { s, token, session }
    }
    fn reply(&self, p: Value, mode: Option<&str>) -> Value {
        let auth = format!("Bearer {}", self.token);
        let mut headers = vec![
            ("authorization", auth.as_str()),
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("mcp-session-id", self.session.as_str()),
        ];
        if let Some(mode) = mode {
            headers.push(("t3-mode-limit", mode));
        }
        let (code,_,body)=common::http(&self.s.base,"POST","/mcp",&headers,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"t3_thread_launch","arguments":p}}).to_string().as_bytes());
        assert_eq!(code, 200);
        serde_json::from_slice(&body).unwrap()
    }
    fn ok(&self, p: Value) -> Value {
        let v = self.reply(p, None);
        assert_eq!(v["result"]["isError"], false, "{v}");
        v["result"]["structuredContent"].clone()
    }
    fn denied(&self, p: Value, mode: Option<&str>, code: &str) {
        let v = self.reply(p, mode);
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["structuredContent"]["code"], code, "{v}");
    }
}
fn projection(s: &support::Server, tid: &Value) -> Value {
    t3_protocol::api::orchestration::thread_projection(&s.connect(), tid.as_str().unwrap()).unwrap()
}
#[test]
fn external_launch_inherits_project_defaults_falls_back_and_replays_the_saved_model() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    project(&s);
    let c = s.connect();
    c.rpc("server.updateSettings",json!({"patch":{"defaultModelSelection":{"instanceId":"claudeAgent","model":"exhausted"},"fallbackModelSelection":{"instanceId":"codex","model":"fallback","options":[{"id":"future-option","value":"preserved"}]},"defaultRuntimeMode":"full-access"}})).unwrap();
    let m = Mcp::new(&s, "auto");
    let request = json!({"title":"  Launch  ","projectId":"p","clientRequestId":"é".repeat(256)});
    let first = m.ok(request.clone());
    assert_eq!(first["modelSelection"]["model"], "fallback");
    assert!(first["runId"].is_null());
    assert!(first["link"]
        .as_str()
        .unwrap()
        .starts_with("<t3-thread://v2/"));
    let p = projection(&s, &first["threadId"]);
    assert_eq!(p["thread"]["runtimeMode"], "approval-required");
    assert_eq!(p["thread"]["title"], "Launch");
    assert_eq!(p["thread"]["createdBy"], "agent");
    assert_eq!(p["thread"]["creationSource"], "mcp");
    c.rpc(
        "server.updateSettings",
        json!({"patch":{"defaultModelSelection":null,"fallbackModelSelection":null}}),
    )
    .unwrap();
    assert_eq!(m.ok(request.clone()), first);
    m.denied(
        json!({"projectId":"p","title":"Missing default"}),
        None,
        "invalid_request",
    );
    let other = Mcp::new(&s, "auto");
    let mut explicit = request;
    explicit["modelSelection"] = first["modelSelection"].clone();
    assert_ne!(other.ok(explicit)["threadId"], first["threadId"]);
}

#[test]
fn model_aliases_options_and_effective_project_defaults_preserve_prompt_text() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    project(&s);
    let c = s.connect();
    let m = Mcp::new(&s, "full-access");
    c.rpc("server.updateSettings",json!({"patch":{"defaultModelSelection":{"instanceId":"codex","model":"global","options":[{"id":"global-only","value":true}]}}})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"project-model","projectId":"p","defaultModelSelection":{"instanceId":"codex","model":"project"}})).unwrap();
    let from_project = m.ok(json!({"title":"Default","projectId":"p"}));
    assert_eq!(from_project["modelSelection"]["model"], "project");
    assert!(from_project["modelSelection"]["options"].is_null());
    c.rpc("server.updateSettings",json!({"patch":{"projectSettingsOverrides":{"p":{"defaultModelSelection":{"instanceId":"codex","model":"override"},"defaultRuntimeMode":"approval-required"}}}})).unwrap();
    let from_override = m.ok(json!({"title":"Override","projectId":"p"}));
    assert_eq!(from_override["modelSelection"]["model"], "override");
    assert_eq!(
        projection(&s, &from_override["threadId"])["thread"]["runtimeMode"],
        "approval-required"
    );
    let prompt = "  preserve this prompt\n ";
    let alias=m.ok(json!({"projectId":" p ","title":" Alias ","message":prompt,"modelSelection":{"provider":" codex ","model":" test ","options":{" effort ":" high ","futureFlag":true,"ignored":12}},"workspaceStrategy":{"type":"root","ignored":"field"}}));
    let p = projection(&s, &alias["threadId"]);
    assert_eq!(p["messages"][0]["text"], prompt);
    assert_eq!(
        p["thread"]["modelSelection"],
        json!({"instanceId":"codex","model":"test","options":[{"id":"effort","value":"high"},{"id":"futureFlag","value":true}]})
    );
}
#[test]
fn launch_rechecks_replay_modes_and_read_only_requests_have_no_scratch_effects() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    project(&s);
    let model = json!({"instanceId":"codex","model":"test"});
    let m = Mcp::new(&s, "full-access");
    let request = json!({"projectId":"p","title":"Mode","modelSelection":model,"clientRequestId":"mode","runtimeMode":"full-access"});
    let first = m.ok(request.clone());
    let before: i64 = s
        .state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    m.denied(
        request.clone(),
        Some("approval-required/default"),
        "runtime_mode_escalation_denied",
    );
    let mut inherited = request.clone();
    inherited.as_object_mut().unwrap().remove("runtimeMode");
    m.denied(
        inherited,
        Some("approval-required/default"),
        "runtime_mode_escalation_denied",
    );
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM receipts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        before
    );
    assert_eq!(m.ok(request)["threadId"], first["threadId"]);
    m.denied(json!({"projectId":"p","title":"Interaction","modelSelection":model,"interactionMode":"default"}),Some("full-access/plan"),"interaction_mode_escalation_denied");
    let read = Mcp::new(&s, "read-only");
    read.denied(
        json!({"title":"No","scratch":true,"modelSelection":model}),
        None,
        "capability_denied",
    );
    assert!(!s.dir.join("scratch").exists());
}
#[test]
fn worktree_preflight_rejects_outside_and_missing_base_before_claim_and_replays_after_base_deletion(
) {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    let root = project(&s);
    let m = Mcp::new(&s, "full-access");
    let base = json!({"title":"Worktree","projectId":"p","modelSelection":{"instanceId":"codex","model":"test"}});
    let outside = s.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mut p = base.clone();
    p["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":outside});
    m.denied(p, None, "invalid_request");
    let mut missing = base.clone();
    missing["workspaceStrategy"] =
        json!({"type":"worktree","baseRef":"missing","startFromOrigin":false});
    m.denied(missing, None, "invalid_request");
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM records WHERE kind='thread'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let existing = s.dir.join("existing");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "existing",
            existing.to_str().unwrap(),
        ],
    );
    for path in [&root, &existing] {
        let mut p = base.clone();
        p["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":path});
        let r = m.ok(p);
        assert_eq!(
            projection(&s, &r["threadId"])["thread"]["worktreePath"],
            json!(path)
        );
    }
    std::fs::rename(&existing, root.join("retained-existing-worktree")).unwrap();
    std::fs::create_dir_all(&existing).unwrap();
    let mut stale = base.clone();
    stale["workspaceStrategy"] = json!({"type":"existing_worktree","worktreePath":existing});
    m.denied(stale, None, "invalid_request");
    git(&root, &["branch", "base"]);
    let mut p = base;
    p["clientRequestId"] = json!("worktree");
    p["workspaceStrategy"] = json!({"type":"worktree","baseRef":"base","startFromOrigin":false});
    let first = m.ok(p.clone());
    assert!(support::wait_for(WAIT, || projection(
        &s,
        &first["threadId"]
    )["thread"]["worktreePath"]
        .is_string()));
    git(&root, &["branch", "-D", "base"]);
    assert_eq!(m.ok(p)["threadId"], first["threadId"]);
}
#[test]
fn scratch_bootstraps_once_preserves_icon_and_allocates_distinct_non_git_folders() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    let m = Mcp::new(&s, "approval-required");
    let model = json!({"instanceId":"codex","model":"test"});
    m.denied(
        json!({"title":"Bad","scratch":true,"projectId":"p","modelSelection":model}),
        None,
        "invalid_request",
    );
    assert!(!s.dir.join("scratch").exists());
    let p = json!({"title":"Scratch","scratch":true,"clientRequestId":"scratch","modelSelection":model});
    let first = m.ok(p.clone());
    let path = projection(&s, &first["threadId"])["thread"]["worktreePath"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(Path::new(&path).is_dir());
    assert!(!Path::new(&path).join(".git").exists());
    assert_eq!(m.ok(p), first);
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.update","commandId":"icon","projectId":first["projectId"],"projectIcon":{"kind":"emoji","emoji":"x"}})).unwrap();
    let second = m.ok(json!({"title":"Scratch","scratch":true,"modelSelection":model}));
    assert_eq!(first["projectId"], second["projectId"]);
    assert_ne!(
        projection(&s, &second["threadId"])["thread"]["worktreePath"],
        json!(path)
    );
    let ensured = c.rpc("projects.ensureScratch", json!({})).unwrap();
    assert_eq!(ensured["projectId"], first["projectId"]);
    let projects = c.request_json("GET", "/api/projects", None, false).unwrap();
    assert_eq!(projects["projects"][0]["projectIcon"]["emoji"], "x");
    let repository_profile =
        start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    git(&repository_profile.dir, &["init", "-b", "main"]);
    let blocked = Mcp::new(&repository_profile, "approval-required");
    blocked.denied(
        json!({"title":"Unavailable","scratch":true,"modelSelection":model}),
        None,
        "orchestration_error",
    );
    assert!(!repository_profile.dir.join("scratch").exists());
    let error = support::raw_exit(&repository_profile, "projects.ensureScratch", json!({}));
    assert_eq!(
        error["exit"]["cause"][0]["error"]["_tag"],
        "OrchestrationDispatchCommandError"
    );
}

#[test]
fn launch_claims_pending_uploads_before_setup_and_both_native_transports_read_copies() {
    for instance in ["claudeAgent", "codex"] {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/input/peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let s = start_recording_cleanup(|s| {
            s.streams.terminals.set_shell("/bin/sh");
            std::fs::write(s.dir.join("settings.json"),json!({"enableAgentBrowserAccess":false,"worktreesDirectory":s.dir.join("managed"),"newWorktreesStartFromOrigin":false,"defaultProjectScripts":[{"id":"setup","name":"Setup","icon":"configure","runOnWorktreeCreate":true,"async":false,"command":"echo started > setup-started; while [ ! -f \"$T3CODE_PROJECT_ROOT/release\" ]; do sleep 0.02; done; echo ready > setup-ready"}]}).to_string()).unwrap();
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let root = project(&s);
        let owner = s.connect();
        let m = Mcp::new(&s, "full-access");
        let image = owner
            .upload_attachment(
                "pixel.png",
                "image/png",
                8,
                std::io::Cursor::new(b"\x89PNG\r\n\x1a\n"),
            )
            .unwrap();
        let file = owner
            .upload_attachment(
                "notes.txt",
                "text/plain",
                10,
                std::io::Cursor::new(b"file-proof"),
            )
            .unwrap();
        let request = json!({"title":"Input","projectId":"p","clientRequestId":"inputs","modelSelection":{"instanceId":instance,"model":"test","options":[{"id":"effort","value":"high"},{"id":"fastMode","value":true}]},"workspaceStrategy":{"type":"worktree","baseRef":"main","branch":"proof/input","startFromOrigin":false},"message":"input-proof context-proof","attachments":[image,file]});
        let accepted = m.ok(request.clone());
        assert_eq!(accepted["status"], "preparing");
        assert!(support::wait_for(WAIT, || projection(
            &s,
            &accepted["threadId"]
        )["thread"]["worktreePath"]
            .is_string()));
        let p = projection(&s, &accepted["threadId"]);
        let work = PathBuf::from(p["thread"]["worktreePath"].as_str().unwrap());
        assert!(support::wait_for(WAIT, || work
            .join("setup-started")
            .exists()));
        assert!(p["providerTurns"].as_array().unwrap().is_empty());
        assert_eq!(p["messages"][0]["createdBy"], "agent");
        assert_eq!(p["messages"][0]["creationSource"], "mcp");
        for attachment in p["messages"][0]["attachments"].as_array().unwrap() {
            use std::os::unix::fs::PermissionsExt;
            let id = attachment["id"].as_str().unwrap();
            assert!(!id.starts_with("pending-"));
            let path = o3_server::assets::attachment_path(&s.state, id).unwrap();
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for attachment in [&image, &file] {
            owner
                .rpc(
                    "attachments.delete",
                    json!({"attachmentId":attachment["id"]}),
                )
                .unwrap();
            assert!(o3_server::assets::attachment_path(
                &s.state,
                attachment["id"].as_str().unwrap()
            )
            .is_none());
        }
        assert_eq!(m.ok(request)["runId"], accepted["runId"]);
        std::fs::write(root.join("release"), "").unwrap();
        assert!(support::wait_for(WAIT, || {
            let p = projection(&s, &accepted["threadId"]);
            matches!(
                p["runs"][0]["status"].as_str(),
                Some("completed" | "failed")
            )
        }));
        let p = projection(&s, &accepted["threadId"]);
        assert_eq!(p["runs"][0]["status"], "completed", "{p}");
        assert!(p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["text"] == "verified-input"));
        assert_eq!(p["runs"].as_array().unwrap().len(), 1);
        assert!(work.join("setup-ready").exists());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let _ = o3_server::terminal::call(
                &s.state,
                "terminal.close",
                &json!({"threadId":accepted["threadId"]}),
            )
            .await;
            claude.shutdown().await;
            codex.shutdown().await;
        });
    }
}
#[test]
fn failed_attachment_or_message_admission_releases_copies_and_retry_keeps_one_message() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    project(&s);
    let owner = s.connect();
    let m = Mcp::new(&s, "full-access");
    let file = owner
        .upload_attachment(
            "notes.txt",
            "text/plain",
            10,
            std::io::Cursor::new(b"file-proof"),
        )
        .unwrap();
    let files = || {
        let mut files = std::fs::read_dir(s.dir.join("attachments"))
            .unwrap()
            .map(|p| p.unwrap().file_name())
            .collect::<Vec<_>>();
        files.sort();
        files
    };
    let pending = files();
    let base = json!({"title":"Copies","projectId":"p","clientRequestId":"copies","modelSelection":{"instanceId":"codex","model":"test"},"message":"hello","attachments":[file]});
    let mut duplicate = base.clone();
    duplicate["attachments"] = json!([file, file]);
    m.denied(duplicate, None, "orchestration_error");
    assert_eq!(files(), pending);
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM records WHERE kind='thread'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_mcp_initial BEFORE INSERT ON records WHEN NEW.kind='runs' BEGIN SELECT RAISE(ABORT,'reject initial admission'); END;").unwrap();
    m.denied(base.clone(), None, "orchestration_error");
    assert_eq!(files(), pending);
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM records WHERE kind='thread'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    s.state
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_mcp_initial")
        .unwrap();
    let accepted = m.ok(base.clone());
    assert_eq!(m.ok(base)["runId"], accepted["runId"]);
    assert_eq!(
        projection(&s, &accepted["threadId"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(files().len() > pending.len());
}

#[test]
fn native_launch_inherits_caller_model_modes_and_project_but_uses_project_root() {
    for instance in ["claudeAgent", "codex"] {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp-launch-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let s = start_recording_cleanup(|s| {
            std::fs::write(
                s.dir.join("settings.json"),
                json!({"enableAgentBrowserAccess":false}).to_string(),
            )
            .unwrap();
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let root = project(&s);
        let work = s.dir.join("parent");
        git(
            &root,
            &["worktree", "add", "-b", "caller", work.to_str().unwrap()],
        );
        let owner = s.connect();
        owner.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"parent","threadId":"parent","projectId":"p","title":"Parent","modelSelection":{"instanceId":instance,"model":"test","options":[{"id":"effort","value":"high"}]},"runtimeMode":"approval-required","interactionMode":"plan","branch":"caller","worktreePath":work})).unwrap();
        owner.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"start","threadId":"parent","messageId":"parent-message","text":"launch-parent","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        let publication = work.join("native-launch.json");
        let mut published = None;
        assert!(
            support::wait_for(WAIT, || {
                let bytes = match std::fs::read(&publication) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
                    Err(error) => panic!("Unable to read native launch fixture: {error}"),
                };
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => {
                        published = Some(value);
                        true
                    }
                    Err(error) if error.is_eof() => false,
                    Err(error) => panic!("Invalid native launch fixture JSON: {error}"),
                }
            }),
            "{}",
            projection(&s, &json!("parent"))
        );
        let result = published.expect("The existing deadline requires complete fixture JSON");
        let tid = &result["threadId"];
        assert!(
            support::wait_for(WAIT, || projection(&s, tid)["runs"][0]["status"]
                == "completed"),
            "{}",
            projection(&s, tid)
        );
        let p = projection(&s, tid);
        assert_eq!(p["thread"]["projectId"], "p");
        assert_eq!(p["thread"]["modelSelection"]["instanceId"], instance);
        assert_eq!(p["thread"]["modelSelection"]["model"], "test");
        assert_eq!(p["thread"]["runtimeMode"], "approval-required");
        assert_eq!(p["thread"]["interactionMode"], "plan");
        assert!(p["thread"]["worktreePath"].is_null());
        assert_eq!(p["messages"][0]["senderThreadId"], "parent");
        assert_eq!(p["messages"][0]["createdBy"], "agent");
        assert!(p["thread"]["lineage"]["parentThreadId"].is_null());
        assert_eq!(
            s.state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM records WHERE kind='thread'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(root.join("native-child-proof")).unwrap(),
            "ready"
        );
        assert!(!work.join("native-child-proof").exists());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            claude.shutdown().await;
            codex.shutdown().await;
        });
    }
}

#[test]
fn authority_changed_by_claim_commit_refuses_message_admission_and_retry_recovers_identity() {
    for revoke in [false, true] {
        let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
        project(&s);
        let m = Mcp::new(&s, "full-access");
        let claims = s.state.mcp_session(&m.token).unwrap();
        {
            let db = s.state.db.lock().unwrap();
            db.execute_batch("CREATE TABLE launch_authority_test(session TEXT,revoke INTEGER);")
                .unwrap();
            db.execute(
                "INSERT INTO launch_authority_test VALUES(?1,?2)",
                rusqlite::params![claims["sid"].as_str().unwrap(), revoke],
            )
            .unwrap();
            db.execute_batch("CREATE TRIGGER lower_launch_authority AFTER INSERT ON receipts WHEN NEW.command_id LIKE 'mcp:launch:%' BEGIN UPDATE auth_sessions SET client=json_set(client,'$.runtimeModeCeiling','approval-required'),revoked_at=CASE WHEN (SELECT revoke FROM launch_authority_test) THEN 1 ELSE revoked_at END WHERE session_id=(SELECT session FROM launch_authority_test); END;").unwrap();
        }
        let request = json!({"title":"Authority","projectId":"p","clientRequestId":"authority","runtimeMode":"full-access","modelSelection":{"instanceId":"codex","model":"test"},"message":"run after approval restored"});
        m.denied(
            request.clone(),
            None,
            if revoke {
                "capability_denied"
            } else {
                "runtime_mode_escalation_denied"
            },
        );
        let tid: String = {
            let db = s.state.db.lock().unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM records WHERE kind IN ('runs','messages')",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM thread_launches", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            db.query_row("SELECT id FROM records WHERE kind='thread'", [], |r| {
                r.get(0)
            })
            .unwrap()
        };
        {
            let db = s.state.db.lock().unwrap();
            db.execute_batch("DROP TRIGGER lower_launch_authority")
                .unwrap();
            db.execute("UPDATE auth_sessions SET client=json_set(client,'$.runtimeModeCeiling','full-access'),revoked_at=NULL WHERE session_id=?",[claims["sid"].as_str().unwrap()]).unwrap();
        }
        let resumed = m.ok(request.clone());
        assert_eq!(resumed["threadId"], tid);
        assert_eq!(m.ok(request)["runId"], resumed["runId"]);
        assert_eq!(
            projection(&s, &resumed["threadId"])["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn remote_only_base_and_git_commit_search_are_accepted_and_prepared() {
    let s = start_recording_cleanup(|s| *s.provider.lock().unwrap() = Some(Arc::new(Catalog)));
    let root = project(&s);
    let m = Mcp::new(&s, "full-access");
    let remote = s.dir.join("origin.git");
    git(
        &s.dir,
        &[
            "clone",
            "--bare",
            root.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&remote, &["branch", "feature/remote-only", "main"]);
    for (base, fetch) in [("feature/remote-only", true), (":/first", false)] {
        let launched=m.ok(json!({"title":"Base","projectId":"p","modelSelection":{"instanceId":"codex","model":"test"},"workspaceStrategy":{"type":"worktree","baseRef":base,"startFromOrigin":fetch}}));
        assert!(support::wait_for(WAIT, || projection(
            &s,
            &launched["threadId"]
        )["thread"]["worktreePath"]
            .is_string()));
        let p = projection(&s, &launched["threadId"]);
        let path = Path::new(p["thread"]["worktreePath"].as_str().unwrap());
        assert!(path.join(".git").is_file());
        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(&root, &["rev-parse", "main"])
        );
    }
    assert!(
        git(
            &root,
            &[
                "rev-parse",
                "--verify",
                "refs/remotes/origin/feature/remote-only"
            ]
        )
        .len()
            == 40
    );
}

fn start_recording_cleanup(
    configure: impl FnOnce(&std::sync::Arc<o3_server::store::State>),
) -> support::Server {
    support::start(|state| {
        retained_worktrees::RetainingExecutor::install(state);
        configure(state);
    })
}
