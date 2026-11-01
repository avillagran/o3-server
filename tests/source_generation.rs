mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};
struct Writer(Arc<Mutex<Vec<Value>>>);
impl Provider for Writer {
    fn config(&self) -> Vec<Value> {
        [ ("codex","writer"),("claudeAgent","reviewer")].into_iter().map(|(id,model)|json!({"instanceId":id,"driver":id,"enabled":true,"installed":true,"status":"ready","models":[{"slug":model,"name":model,"capabilities":{"optionDescriptors":[{"id":"reasoningEffort","type":"select","options":[{"id":"low"}]}]}}]})).collect()
    }

    fn execute<'a>(
        &'a self,
        input: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        _: tokio::sync::mpsc::Receiver<ProviderControl>,
        _: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(json!({"text":input.text,"selection":input.model_selection,"native":input.native_session_id}));
            if input.model_selection["model"] == "fail" {
                return Err("generation unavailable".into());
            }
            let text = if input.text.starts_with("Write a concise review") {
                "Inspect error paths and test the failure case."
            } else if input.text.starts_with("Write only a short Git") {
                "reader-generation"
            } else {
                "Apply reader settings\n\nCover the generated path."
            };
            out.send(ProviderEvent::TextDelta(text.into()))
                .await
                .unwrap();
            out.send(ProviderEvent::Completed).await.unwrap();
            Ok(())
        })
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
#[test]
fn real_settings_and_action_readers_cover_commit_review_and_failure_paths() {
    let bin = std::env::temp_dir().join(format!("o3-scm-bin-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("gh"),
        "#!/bin/sh\nif [ \"$1 $2\" = 'pr diff' ]; then echo '+ review change'; elif [ \"$1 $2\" = 'pr list' ]; then echo '[]'; elif [ \"$1 $2\" = 'pr create' ]; then echo 'https://github.com/example/repo/pull/1'; else exit 1; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    let seen = Arc::new(Mutex::new(vec![]));
    let s = support::start(|s| *s.provider.lock().unwrap() = Some(Arc::new(Writer(seen.clone()))));
    let c = s.connect();
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Writer Test"]);
    git(&root, &["config", "user.email", "writer@example.invalid"]);
    std::fs::write(root.join("a"), "initial").unwrap();
    git(&root, &["add", "a"]);
    git(&root, &["commit", "-m", "initial"]);
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"SCM","workspaceRoot":root})).unwrap();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    for (key, value) in [
        (
            "sourceControlWriterModelSelection",
            json!({"instanceId":"claudeAgent","model":"reviewer","options":[{"id":"reasoningEffort","value":"low"}]}),
        ),
        (
            "reviewGuideModelSelection",
            json!({"instanceId":"claudeAgent","model":"reviewer"}),
        ),
        ("reviewGuideInstructions", json!("Check error paths")),
        ("reviewGuidesEnabled", json!(true)),
        ("sourceControlWritingStyle.mode", json!("custom")),
        (
            "sourceControlWritingStyle.customInstructions",
            json!("Use imperative verbs"),
        ),
    ] {
        let patch =
            t3_protocol::native_settings::patch(&json!({"key":key,"value":value}), Some(&config))
                .unwrap();
        let saved = c.rpc("server.updateSettings", patch).unwrap();
        assert!(t3_protocol::native_settings::snapshot(&saved)["fields"].is_array());
    }
    let run = |id: &str, action: &str, feature: bool, message: Option<&str>| {
        let mut events = vec![];
        let stop = std::cell::Cell::new(false);
        t3_protocol::api::streams::subscribe_stacked_action(
            &c,
            &t3_protocol::api::streams::StackedAction {
                action_id: id.into(),
                cwd: root.to_string_lossy().into(),
                action: action.into(),
                feature_branch: Some(feature),
                commit_message: message.map(str::to_owned),
                ..Default::default()
            },
            || stop.get(),
            || false,
            |v| {
                if let Some(v) = v {
                    if matches!(
                        v["kind"].as_str(),
                        Some("action_finished" | "action_failed")
                    ) {
                        stop.set(true);
                    }
                    events.push(v);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(events.last().unwrap()["kind"], "action_finished");
    };
    std::fs::write(root.join("a"), "changed").unwrap();
    run("generated", "commit", false, None);
    assert!(git(&root, &["log", "-1", "--format=%B"]).starts_with("Apply reader settings\n\n"));
    let p = json!({"projectId":"p","repository":"example/repo","number":1});
    let review = c
        .rpc("pullRequests.generateReviewGuide", p.clone())
        .unwrap();
    assert!(review["text"].as_str().unwrap().contains("error paths"));
    assert_eq!(c.rpc("pullRequests.reviewGuide", p).unwrap(), review);
    let traces = seen.lock().unwrap();
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[0]["selection"]["model"], "reviewer");
    assert!(traces[0]["selection"]["options"].is_array());
    assert!(traces[0]["text"]
        .as_str()
        .unwrap()
        .contains("Use imperative verbs"));
    assert_eq!(traces[1]["selection"]["model"], "reviewer");
    assert!(traces[1]["text"]
        .as_str()
        .unwrap()
        .contains("Check error paths"));
    assert!(traces.iter().all(|v| v["native"].is_null()));
    drop(traces);
    c.rpc("server.updateSettings",json!({"patch":{"branchNamingMode":"semantic","branchNamePrefix":"reader","branchNameInstructions":"Describe the change"}})).unwrap();
    std::fs::write(root.join("a"), "branch change").unwrap();
    std::fs::create_dir_all(root.join(".github")).unwrap();
    std::fs::write(
        root.join(".github/pull_request_template.md"),
        "Required validation section",
    )
    .unwrap();
    run("branch", "commit", true, Some("Branch change"));
    assert!(git(&root, &["branch", "--show-current"]).starts_with("reader/reader-generation-"));
    run("pr", "create_pr", false, None);
    let traces = seen.lock().unwrap();
    assert!(traces[2]["text"]
        .as_str()
        .unwrap()
        .contains("Describe the change"));
    assert!(traces[3]["text"]
        .as_str()
        .unwrap()
        .contains("Required validation section"));
    drop(traces);
    c.rpc("server.updateSettings",json!({"patch":{"sourceControlWriterModelSelection":{"instanceId":"claudeAgent","model":"fail"}}})).unwrap();
    std::fs::write(root.join("a"), "third").unwrap();
    run("fallback", "commit", false, None);
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "Update a");
    std::fs::remove_dir_all(bin).unwrap();
}
