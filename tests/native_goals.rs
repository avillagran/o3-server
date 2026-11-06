mod support;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Fixture {
    server: support::Server,
    client: t3_protocol::Connection,
    root: PathBuf,
    live: bool,
}
impl Fixture {
    fn new() -> Self {
        Self::with_binary(None)
    }
    fn with_binary(binary: Option<PathBuf>) -> Self {
        Self::with_driver("codex", binary)
    }
    fn with_driver(driver: &str, binary: Option<PathBuf>) -> Self {
        let live = binary.is_some();
        let server = support::start(|state| {
            *state.provider.lock().unwrap() =
                Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                    Arc::new(o3_server::provider::ScriptedProvider),
                    state.dir.clone(),
                )));
        });
        let client = server.connect();
        let root = server.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        let peer = binary.unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/goal-peer.py")
        });
        if live {
            let home = server.dir.join("native-home");
            std::fs::create_dir(&home).unwrap();
            let source = std::env::var_os(if driver == "codex" {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            })
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap()).join(if driver == "codex" {
                    ".codex"
                } else {
                    ".claude"
                })
            });
            let name = if driver == "codex" {
                "auth.json"
            } else {
                ".credentials.json"
            };
            std::fs::copy(source.join(name), home.join(name))
                .expect("installed provider authentication required");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(home.join(name), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        client.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"goals":{"driver":driver,"enabled":true,"config":{"binaryPath":peer,"homePath":server.dir.join("native-home")}}}}})).unwrap();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Goals","workspaceRoot":root})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"t","projectId":"p","title":"Goals","modelSelection":{"instanceId":"goals","model":if driver=="claudeAgent" {"haiku"}else if live {"gpt-6.1-sol"} else {"goal-fixture"},"options":if live && driver=="codex" {json!([{"id":"reasoningEffort","value":"low"}])}else{json!([])}},"runtimeMode":if live {"approval-required"}else{"full-access"},"interactionMode":"default"})).unwrap();
        Self {
            server,
            client,
            root,
            live,
        }
    }
    fn send(&self, id: &str, text: &str) {
        self.client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"t","text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    }
    fn projection(&self) -> Value {
        self.client
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap()
    }
    fn wait(&self, test: impl Fn() -> bool) {
        assert!(
            support::wait_for(Duration::from_secs(if self.live { 90 } else { 12 }), test),
            "{}",
            self.projection()
        );
    }
    fn goal(&self) -> Value {
        self.projection()["providerThreads"][0]["goal"].clone()
    }
    fn done(&self) {
        self.wait(|| {
            self.projection()["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["status"] == "completed")
                && self.server.state.active.lock().unwrap().is_empty()
        });
    }
    fn trace(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("goal-trace.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn stop(&self) {
        let run = self.projection()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["status"] == "running")
            .unwrap()["id"]
            .clone();
        self.client
            .rpc(
                "orchestration.dispatchCommand",
                json!({"type":"run.interrupt","commandId":"stop","threadId":"t","runId":run}),
            )
            .unwrap();
        self.wait(|| {
            self.projection()["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["id"] == run && record["status"] == "interrupted")
        });
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let provider = self.server.state.provider.lock().unwrap().clone().unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(provider.dispose())
            .unwrap();
    }
}

#[test]
fn native_goal_control_turns_do_not_reach_inference_and_preserve_rollback_boundaries() {
    let f = Fixture::new();
    f.send("show", "/goal");
    f.done();
    assert!(f.projection().to_string().contains("No goal is set."));
    f.send("clear", "/goal clear");
    f.done();
    f.send("pause", "/goal pause");
    f.done();
    f.send("resume", "/goal resume");
    f.done();
    assert!(f.goal().is_null());
    assert!(!f
        .trace()
        .iter()
        .any(|entry| entry["method"] == "turn/start"));
    assert!(f.projection()["providerTurns"]
        .as_array()
        .unwrap()
        .iter()
        .all(|turn| turn["nativeTurnRef"].is_null()));
}

#[test]
fn fast_first_turn_and_native_continuation_remain_one_run_with_separate_completed_messages() {
    let f = Fixture::new();
    f.send("set", "/goal Implement all requested work");
    f.wait(|| f.goal()["status"] == "active" && f.projection().to_string().contains("FIRST"));
    assert_eq!(f.projection()["runs"][0]["status"], "running");
    let native_start = f
        .trace()
        .into_iter()
        .find(|entry| entry["method"] == "turn/start")
        .unwrap();
    assert_eq!(
        native_start["params"]["input"][0]["text"],
        "Start working toward this goal: Implement all requested work"
    );
    std::fs::write(f.root.join("next"), "").unwrap();
    f.done();
    let projection = f.projection();
    assert_eq!(projection["runs"].as_array().unwrap().len(), 1);
    assert_eq!(projection["providerTurns"].as_array().unwrap().len(), 2);
    assert!(projection["providerTurns"]
        .as_array()
        .unwrap()
        .iter()
        .all(|turn| turn["status"] == "completed"));
    let replies: Vec<_> = projection["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect();
    assert_eq!(replies.len(), 2, "{projection}");
    assert!(replies.iter().all(|message| message["streaming"] == false));
    assert_eq!(f.goal()["status"], "complete");
}

#[test]
fn goal_grace_expiry_pauses_native_goal_instead_of_leaving_unowned_work() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-next"), "").unwrap();
    f.send("set", "/goal Finish the task");
    f.done();
    assert_eq!(f.goal()["status"], "paused");
    assert_eq!(
        f.trace()
            .iter()
            .filter(|entry| entry["method"] == "turn/start")
            .count(),
        1
    );
}

#[test]
fn stop_between_goal_turns_pauses_without_interrupting_an_already_finished_turn() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-next"), "").unwrap();
    f.send("set", "/goal Keep working");
    f.wait(|| f.goal()["status"] == "active" && f.projection().to_string().contains("FIRST"));
    f.stop();
    assert_eq!(f.goal()["status"], "paused");
    assert!(!f
        .trace()
        .iter()
        .any(|entry| entry["method"] == "turn/interrupt"));
}

#[test]
fn stop_during_automatic_goal_turn_targets_the_current_native_turn() {
    let f = Fixture::new();
    std::fs::write(f.root.join("hold-second"), "").unwrap();
    f.send("set", "/goal Keep working");
    f.wait(|| f.goal()["status"] == "active");
    std::fs::write(f.root.join("next"), "").unwrap();
    f.wait(|| f.projection()["providerTurns"].as_array().unwrap().len() == 2);
    f.stop();
    assert_eq!(f.goal()["status"], "paused");
    assert!(f
        .trace()
        .iter()
        .any(|entry| entry["method"] == "turn/interrupt"
            && entry["params"]["turnId"] == "automatic-native-1"));
}

#[test]
fn late_automatic_turn_is_paused_and_interrupted_after_its_run_has_ended() {
    let f = Fixture::new();
    for name in ["no-next", "late-after-pause"] {
        std::fs::write(f.root.join(name), "").unwrap();
    }
    f.send("set", "/goal Finish the work");
    f.done();
    std::fs::write(f.root.join("orphan"), "").unwrap();
    f.wait(|| {
        f.trace().iter().any(|entry| {
            entry["method"] == "turn/interrupt" && entry["params"]["turnId"] == "late-native-1"
        })
    });
    assert_eq!(f.goal()["status"], "paused");
    assert_eq!(f.projection()["providerTurns"].as_array().unwrap().len(), 1);
}

#[test]
fn stop_does_not_wait_for_activation_and_late_ack_cannot_restart_work() {
    let f = Fixture::new();
    for name in ["no-next", "hold-activation"] {
        std::fs::write(f.root.join(name), "").unwrap();
    }
    f.send("set", "/goal Finish the work");
    f.wait(|| f.goal()["status"] == "active" && f.projection().to_string().contains("FIRST"));
    f.stop();
    std::fs::write(f.root.join("activation-reply"), "").unwrap();
    f.wait(|| {
        f.trace()
            .iter()
            .filter(|entry| {
                entry["method"] == "thread/goal/set" && entry["params"]["status"] == "paused"
            })
            .count()
            >= 3
    });
    assert_eq!(f.goal()["status"], "paused");
}

#[test]
fn paused_goal_resumes_with_empty_native_input_then_can_be_cleared() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-next"), "").unwrap();
    f.send("set", "/goal Finish the work");
    f.done();
    f.send("show", "/goal edit");
    f.done();
    assert!(f
        .projection()
        .to_string()
        .contains("Goal paused: Finish the work"));
    f.send("pause", "/goal pause");
    f.done();
    std::fs::remove_file(f.root.join("no-next")).unwrap();
    f.send("resume", "/goal resume");
    f.wait(|| f.goal()["status"] == "active");
    std::fs::write(f.root.join("next"), "").unwrap();
    f.done();
    let starts: Vec<_> = f
        .trace()
        .into_iter()
        .filter(|entry| entry["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[1]["params"]["input"], json!([]));
    f.send("clear", "/goal clear");
    f.done();
    assert!(f.goal().is_null());
}

#[test]
fn unsupported_native_goals_fail_without_sending_the_command_to_inference() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-goal-api"), "").unwrap();
    f.send("set", "/goal Finish the work");
    f.wait(|| f.projection()["runs"][0]["status"] == "failed");
    assert!(!f
        .trace()
        .iter()
        .any(|entry| entry["method"] == "turn/start"));
}

#[test]
#[ignore = "installed Codex authentication and one short goal inference; isolated native state"]
fn live_codex_goal_controls_and_completion() {
    let binary = PathBuf::from(
        std::env::var_os("O3_LIVE_CODEX_BINARY").expect("set an explicit installed Codex binary"),
    );
    let f = Fixture::with_binary(Some(binary));
    f.send("show", "/goal");
    f.done();
    assert!(f.goal().is_null());
    f.send("set","/goal Your entire task is to reply with exactly NATIVE_GOAL_PROOF and mark this native goal complete using update_goal. Do not use filesystem, shell, network or other tools.");
    f.done();
    assert_eq!(f.goal()["status"], "complete", "{}", f.projection());
    assert!(f.projection().to_string().contains("NATIVE_GOAL_PROOF"));
    f.send("pause", "/goal pause");
    f.done();
    assert_eq!(f.goal()["status"], "paused");
    f.send("resume", "/goal resume");
    f.done();
    assert_eq!(f.goal()["status"], "complete", "{}", f.projection());
    f.send("clear", "/goal clear");
    f.done();
    assert!(f.goal().is_null());
}

#[test]
fn claude_goal_confirmation_feedback_and_success_use_native_projection() {
    let f = Fixture::with_driver("claudeAgent", None);
    f.send("set", "/goal Finish the work");
    f.done();
    assert_eq!(
        f.goal(),
        json!({"objective":"Finish the work","status":"active","checks":0})
    );
    f.send("show", "/goal");
    f.done();
    assert!(f
        .projection()
        .to_string()
        .contains("Goal active: Finish the work"));
    f.send("work", "work");
    f.done();
    assert_eq!(
        f.goal(),
        json!({"objective":"Finish the work","status":"complete","checks":1})
    );
    f.send("clear", "/goal clear");
    f.done();
    assert!(f.goal().is_null());
}

#[test]
fn claude_child_output_or_failed_turn_does_not_complete_the_root_goal() {
    let f = Fixture::with_driver("claudeAgent", None);
    f.send("set", "/goal Finish the work");
    f.done();
    f.send("child", "child-only");
    f.done();
    assert_eq!(f.goal()["status"], "active");
    f.send("fail", "fail");
    f.wait(|| {
        f.projection()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["status"] == "failed")
    });
    assert_eq!(f.goal()["status"], "active");
    assert_eq!(f.goal()["checks"], 1);
    assert_eq!(f.goal()["lastCheck"], "continue until verified");
}

