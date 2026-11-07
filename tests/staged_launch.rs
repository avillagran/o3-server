#[path = "support/worktree_removal.rs"]
mod retained_worktrees;
mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_protocol::Connection;
struct NameWriter {
    native: Arc<dyn Provider>,
    seen: Arc<std::sync::Mutex<Vec<Value>>>,
}
impl Provider for NameWriter {
    fn config(&self) -> Vec<Value> {
        self.native.config()
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        controls: tokio::sync::mpsc::Receiver<ProviderControl>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        if input.runtime_mode != "text-only" {
            return self.native.execute(input, out, controls, cancel);
        }
        Box::pin(async move {
            assert!(input.native_session_id.is_none());
            assert!(input.native_mcp.is_none());
            self.seen.lock().unwrap().push(json!({"text":input.text,"selection":input.model_selection,"attachments":input.attachments,"context":input.context}));
            out.send(ProviderEvent::TextDelta(
                input.model_selection["model"].as_str().unwrap().into(),
            ))
            .await
            .map_err(|e| e.to_string())?;
            out.send(ProviderEvent::Completed)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}
const WAIT: Duration = Duration::from_secs(18);
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
struct Fixture {
    s: support::Server,
    c: Connection,
    root: PathBuf,
    removal: std::sync::Arc<retained_worktrees::RetainingExecutor>,
    claude: Arc<o3_server::claude::ClaudeProvider>,
    codex: Arc<o3_server::codex::CodexProvider>,
}
impl Fixture {
    fn writer(&self, name: &str, mode: &str, prefix: &str) -> Arc<std::sync::Mutex<Vec<Value>>> {
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let native = self.s.state.provider.lock().unwrap().clone().unwrap();
        *self.s.state.provider.lock().unwrap() = Some(Arc::new(NameWriter {
            native,
            seen: seen.clone(),
        }));
        let file = self.s.dir.join("settings.json");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        settings["sourceControlWriterModelSelection"] =
            json!({"instanceId":"claudeAgent","model":name});
        settings["branchNamingMode"] = json!(mode);
        settings["branchNamePrefix"] = json!(prefix);
        std::fs::write(file, settings.to_string()).unwrap();
        seen
    }
    fn new(script: &str, sync: bool) -> Self {
        let peer = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace-peer.py");
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
        let installed = Arc::new(std::sync::Mutex::new(None));
        let configure = installed.clone();
        let s = support::start(|s| {
            *configure.lock().unwrap() = Some(retained_worktrees::RetainingExecutor::install(s));
            s.streams.terminals.set_shell("/bin/sh");
            std::fs::write(s.dir.join("settings.json"),json!({"enableAgentBrowserAccess":false,"newWorktreesStartFromOrigin":false,"worktreesDirectory":s.dir.join("managed"),"defaultProjectScripts":[{"id":"install","name":"Install","icon":"configure","command":script,"runOnWorktreeCreate":true,"async":!sync}]}).to_string()).unwrap();
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: Some(claude.clone()),
                codex: codex.clone(),
            }));
        });
        let root = s.dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        std::fs::write(root.join("f"), "base\n").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-m", "initial"]);
        let c = s.connect();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"p","workspaceRoot":root})).unwrap();
        let removal = installed.lock().unwrap().clone().unwrap();
        Self {
            s,
            c,
            root,
            removal,
            claude,
            codex,
        }
    }
    fn request(&self, instance: &str) -> Value {
        json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Launch","modelSelection":{"instanceId":instance,"model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"worktree","baseRef":"main","branch":"feature/launch","startFromOrigin":false},"initialMessage":{"messageId":"m","text":"idle-continuation","attachments":[]}})
    }
    fn read(&self) -> Value {
        t3_protocol::api::orchestration::thread_projection(&self.c, "t").unwrap()
    }
    fn wait(&self, mut condition: impl FnMut(&Value) -> bool) {
        assert!(
            support::wait_for(WAIT, || condition(&self.read())),
            "{}",
            self.read()
        );
    }
    fn work(&self) -> PathBuf {
        PathBuf::from(self.read()["thread"]["worktreePath"].as_str().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let _ = o3_server::terminal::call(
                &self.s.state,
                "terminal.close",
                &json!({"threadId":"t"}),
            )
            .await;
            self.claude.shutdown().await;
            self.codex.shutdown().await;
        });
    }
}
fn subscribe_setup(f: &Fixture) -> support::Stream {
    let spec = t3_protocol::api::streams::worktree_setup_spec("t").unwrap();
    support::subscribe(f.s.connect(), spec.tag(), spec.payload.clone())
}
const GATED:&str="echo started > setup-started; while [ ! -f \"$T3CODE_PROJECT_ROOT/release-setup\" ]; do sleep 0.02; done; printf 'one\\ntwo\\nthree\\nfour\\nfive\\n'; echo ready > setup-ready";
#[test]
fn setup_waits_for_startup_child_prompt_before_writing_the_command() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("echo ready > setup-ready", true);
    let shell = f.s.dir.join("startup-shell");
    std::fs::write(
        &shell,
        "#!/bin/sh\npython3 -c 'input(\"Confirm startup: \")'\nexec /bin/sh -i\n",
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.s.state
        .streams
        .terminals
        .set_shell(shell.to_str().unwrap());
    let mut setup = subscribe_setup(&f);
    f.c.rpc("orchestration.launchThread", f.request("codex"))
        .unwrap();
    setup.until(WAIT, |v| {
        v["stages"].as_array().is_some_and(|stages| {
            stages
                .iter()
                .any(|stage| stage["detail"] == "Waiting for shell startup prompt")
        })
    });
    assert!(!f.work().join("setup-ready").exists());
    assert!(f.read()["providerTurns"].as_array().unwrap().is_empty());
    f.c.rpc(
        "terminal.write",
        json!({"threadId":"t","terminalId":"setup-install","data":"\r"}),
    )
    .unwrap();
    f.wait(|p| p["runs"][0]["status"] == "completed");
    assert!(f.work().join("setup-ready").exists());
}
#[test]
fn deferred_launch_tracks_setup_and_waits_before_both_native_providers() {
    for instance in ["codex", "claudeAgent"] {
        let f = Fixture::new(GATED, true);
        let p = f.request(instance);
        let launch = t3_protocol::api::orchestration::ThreadLaunch::from_json(&p).unwrap();
        let accepted = t3_protocol::api::orchestration::launch_thread(&f.c, &launch).unwrap();
        assert_eq!(accepted["projection"]["runs"][0]["status"], "preparing");
        assert_eq!(
            accepted["projection"]["thread"]["worktreePath"],
            Value::Null
        );
        let rid = accepted["projection"]["runs"][0]["id"].clone();
        let mut setup = subscribe_setup(&f);
        setup.until(WAIT, |v| {
            v["stages"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|v| v["id"] == "setup-script" && v["status"] == "running")
            })
        });
        f.wait(|p| p["thread"]["worktreePath"].is_string());
        let work = f.work();
        assert!(support::wait_for(WAIT, || work
            .join("setup-started")
            .exists()));
        assert_eq!(f.read()["runs"][0]["status"], "preparing");
        assert!(f.read()["providerTurns"].as_array().unwrap().is_empty());
        let replay = f.c.rpc("orchestration.launchThread", p).unwrap();
        assert_eq!(replay["resumed"], true);
        assert_eq!(replay["projection"]["runs"][0]["id"], rid);
        std::fs::write(f.root.join("release-setup"), "").unwrap();
        f.wait(|p| p["runs"][0]["status"] == "completed");
        let values = setup.until(WAIT, |v| v["phase"] == "done");
        let done = values.last().unwrap();
        assert_eq!(
            done["stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == "setup-script")
                .unwrap()["tail"],
            json!(["two", "three", "four", "five"])
        );
        assert!(work.join("setup-ready").exists());
        assert!(work.join("observe-idle-continuation.json").exists());
        assert_eq!(f.read()["runs"].as_array().unwrap().len(), 1);
        assert_eq!(f.read()["runs"][0]["id"], rid);
    }
}
#[test]
fn cancelled_setup_reaps_daemon_rolls_back_owned_checkout_and_allows_retry() {
    let script = r#"python3 -c 'import os,time; os.fork() and os._exit(0); os.setsid(); os.fork() and os._exit(0); os.environ.clear(); [os.close(fd) for fd in (0,1,2)]; open("daemon.pid","w").write(str(os.getpid())); time.sleep(1000)'; while :; do sleep 1; done"#;
    let f = Fixture::new(script, true);
    f.c.rpc("orchestration.launchThread", f.request("codex"))
        .unwrap();
    f.wait(|p| p["thread"]["worktreePath"].is_string());
    let work = f.work();
    assert!(support::wait_for(WAIT, || std::fs::read_to_string(
        work.join("daemon.pid")
    )
    .is_ok_and(|v| v.parse::<i32>().is_ok())));
    let daemon = std::fs::read_to_string(work.join("daemon.pid"))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert_eq!(
        f.c.rpc("worktreeSetup.cancel", json!({"threadId":"t"}))
            .unwrap(),
        json!({"cancelled":true})
    );
    assert!(!Path::new(&format!("/proc/{daemon}")).exists());
    assert!(!work.exists());
    f.removal.assert_retained_worktree(&work);
    assert_eq!(f.read()["runs"][0]["status"], "failed");
    assert!(f.read()["providerTurns"].as_array().unwrap().is_empty());
    assert_eq!(
        f.c.rpc("worktreeSetup.cancel", json!({"threadId":"t"}))
            .unwrap(),
        json!({"cancelled":false})
    );
    let rid = f.read()["runs"][0]["id"].clone();
    f.c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"after-failure","threadId":"t","messageId":"queued","text":"idle-return","attachments":[],"dispatchMode":{"type":"queue_after_active"}})).unwrap();
    let held = f.read();
    let queued = held["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == "queued")
        .unwrap();
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["queueHeld"], true);
    let file = f.s.dir.join("settings.json");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    settings["defaultProjectScripts"][0]["command"] = json!("echo retry-ready > setup-ready");
    std::fs::write(file, settings.to_string()).unwrap();
    f.c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"prepared-run.retry","commandId":"retry","threadId":"t","runId":rid}),
    )
    .unwrap();
    f.wait(|p| {
        p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] == "completed")
            .count()
            == 2
    });
    assert_eq!(f.read()["runs"].as_array().unwrap().len(), 2);
    assert!(f.work().join("setup-ready").exists());
}
#[test]
fn asynchronous_setup_releases_agent_and_finishes_tracking_after_script_exit() {
    let f = Fixture::new(GATED, false);
    f.c.rpc("orchestration.launchThread", f.request("codex"))
        .unwrap();
    f.wait(|p| p["runs"][0]["status"] == "completed");
    assert!(!f.work().join("setup-ready").exists());
    assert_eq!(
        f.c.rpc("worktreeSetup.cancel", json!({"threadId":"t"}))
            .unwrap(),
        json!({"cancelled":false})
    );
    let mut setup = subscribe_setup(&f);
    assert_eq!(setup.next(WAIT)["phase"], "running");
    std::fs::write(f.root.join("release-setup"), "").unwrap();
    setup.until(WAIT, |v| v["phase"] == "done");
    assert!(f.work().join("setup-ready").exists());
}

