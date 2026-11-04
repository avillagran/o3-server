mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use t3_protocol::Connection;
fn cmd(c: &Connection, value: Value) -> t3_protocol::Result<Value> {
    c.rpc("orchestration.dispatchCommand", value)
}
fn projection(c: &Connection) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap()
}
fn trace(root: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("selection-trace.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
#[test]
fn a_turn_scoped_selection_can_steer_without_restarting_the_running_turn() {
    for (instance, metadata_first) in [
        ("codex", false),
        ("codex", true),
        ("claudeAgent", false),
        ("claudeAgent", true),
    ] {
        let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/selection-peer.py");
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer.clone(), None));
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer));
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                codex: codex.clone(),
                claude: Some(claude.clone()),
            }));
        });
        let c = s.connect();
        let root = s.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".selection-fixture"), "").unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Selection","workspaceRoot":root})).unwrap();
        cmd(&c,json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Selection","modelSelection":{"instanceId":instance,"model":"one"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        cmd(&c,json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":"HOLD original turn","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(10), || projection(
            &c
        )["providerTurns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["status"] == "running")));
        let before = projection(&c);
        let run = before["runs"][0].clone();
        let native = before["providerThreads"][0]["nativeThreadRef"].clone();
        cmd(&c,json!({"type":"thread.interaction-mode.set","commandId":"plan","threadId":"t","interactionMode":"plan"})).unwrap();
        cmd(&c,json!({"type":"thread.runtime-mode.set","commandId":"runtime","threadId":"t","runtimeMode":"approval-required"})).unwrap();
        let policy:String=s.state.db.lock().unwrap().query_row("SELECT json_extract(body,'$.executionPolicy.runtimeMode') FROM run_internal WHERE id=?",[run["id"].as_str()],|r|r.get(0)).unwrap();
        assert_eq!(policy, "full-access");
        assert!(before["runs"][0].get("executionPolicy").is_none());
        if metadata_first {
            cmd(&c,json!({"type":"thread.model-selection.set","commandId":"model-pref","threadId":"t","modelSelection":{"instanceId":instance,"model":"two"}})).unwrap();
        }
        let steer = json!({"type":"message.dispatch","commandId":"steer","threadId":"t","messageId":"steer","text":"finish with next-turn model","attachments":[],"modelSelection":{"instanceId":instance,"model":"two"},"dispatchMode":{"type":"steer_active","targetRunId":run["id"]}});
        let receipt = cmd(&c, steer.clone()).unwrap();
        assert_eq!(cmd(&c, steer).unwrap(), receipt);
        assert!(support::wait_for(Duration::from_secs(10), || projection(
            &c
        )["runs"][0]["status"]
            == "completed"));
        let p = projection(&c);
        assert_eq!(p["runs"].as_array().unwrap().len(), 1);
        assert_eq!(p["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(p["runs"][0]["modelSelection"]["model"], "one");
        assert_eq!(p["thread"]["modelSelection"]["model"], "two");
        assert_eq!(p["providerThreads"][0]["nativeThreadRef"], native);
        cmd(&c,json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"NEXT turn","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(10), || projection(
            &c
        )["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |r| r["userMessageId"] == "next" && r["status"] == "completed"
            )));
        let frames = trace(&root);
        assert!(!frames.iter().any(|f| f["method"] == "turn/interrupt"
            || f["method"] == "thread/unsubscribe"
            || f["method"] == "claude/interrupt"));
        let starts: Vec<_> = frames
            .iter()
            .filter(|f| {
                f["method"]
                    == if instance == "codex" {
                        "turn/start"
                    } else {
                        "claude/user"
                    }
            })
            .collect();
        assert_eq!(starts[0]["params"]["model"], "one");
        assert_eq!(starts.last().unwrap()["params"]["model"], "two");
        if instance == "codex" {
            assert_eq!(starts[0]["params"]["collaborationMode"], Value::Null);
            assert_eq!(
                starts.last().unwrap()["params"]["collaborationMode"]["mode"],
                "plan"
            );
            assert!(frames
                .iter()
                .any(|f| f["method"] == "thread/resume"
                    && f["params"]["approvalPolicy"] == "on-request"));
        } else {
            assert!(frames
                .iter()
                .any(|f| f["method"] == "claude/set_permission_mode"
                    && f["params"]["mode"] == "plan"));
        }

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                codex.shutdown().await;
                claude.shutdown().await;
            });
    }
}