#[test]
#[ignore = "installed Claude authentication and one short Haiku goal turn; isolated native state"]
fn live_claude_goal_controls_and_completion() {
    let binary = PathBuf::from(
        std::env::var_os("O3_LIVE_CLAUDE_BINARY").expect("set an explicit installed Claude binary"),
    );
    let f = Fixture::with_driver("claudeAgent", Some(binary));
    let objective = "Reply only CLAUDE_GOAL_PROOF to complete this goal. Use no tools.";
    f.send("set", &format!("/goal {objective}"));
    f.done();
    assert_eq!(f.goal()["objective"], objective, "{}", f.projection());
    f.send(
        "work",
        "Complete the goal now. Reply only CLAUDE_GOAL_PROOF. Use no tools.",
    );
    f.done();
    assert_eq!(f.goal()["status"], "complete", "{}", f.projection());
    f.send("clear", "/goal clear");
    f.done();
    assert!(f.goal().is_null());
}

#[test]
fn ordinary_prompt_does_not_read_or_mutate_native_goals() {
    let f = Fixture::new();
    f.send("ordinary", "Do the requested work");
    f.done();
    assert!(f.goal().is_null());
    assert!(!f.trace().iter().any(|entry| entry["method"]
        .as_str()
        .is_some_and(|method| method.starts_with("thread/goal/"))));
}