#[test]
fn failed_sync_setup_keeps_terminal_and_checkout_for_an_in_place_retry() {
    let script="if [ ! -f \"$T3CODE_PROJECT_ROOT/retry-ready\" ]; then echo inspect-this-failure; exit 7; fi; echo recovered > recovered";
    let f = Fixture::new(script, true);
    f.c.rpc("orchestration.launchThread", f.request("codex"))
        .unwrap();
    f.wait(|p| p["runs"][0]["status"] == "failed");
    let work = f.work();
    assert!(work.join(".git").exists());
    assert!(f.read()["runs"][0]["error"]
        .as_str()
        .unwrap()
        .contains("exited with 7"));
    let mut terminal_stream = support::subscribe(
        f.s.connect(),
        "terminal.attach",
        json!({"threadId":"t","terminalId":"setup-install"}),
    );
    let terminal = terminal_stream.next(WAIT)["snapshot"].clone();
    assert_eq!(terminal["status"], "running");
    assert!(terminal["history"]
        .as_str()
        .unwrap()
        .contains("inspect-this-failure"));
    std::fs::write(work.join("keep-local"), "preserved").unwrap();
    std::fs::write(f.root.join("retry-ready"), "").unwrap();
    let rid = f.read()["runs"][0]["id"].clone();
    f.c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"prepared-run.retry","commandId":"retry","threadId":"t","runId":rid}),
    )
    .unwrap();
    f.wait(|p| p["runs"][0]["status"] == "completed");
    assert_eq!(f.work(), work);
    assert_eq!(
        std::fs::read_to_string(work.join("keep-local")).unwrap(),
        "preserved"
    );
    assert!(work.join("recovered").exists());
    assert_eq!(f.read()["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn scratch_launch_normalizes_worktree_to_a_managed_non_git_folder_without_reclaiming_on_replay() {
    let f = Fixture::new(GATED, true);
    let scratch = f.s.dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    f.c.rpc("projects.mutate",json!({"type":"project.update","commandId":"scratch","projectId":"p","workspaceRoot":scratch})).unwrap();
    let request = f.request("codex");
    let accepted =
        f.c.rpc("orchestration.launchThread", request.clone())
            .unwrap();
    let path = PathBuf::from(
        accepted["projection"]["thread"]["worktreePath"]
            .as_str()
            .unwrap(),
    );
    assert!(path.starts_with(&scratch));
    assert!(!path.join(".git").exists());
    assert_eq!(accepted["projection"]["thread"]["branch"], Value::Null);
    f.wait(|p| p["runs"][0]["status"] == "completed");
    assert!(path.join("observe-idle-continuation.json").exists());
    assert!(!path.join("setup-ready").exists());
    let mut setup = subscribe_setup(&f);
    assert_eq!(setup.next(WAIT), Value::Null);
    std::fs::write(scratch.join("release-setup"), "").unwrap();
    assert!(support::wait_for(WAIT, || path
        .join("setup-ready")
        .exists()));
    let replay = f.c.rpc("orchestration.launchThread", request).unwrap();
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["projection"]["thread"]["worktreePath"], json!(path));
    assert_eq!(
        std::fs::read_dir(&scratch)
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().unwrap().is_dir())
            .count(),
        1
    );
}