struct Classified(o3_server::provider::Providers);
impl o3_server::provider::Provider for Classified {
    fn capabilities(&self, instance: &str) -> Option<Value> {
        self.0.capabilities(instance)
    }
    fn plan_selection_transition(
        &self,
        _instance: &str,
        _current: &Value,
        target: &Value,
        _caps: &Value,
    ) -> Result<Option<o3_server::selection_transition::Selection>, String> {
        use o3_server::selection_transition::Selection::*;
        Ok(match target["model"].as_str().unwrap() {
            "restart" => Some(RestartSession),
            "handoff" => Some(CreateWithHandoff),
            "reject" => Some(Reject {
                reason: "adapter refuses this model".into(),
            }),
            "missing" => None,
            _ => Some(ApplyOnNextTurn),
        })
    }
    fn detach<'a>(
        &'a self,
        instance: &'a str,
        key: &'a str,
        native: Option<&'a str>,
        revoke: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.0.detach(instance, key, native, revoke)
    }
    fn execute<'a>(
        &'a self,
        input: o3_server::provider::RunInput,
        output: tokio::sync::mpsc::Sender<o3_server::provider::ProviderEvent>,
        control: tokio::sync::mpsc::Receiver<o3_server::provider::ProviderControl>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        self.0.execute(input, output, control, cancel)
    }
}
#[test]
fn classified_restart_handoff_and_refusal_are_durable_for_one_native_instance() {
    for instance in ["codex", "claudeAgent"] {
        for target in ["restart", "handoff", "reject", "missing"] {
            let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/selection-peer.py");
            let codex = Arc::new(o3_server::codex::CodexProvider::new(peer.clone(), None));
            let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer));
            let s = support::start(|s| {
                *s.provider.lock().unwrap() =
                    Some(Arc::new(Classified(o3_server::provider::Providers {
                        codex: codex.clone(),
                        claude: Some(claude.clone()),
                    })));
            });
            let c = s.connect();
            let root = s.dir.join("workspace");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join(".selection-fixture"), "").unwrap();
            c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Classification","workspaceRoot":root})).unwrap();
            cmd(&c,json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Classification","modelSelection":{"instanceId":instance,"model":"one"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
            cmd(&c,json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":"CLASSIFIER-HISTORY-WORD","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
            assert!(support::wait_for(Duration::from_secs(10), || projection(
                &c
            )["runs"][0]["status"]
                == "completed"));
            let before = projection(&c);
            let old = before["providerThreads"][0].clone();
            let change = json!({"type":"thread.model-selection.set","commandId":"change","threadId":"t","modelSelection":{"instanceId":instance,"model":target}});
            if matches!(target, "reject" | "missing") {
                c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"unavailable":{"driver":"cursor","enabled":true}}}})).unwrap();
                let error=cmd(&c,json!({"type":"provider.switch","commandId":"unavailable-target","threadId":"t","modelSelection":{"instanceId":"unavailable","model":"any"}})).unwrap_err().to_string();
                assert!(error.contains("unavailable"), "{error}");
                let error = cmd(&c, change).unwrap_err().to_string();
                assert!(
                    error.contains(if target == "reject" {
                        "adapter refuses"
                    } else {
                        "did not classify"
                    }),
                    "{error}"
                );
                assert_eq!(
                    projection(&c)["thread"]["modelSelection"],
                    before["thread"]["modelSelection"]
                );
                assert_eq!(
                    s.state
                        .db
                        .lock()
                        .unwrap()
                        .query_row(
                            "SELECT count(*) FROM receipts WHERE command_id='change'",
                            [],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                    0
                );
                assert_eq!(
                    s.state
                        .db
                        .lock()
                        .unwrap()
                        .query_row("SELECT count(*) FROM provider_selection_plans", [], |r| r
                            .get::<_, i64>(
                            0
                        ))
                        .unwrap(),
                    0
                );
                if target == "reject" {
                    cmd(&c,json!({"type":"provider.switch","commandId":"delete-plan","threadId":"t","modelSelection":{"instanceId":instance,"model":"restart"}})).unwrap();
                    cmd(
                        &c,
                        json!({"type":"thread.delete","commandId":"delete","threadId":"t"}),
                    )
                    .unwrap();
                    assert_eq!(
                        s.state
                            .db
                            .lock()
                            .unwrap()
                            .query_row("SELECT count(*) FROM provider_selection_plans", [], |r| r
                                .get::<_, i64>(
                                0
                            ))
                            .unwrap(),
                        0
                    );
                }
            } else {
                let receipt = cmd(&c, change.clone()).unwrap();
                assert_eq!(cmd(&c, change).unwrap(), receipt);
                // The latest classification survives a State reopen before next admission.
                {
                    let reopened = o3_server::store::State::open(&s.dir).unwrap();
                    assert_eq!(
                        reopened
                            .db
                            .lock()
                            .unwrap()
                            .query_row("SELECT count(*) FROM provider_selection_plans", [], |r| r
                                .get::<_, i64>(
                                0
                            ))
                            .unwrap(),
                        1
                    );
                }
                cmd(&c,json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"NEXT classified turn","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
                assert!(support::wait_for(Duration::from_secs(10), || projection(
                    &c
                )["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["userMessageId"] == "next"
                        && matches!(
                            r["status"].as_str(),
                            Some("completed" | "failed")
                        ))));
                let p = projection(&c);
                let run = p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["userMessageId"] == "next")
                    .unwrap();
                assert_eq!(
                    run["status"], "completed",
                    "{instance}/{target}: {}",
                    run["error"]
                );
                let pt = p["providerThreads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|pt| pt["id"] == run["providerThreadId"])
                    .unwrap();
                if target == "restart" {
                    assert_eq!(pt["id"], old["id"]);
                    assert_eq!(pt["nativeThreadRef"], old["nativeThreadRef"]);
                    assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
                } else {
                    assert_ne!(pt["id"], old["id"]);
                    assert_ne!(pt["nativeThreadRef"], old["nativeThreadRef"]);
                    assert!(p["contextHandoffs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|h| h["toProviderThreadId"] == pt["id"]));
                    assert!(trace(&root).iter().any(|f| ((instance == "codex"
                        && f["method"] == "thread/inject_items")
                        || (instance == "claudeAgent" && f["method"] == "claude/user"))
                        && f["params"].to_string().contains("CLASSIFIER-HISTORY-WORD")));
                }
                assert_eq!(
                    s.state
                        .db
                        .lock()
                        .unwrap()
                        .query_row("SELECT count(*) FROM provider_selection_plans", [], |r| r
                            .get::<_, i64>(
                            0
                        ))
                        .unwrap(),
                    0
                );
            }
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    codex.shutdown().await;
                    claude.shutdown().await;
                });
        }
    }
}