#[test]
fn native_start_or_activation_failure_leaves_the_prepared_goal_paused() {
    for marker in ["start-fails", "activation-fails"] {
        let f = Fixture::new();
        std::fs::write(f.root.join(marker), "").unwrap();
        f.send("set", "/goal Finish the task");
        f.wait(|| {
            f.projection()["runs"][0]["status"] == "failed"
                && f.server.state.active.lock().unwrap().is_empty()
        });
        assert_eq!(f.goal()["status"], "paused", "{marker}");
        assert!(f
            .trace()
            .iter()
            .any(|entry| entry["method"] == "thread/goal/set"
                && entry["params"]["origin"] == "automatic"
                && entry["params"]["status"] == "paused"));
    }
}

#[test]
fn goal_cleared_during_pending_activation_is_not_revived_by_the_older_response() {
    let f = Fixture::new();
    for name in ["no-next", "clear-during-activation"] {
        std::fs::write(f.root.join(name), "").unwrap();
    }
    f.send("set", "/goal Finish the task");
    f.wait(|| f.goal()["status"] == "active" && f.projection().to_string().contains("FIRST"));
    std::fs::write(f.root.join("clear-native"), "").unwrap();
    f.done();
    assert!(f.goal().is_null());
    std::fs::write(f.root.join("activation-reply"), "").unwrap();
    f.send("show", "/goal");
    f.done();
    assert!(f.goal().is_null());
}