#[test]
fn temporary_branch_names_follow_naming_policy_and_cancel_removes_the_renamed_ref() {
    for (mode, prefix, raw, collision, expected) in [
        (
            "semantic",
            "",
            "  Ádd Régions  ",
            false,
            "feature/add-regions",
        ),
        (
            "static",
            "Team//Fix !",
            "Hot Fix",
            false,
            "team/fix/hot-fix",
        ),
        (
            "custom",
            "ignored",
            "Release/EXACT_v1",
            false,
            "Release/EXACT_v1",
        ),
        ("semantic", "", "add-regions", true, "feature/add-regions-2"),
    ] {
        let f = Fixture::new(GATED, true);
        let seen = f.writer(raw, mode, prefix);
        if collision {
            git(&f.root, &["branch", "feature/add-regions"]);
        }
        let mut request = f.request("codex");
        request["workspaceStrategy"]
            .as_object_mut()
            .unwrap()
            .remove("branch");
        request["initialMessage"]["context"] = json!({"proof":"branch-context"});
        f.c.rpc("orchestration.launchThread", request).unwrap();
        f.wait(|p| p["thread"]["branch"] == expected);
        let work = f.work();
        assert_eq!(git(&work, &["symbolic-ref", "--short", "HEAD"]), expected);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(
            seen.lock().unwrap()[0]["context"],
            json!({"proof":"branch-context"})
        );
        assert_eq!(
            t3_protocol::api::worktree_setup::cancel(&f.c, "t").unwrap(),
            json!({"cancelled":true})
        );
        assert!(!work.exists());
        f.removal.assert_retained_worktree(&work);
        assert!(!std::process::Command::new("git")
            .current_dir(&f.root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{expected}")
            ])
            .status()
            .unwrap()
            .success());
        if collision {
            assert_eq!(
                git(
                    &f.root,
                    &["show-ref", "--verify", "refs/heads/feature/add-regions"]
                )
                .split_whitespace()
                .count(),
                2
            );
        }
    }
}