#[test]
fn reordered_equivalent_options_preserve_native_process_and_approval_policy() {
    for instance in ["codex", "claudeAgent"] {
        let peer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/selection-peer.py");
        let codex = Arc::new(o3_server::codex::CodexProvider::new(peer.clone(), None));
        let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer));
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                codex: codex.clone(),
                claude: Some(claude.clone()),
            }));
        });
        let c = s.connect();
        let root = s.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".selection-fixture"), "").unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Equivalent options","workspaceRoot":root})).unwrap();
        let options = if instance == "codex" {
            json!([{"id":"reasoningEffort","value":"high"},{"id":"serviceTier","value":"default"}])
        } else {
            json!([{"id":"effort","value":"high"},{"id":"fastMode","value":false}])
        };
        cmd(&c,json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Equivalent options","modelSelection":{"instanceId":instance,"model":"one","options":options},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        cmd(&c,json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":"NEXT original order","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(10), || projection(
            &c
        )["runs"][0]["status"]
            == "completed"));
        let before = projection(&c)["providerThreads"][0]["nativeThreadRef"].clone();
        s.state.db.lock().unwrap().execute("INSERT INTO records VALUES('approvalPolicies','sentinel','t','{\"id\":\"sentinel\"}')",[]).unwrap();
        let reversed: Vec<_> = options.as_array().unwrap().iter().rev().cloned().collect();
        cmd(&c,json!({"type":"provider.switch","commandId":"reorder","threadId":"t","modelSelection":{"instanceId":instance,"model":"one","options":reversed}})).unwrap();
        assert_eq!(
            s.state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM records WHERE kind='approvalPolicies'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        cmd(&c,json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"NEXT reordered options","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(10), || projection(
            &c
        )["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |r| r["userMessageId"] == "next" && r["status"] == "completed"
            )));
        assert_eq!(
            projection(&c)["providerThreads"][0]["nativeThreadRef"],
            before
        );
        let frames = trace(&root);
        let starts: Vec<_> = frames
            .iter()
            .filter(|f| {
                f["method"]
                    == if instance == "codex" {
                        "turn/start"
                    } else {
                        "claude/user"
                    }
            })
            .collect();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0]["pid"], starts[1]["pid"]);
        assert!(projection(&c)["contextHandoffs"]
            .as_array()
            .unwrap()
            .is_empty());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                codex.shutdown().await;
                claude.shutdown().await;
            });
    }
}