#[test]
fn explicit_resume_can_restart_a_completed_goal_without_overwriting_its_prior_turns() {
    let f = Fixture::new();
    std::fs::write(f.root.join("next"), "").unwrap();
    f.send("set", "/goal Finish the task");
    f.done();
    assert_eq!(f.goal()["status"], "complete");
    let previous = f.projection()["providerTurns"].clone();
    f.send("resume", "/goal resume");
    f.done();
    let current = f.projection();
    assert_eq!(f.goal()["status"], "complete");
    assert_eq!(
        current["providerTurns"].as_array().unwrap().len(),
        4,
        "{current}"
    );
    for previous in previous.as_array().unwrap() {
        assert!(current["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|turn| turn["id"] == previous["id"]
                && turn["runAttemptId"] == previous["runAttemptId"]));
    }
}

#[test]
#[ignore = "two installed Codex goal turns, automatic continuation and isolated native state"]
fn live_codex_goal_spans_automatic_turns() {
    let binary = PathBuf::from(
        std::env::var_os("O3_LIVE_CODEX_BINARY").expect("set an explicit installed Codex binary"),
    );
    let f = Fixture::with_binary(Some(binary));
    f.send("set","/goal This goal needs two native turns. If there is no earlier assistant reply containing PHASE_ONE, reply PHASE_ONE now and end this native turn while keeping the goal active; do not mark it complete yet. Otherwise use update_goal to mark the goal complete and reply PHASE_TWO. Do not use filesystem, shell, network or other tools.");
    f.done();
    let projection = f.projection();
    assert_eq!(f.goal()["status"], "complete", "{projection}");
    assert_eq!(projection["runs"].as_array().unwrap().len(), 1);
    assert!(
        projection["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|turn| !turn["nativeTurnRef"].is_null())
            .count()
            >= 2,
        "{projection}"
    );
    let replies: Vec<_> = projection["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect();
    assert!(replies.iter().any(|message| message["text"]
        .as_str()
        .is_some_and(|text| text.contains("PHASE_ONE"))));
    assert!(replies.iter().any(|message| message["text"]
        .as_str()
        .is_some_and(|text| text.contains("PHASE_TWO"))));
    assert!(replies.iter().all(|message| message["streaming"] == false));
    f.send("clear", "/goal clear");
    f.done();
}

#[test]
fn oversized_goal_does_not_clear_the_current_goal_and_counts_unicode_characters() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-next"), "").unwrap();
    f.send("set", "/goal Keep this goal");
    f.done();
    let before = f.goal();
    let calls = f
        .trace()
        .iter()
        .filter(|entry| {
            entry["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("thread/goal/"))
        })
        .count();
    f.send("large", &format!("/goal {}", "🙂".repeat(4001)));
    f.wait(|| {
        f.projection()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["status"] == "failed")
            && f.server.state.active.lock().unwrap().is_empty()
    });
    assert_eq!(f.goal(), before);
    assert_eq!(
        calls,
        f.trace()
            .iter()
            .filter(|entry| entry["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("thread/goal/")))
            .count()
    );
    assert!(f
        .projection()
        .to_string()
        .contains("The goal objective is 4001 characters"));
    f.send("boundary", &format!("/goal {}", "🙂".repeat(4000)));
    f.wait(|| f.goal()["status"] == "active");
    f.stop();
    assert_eq!(
        f.goal()["objective"].as_str().unwrap().chars().count(),
        4000
    );
}

#[test]
fn native_objective_rejection_publishes_the_cleared_goal_and_actionable_failure() {
    let f = Fixture::new();
    std::fs::write(f.root.join("no-next"), "").unwrap();
    f.send("set", "/goal Keep working");
    f.done();
    f.send("reject", "/goal reject-this-objective");
    f.wait(|| {
        f.projection()["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["status"] == "failed")
            && f.server.state.active.lock().unwrap().is_empty()
    });
    assert!(f.goal().is_null());
    assert!(f
        .projection()
        .to_string()
        .contains("Codex rejected the goal: goal objective must not be empty."));
}