#[test]
fn rename_metadata_failure_still_cleans_the_physically_renamed_owned_branch() {
    let f = Fixture::new(GATED, true);
    f.writer("recover-rename", "semantic", "");
    f.s.state.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_rename BEFORE UPDATE ON records WHEN NEW.kind='thread' AND json_extract(NEW.body,'$.branch')='feature/recover-rename' BEGIN SELECT RAISE(ABORT,'injected rename persistence failure'); END;").unwrap();
    let mut request = f.request("codex");
    request["workspaceStrategy"]
        .as_object_mut()
        .unwrap()
        .remove("branch");
    f.c.rpc("orchestration.launchThread", request).unwrap();
    f.wait(|p| p["thread"]["worktreePath"].is_string());
    let work = f.work();
    let original = f.read()["thread"]["branch"].as_str().unwrap().to_owned();
    assert!(support::wait_for(WAIT, || {
        let output = std::process::Command::new("git")
            .current_dir(&work)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim() == "feature/recover-rename"
    }));
    assert_eq!(
        t3_protocol::api::worktree_setup::cancel(&f.c, "t").unwrap(),
        json!({"cancelled":true})
    );
    assert!(!work.exists());
    f.removal.assert_retained_worktree(&work);
    for branch in [original.as_str(), "feature/recover-rename"] {
        assert!(!std::process::Command::new("git")
            .current_dir(&f.root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ])
            .status()
            .unwrap()
            .success());
    }
}