#[test]
#[ignore = "four installed Claude Haiku/Codex turns; steer next-turn preferences while native shell work remains active"]
fn live_turn_scoped_selection_preserves_active_work_and_native_history() {
    for instance in ["codex", "claudeAgent"] {
        let codex = Arc::new(o3_server::codex::CodexProvider::default());
        let claude = o3_server::claude::ClaudeProvider::discover()
            .map(Arc::new)
            .expect("installed Claude");
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                codex: codex.clone(),
                claude: Some(claude.clone()),
            }));
        });
        let c = s.connect();
        let root = s.dir.join("workspace");
        std::fs::create_dir(&root).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Live selection","workspaceRoot":root})).unwrap();
        let model = if instance == "codex" {
            "gpt-6.1-sol"
        } else {
            "haiku"
        };
        let initial = if instance == "codex" {
            json!([{"id":"reasoningEffort","value":"low"}])
        } else {
            json!([])
        };
        let next = if instance == "codex" {
            json!([{"id":"reasoningEffort","value":"medium"}])
        } else {
            json!([{"id":"fastMode","value":false}])
        };
        cmd(&c,json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Live selection","modelSelection":{"instanceId":instance,"model":model,"options":initial},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let word = format!(
            "PRIVATE{}",
            o3_server::store::random_id().unwrap().replace('-', "")
        );
        cmd(&c,json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":format!("Remember codeword {word}. Execute exactly one shell command: sleep 4; printf 'COMMAND-COMPLETED'. Then reply only with the codeword. Use no other tools."),"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        assert!(
            support::wait_for(Duration::from_secs(90), || {
                let p = projection(&c);
                p["turnItems"].as_array().unwrap().iter().any(|item| {
                    item["status"] == "running"
                        && matches!(
                            item["type"].as_str(),
                            Some("command_execution" | "dynamic_tool")
                        )
                })
            }),
            "{instance}: native shell did not become active"
        );
        let before = projection(&c);
        let run = before["runs"][0].clone();
        let native = before["providerThreads"][0]["nativeThreadRef"].clone();
        cmd(&c,json!({"type":"message.dispatch","commandId":"steer","threadId":"t","messageId":"steer","text":"Let the existing shell command finish. Reply only with the codeword from my earlier message. Do not use more tools.","attachments":[],"modelSelection":{"instanceId":instance,"model":model,"options":next},"dispatchMode":{"type":"steer_active","targetRunId":run["id"]}})).unwrap();
        let done = |mid: &str| {
            assert!(support::wait_for(Duration::from_secs(90), || {
                let p = projection(&c);
                let r = p["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["userMessageId"] == mid)
                    .unwrap();
                assert_ne!(r["status"], "failed", "{instance}/{mid}: {}", r["error"]);
                r["status"] == "completed"
            }));
        };
        done("seed");
        let p = projection(&c);
        assert_eq!(p["runs"].as_array().unwrap().len(), 1);
        assert_eq!(p["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(p["runs"][0]["modelSelection"]["options"], initial);
        assert_eq!(p["thread"]["modelSelection"]["options"], next);
        assert!(
            p["turnItems"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["status"] == "completed"
                    && matches!(
                        i["type"].as_str(),
                        Some("command_execution" | "dynamic_tool")
                    )),
            "native work was interrupted"
        );
        cmd(&c,json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"Reply only with the earlier codeword. Do not use any tools.","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
        done("next");
        let p = projection(&c);
        let next_run = p["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["userMessageId"] == "next")
            .unwrap();
        assert_eq!(next_run["modelSelection"]["options"], next);
        assert_eq!(p["providerThreads"][0]["nativeThreadRef"], native);
        assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
        assert!(p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["runId"] == next_run["id"]
                && m["role"] == "assistant"
                && m["text"].as_str().is_some_and(|t| t.contains(&word))));
        println!("Installed {instance}: active shell completed, steering retained one attempt, next-turn preferences/native identity/private history preserved");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                codex.shutdown().await;
                claude.shutdown().await;
            });
    }
}

#[test]
fn legacy_alias_resolves_canonical_driver_home_policy_and_native_binding() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::ConfiguredProviders::new(
                Arc::new(o3_server::provider::ScriptedProvider),
                s.dir.clone(),
            )));
    });
    let c = s.connect();
    let root = s.dir.join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join(".selection-fixture"), "").unwrap();
    let home = s.dir.join("canonical-home");
    std::fs::create_dir(&home).unwrap();
    let peer =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selection-peer.py");
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"claudeAgent":{"driver":"codex","enabled":true,"config":{"binaryPath":peer,"homePath":home}}}}})).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Legacy alias","workspaceRoot":root})).unwrap();
    cmd(&c,json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Legacy alias","modelSelection":{"instanceId":"claude","model":"one"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    cmd(&c,json!({"type":"message.dispatch","commandId":"seed","threadId":"t","messageId":"seed","text":"NEXT alias","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || projection(
        &c
    )["runs"][0]["status"]
        == "completed"));
    let p = projection(&c);
    let native = p["providerThreads"][0]["nativeThreadRef"].clone();
    assert_eq!(native["driver"], "codex");
    cmd(&c,json!({"type":"provider.switch","commandId":"canonical","threadId":"t","modelSelection":{"instanceId":"claudeAgent","model":"two"}})).unwrap();
    cmd(&c,json!({"type":"message.dispatch","commandId":"next","threadId":"t","messageId":"next","text":"NEXT canonical","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || projection(
        &c
    )["runs"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |r| r["userMessageId"] == "next" && r["status"] == "completed"
        )));
    let p = projection(&c);
    assert_eq!(p["providerThreads"].as_array().unwrap().len(), 1);
    assert_eq!(p["providerThreads"][0]["nativeThreadRef"], native);
    assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
    cmd(
        &c,
        json!({"type":"thread.delete","commandId":"delete","threadId":"t"}),
    )
    .unwrap();
}