#[test]
fn cancellation_finishes_with_a_locked_checkout_and_cleanup_retries_before_admitting_retry() {
    let f = Fixture::new(GATED, true);
    f.c.rpc("orchestration.launchThread", f.request("codex"))
        .unwrap();
    f.wait(|p| p["thread"]["worktreePath"].is_string());
    let work = f.work();
    assert!(support::wait_for(WAIT, || work
        .join("setup-started")
        .exists()));
    git(&f.root, &["worktree", "lock", work.to_str().unwrap()]);
    assert_eq!(
        t3_protocol::api::worktree_setup::cancel(&f.c, "t").unwrap(),
        json!({"cancelled":true})
    );
    assert!(work.exists());
    let rid = f.read()["runs"][0]["id"].clone();
    assert!(f
        .c
        .rpc(
            "orchestration.dispatchCommand",
            json!({"type":"prepared-run.retry","commandId":"retry","threadId":"t","runId":rid})
        )
        .is_err());
    git(&f.root, &["worktree", "unlock", work.to_str().unwrap()]);
    // The filesystem disappears before cleanup_pending is acknowledged. Retry
    // must wait for the same durable predicate used by prepared-run.retry.
    assert!(support::wait_for(WAIT, || {
        !work.exists() && f.s.state.db.lock().unwrap().query_row("SELECT NOT EXISTS(SELECT 1 FROM thread_launches WHERE thread_id='t' AND ((completed=0 AND released=0) OR cleanup_pending=1))",[],|r|r.get::<_,bool>(0)).unwrap()
    }));
    std::fs::write(f.root.join("release-setup"), "").unwrap();
    f.c.rpc(
        "orchestration.dispatchCommand",
        json!({"type":"prepared-run.retry","commandId":"retry","threadId":"t","runId":rid}),
    )
    .unwrap();
    f.wait(|p| p["runs"][0]["status"] == "completed");
}

#[test]
fn branch_generation_and_formatting_use_project_overrides_inside_the_worktree() {
    let f = Fixture::new(GATED, true);
    let seen = f.writer("wrong-machine-default", "semantic", "");
    let file = f.s.dir.join("settings.json");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    settings["projectSettingsOverrides"] = json!({"p":{"sourceControlWriterModelSelection":{"instanceId":"claudeAgent","model":"Project/EXACT"},"branchNamingMode":"custom"}});
    std::fs::write(file, settings.to_string()).unwrap();
    let mut request = f.request("codex");
    request["workspaceStrategy"]
        .as_object_mut()
        .unwrap()
        .remove("branch");
    f.c.rpc("orchestration.launchThread", request).unwrap();
    f.wait(|p| p["thread"]["branch"] == "Project/EXACT");
    assert_eq!(
        seen.lock().unwrap()[0]["selection"]["model"],
        "Project/EXACT"
    );
    assert_eq!(
        t3_protocol::api::worktree_setup::cancel(&f.c, "t").unwrap(),
        json!({"cancelled":true})
    );
}
