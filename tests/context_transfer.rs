mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use t3_protocol::{api::orchestration, Connection};
fn cmd(c: &Connection, v: Value) -> t3_protocol::Result<Value> {
    c.rpc("orchestration.dispatchCommand", v)
}
fn create(c: &Connection, id: &str, instance: &str, model: &str) {
    cmd(c,json!({"type":"thread.create","commandId":format!("create-{id}"),"threadId":id,"projectId":"p","title":id,"modelSelection":{"instanceId":instance,"model":model},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
}
fn projection(c: &Connection, id: &str) -> Value {
    orchestration::thread_projection(c, id).unwrap()
}
fn send(c: &Connection, id: &str, message: &str, status: &str) -> Value {
    cmd(c,json!({"type":"message.dispatch","commandId":format!("send-{id}-{message}"),"threadId":id,"messageId":format!("message-{id}-{message}"),"text":message,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(
        support::wait_for(Duration::from_secs(10), || {
            let p = projection(c, id);
            p["runs"].as_array().unwrap().iter().any(|r| {
                r["userMessageId"] == format!("message-{id}-{message}") && r["status"] == status
            })
        }),
        "{}",
        projection(c, id)
    );
    projection(c, id)["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == format!("message-{id}-{message}"))
        .unwrap()
        .clone()
}
fn server_adapters() -> (
    support::Server,
    Arc<o3_server::claude::ClaudeProvider>,
    Arc<o3_server::codex::CodexProvider>,
) {
    let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transfer/native.py");
    let claude = Arc::new(o3_server::claude::ClaudeProvider::new(peer.clone()));
    let codex = Arc::new(o3_server::codex::CodexProvider::new(peer, None));
    let s = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: Some(claude.clone()),
            codex: codex.clone(),
        }));
    });
    (s, claude, codex)
}
fn server() -> support::Server {
    server_adapters().0
}

fn fork(c: &Connection, source: &str, target: &str, run: &Value) -> Value {
    cmd(c,json!({"type":"thread.fork","commandId":format!("fork-{target}"),"sourceThreadId":source,"targetThreadId":target,"sourcePoint":{"type":"run","runId":run["id"]}})).unwrap()
}
fn trace(s: &support::Server) -> Vec<Value> {
    std::fs::read_to_string(s.dir.join("transfer-trace.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn merge(
    c: &Connection,
    source: &str,
    target: &str,
    run: &Value,
    key: &str,
) -> t3_protocol::Result<Value> {
    cmd(
        c,
        json!({"type":"thread.merge_back","commandId":key,"sourceThreadId":source,"targetThreadId":target,"sourcePoint":{"type":"run","runId":run["id"]},"createdBy":"user"}),
    )
}
#[test]
fn native_fork_and_merge_delta_reach_both_adapters_once() {
    for instance in ["codex", "claudeAgent"] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", instance, "test");
        let parent = send(&c, "parent", "PARENT-SEED", "completed");
        fork(&c, "parent", "fork", &parent);
        let child = send(&c, "fork", "FORK-ONLY", "completed");
        let p = projection(&c, "fork");
        let transfer = &p["contextTransfers"][0];
        assert_eq!(transfer["status"], "consumed", "{p}");
        assert_eq!(transfer["resolution"]["strategy"], "native_fork");
        assert_eq!(transfer["sourcePoint"]["threadId"], "parent");
        assert!(transfer["sourcePoint"]["type"].is_null());
        let before = merge(&c, "fork", "parent", &child, "merge").unwrap();
        assert_eq!(
            merge(&c, "fork", "parent", &child, "merge").unwrap(),
            before
        );
        let first = projection(&c, "parent")["contextTransfers"][0]["id"].clone();
        merge(&c, "fork", "parent", &child, "merge-new").unwrap();
        let p = projection(&c, "parent");
        assert_eq!(p["contextTransfers"][0]["id"], first);
        assert_eq!(p["contextTransfers"][0]["status"], "superseded");
        send(&c, "parent", "APPLY-MERGE", "completed");
        let p = projection(&c, "parent");
        let h = &p["contextHandoffs"][0];
        assert_eq!(h["strategy"], "fork_delta_summary");
        assert_eq!(h["toProviderThreadId"], parent["providerThreadId"]);
        assert_eq!(
            h["delivery"]["status"],
            if instance == "codex" {
                "injected"
            } else {
                "inline"
            },
            "{p}"
        );
        let items = h["history"]["messages"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items.iter().all(|m| m["threadId"] == "fork"));
        assert!(items.iter().any(|m| m["text"] == "FORK-ONLY"));
        send(&c, "parent", "PARENT-NEXT", "completed");
        let logs = trace(&s);
        if instance == "codex" {
            let injections = logs
                .iter()
                .filter(|f| f["method"] == "thread/inject_items")
                .collect::<Vec<_>>();
            assert_eq!(injections.len(), 1);
            assert!(injections[0]["params"]["items"]
                .to_string()
                .contains("FORK-ONLY"));
            assert!(!injections[0]["params"]["items"]
                .to_string()
                .contains("PARENT-SEED"));
            let fork = logs.iter().find(|f| f["method"] == "thread/fork").unwrap();
            assert_eq!(
                fork["params"]["lastTurnId"],
                transfer["sourcePoint"]["providerTurnRef"]["nativeId"]
            );
        } else {
            let last = logs.iter().rfind(|f| f["method"] == "claude/user").unwrap();
            assert_eq!(last["input"][0]["text"], "PARENT-NEXT");
        }
    }
}
#[test]
fn portable_fork_uses_selected_boundary_after_source_advances() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "claudeAgent", "test");
    let parent = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "fork", &parent);
    send(&c, "parent", "LATER-SECRET", "completed");
    send(&c, "fork", "FORK-ONLY", "completed");
    let p = projection(&c, "fork");
    assert_eq!(
        p["contextTransfers"][0]["resolution"]["strategy"],
        "portable_context"
    );
    assert_eq!(p["contextTransfers"][0]["status"], "consumed");
    let logs = trace(&s);
    let input = logs.iter().rfind(|f| f["method"] == "claude/user").unwrap();
    let text = input["input"][0]["text"].as_str().unwrap();
    assert!(text.contains("PARENT-SEED"));
    assert!(!text.contains("LATER-SECRET"));
    assert!(!input["argv"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "--fork-session"));
}
#[test]
fn injection_falls_back_only_on_method_absence_and_uncertain_delivery_is_not_replayed() {
    for model in ["inject-inline", "inject-ambiguous"] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", "codex", model);
        let parent = send(&c, "parent", "PARENT-SEED", "completed");
        fork(&c, "parent", "fork", &parent);
        let child = send(&c, "fork", "FORK-ONLY", "completed");
        merge(&c, "fork", "parent", &child, "merge").unwrap();
        send(
            &c,
            "parent",
            "APPLY-MERGE",
            if model == "inject-inline" {
                "completed"
            } else {
                "failed"
            },
        );
        let p = projection(&c, "parent");
        assert_eq!(
            p["contextHandoffs"][0]["delivery"]["status"],
            if model == "inject-inline" {
                "inline"
            } else {
                "pending"
            },
            "{p}"
        );
        if model == "inject-ambiguous" {
            send(&c, "parent", "RETRY", "completed");
            let logs = trace(&s);
            assert_eq!(
                logs.iter()
                    .filter(|f| f["method"] == "thread/inject_items")
                    .count(),
                2
            );
            let injections = logs
                .iter()
                .filter(|f| f["method"] == "thread/inject_items")
                .collect::<Vec<_>>();
            assert_ne!(
                injections[0]["params"]["threadId"],
                injections[1]["params"]["threadId"]
            );
            assert!(injections[1]["params"]["items"]
                .to_string()
                .contains("PARENT-SEED"));
            assert!(injections[1]["params"]["items"]
                .to_string()
                .contains("FORK-ONLY"));
            assert!(logs
                .iter()
                .any(|f| f["method"] == "retired-credential-probe" && f["status"] == 401));
            let old_digest =
                logs.iter().find(|f| f["method"] == "thread/start").unwrap()["_mcpDigest"].clone();
            let fresh_digest = logs
                .iter()
                .rfind(|f| f["method"] == "thread/start")
                .unwrap()["_mcpDigest"]
                .clone();
            assert_ne!(old_digest, fresh_digest);
            let child_digest =
                logs.iter().find(|f| f["method"] == "thread/fork").unwrap()["_mcpDigest"].clone();
            send(&c, "fork", "FORK-NEXT", "completed");
            let after = trace(&s);
            assert_eq!(
                after
                    .iter()
                    .rfind(|f| f["method"] == "thread/resume")
                    .unwrap()["_mcpDigest"],
                child_digest
            );
            assert!(!logs
                .iter()
                .filter(|f| f["method"] == "turn/start")
                .any(|f| f["params"]["input"].to_string().contains("APPLY-MERGE")));
        }
    }
}
#[test]
fn merge_admission_rejects_busy_parent_wrong_parent_foreign_checkpoints_and_source_runs() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    create(&c, "other", "codex", "test");
    let parent = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "fork", &parent);
    let child = send(&c, "fork", "FORK-ONLY", "completed");
    assert!(merge(&c, "fork", "other", &child, "wrong")
        .unwrap_err()
        .to_string()
        .contains("not a fork"));
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.status','running') WHERE kind='runs' AND id=?",[parent["id"].as_str().unwrap()]).unwrap();
    }
    assert!(merge(&c, "fork", "parent", &child, "busy")
        .unwrap_err()
        .to_string()
        .contains("clear its queue"));
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.status','completed') WHERE kind='runs' AND id=?",[parent["id"].as_str().unwrap()]).unwrap();
        db.execute(
            "INSERT INTO records VALUES ('checkpoints','foreign','other',?)",
            [json!({"id":"foreign","threadId":"other","runId":parent["id"]}).to_string()],
        )
        .unwrap();
    }
    assert!(cmd(&c,json!({"type":"thread.fork","commandId":"foreign","sourceThreadId":"parent","targetThreadId":"bad","sourcePoint":{"type":"checkpoint","checkpointId":"foreign"}})).is_err());
    assert!(cmd(&c,json!({"type":"thread.fork","commandId":"foreign-run","sourceThreadId":"parent","targetThreadId":"bad","sourcePoint":{"type":"run","runId":child["id"]}})).is_err());
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.status','waiting') WHERE kind='runs' AND id=?",[child["id"].as_str().unwrap()]).unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.status','running') WHERE kind='providerTurns' AND json_extract(body,'$.runAttemptId')=?",[child["activeAttemptId"].as_str().unwrap()]).unwrap();
    }
    assert!(merge(&c, "fork", "parent", &child, "live-waiting")
        .unwrap_err()
        .to_string()
        .contains("provider-finished"));
}

#[test]
#[ignore = "eight short installed Claude Haiku/Codex turns; uses isolated credential copies"]
fn live_native_fork_and_merge_back_with_private_codewords() {
    for (instance, model) in [("claudeAgent", "haiku"), ("codex", "gpt-6.1-sol")] {
        let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
        let codex = Arc::new(o3_server::codex::CodexProvider::default());
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: claude.clone(),
                codex: codex.clone(),
            }));
        });
        let c = s.connect();
        let cwd = s.dir.join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Transfer live","workspaceRoot":cwd,"scripts":[]})).unwrap();
        create(&c, "parent", instance, model);
        let first = format!(
            "ALPHA{}",
            o3_server::store::random_id().unwrap().replace('-', "")
        );
        let second = format!(
            "BETA{}",
            o3_server::store::random_id().unwrap().replace('-', "")
        );
        let turn = |thread: &str, id: &str, text: String| -> Value {
            cmd(&c,json!({"type":"message.dispatch","commandId":id,"threadId":thread,"messageId":id,"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
            assert!(
                support::wait_for(Duration::from_secs(90), || {
                    let p = projection(&c, thread);
                    assert!(
                        !p["runs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|r| r["userMessageId"] == id && r["status"] == "failed"),
                        "{instance}: {p}"
                    );
                    p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["userMessageId"] == id && r["status"] == "completed")
                }),
                "{instance}: {}",
                projection(&c, thread)
            );
            projection(&c, thread)["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["userMessageId"] == id)
                .unwrap()
                .clone()
        };
        let parent = turn(
            "parent",
            "seed",
            format!("Remember the first codeword {first}. Reply READY. Do not use any tools."),
        );
        fork(&c, "parent", "fork", &parent);
        let child=turn("fork","child",format!("Remember the second codeword {second}. Reply with only the first codeword from earlier. Do not use any tools."));
        let p = projection(&c, "fork");
        assert!(
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant"
                    && m["text"].as_str().is_some_and(|s| s.contains(&first))),
            "{instance}: native fork lost source context: {p}"
        );
        assert_eq!(
            p["contextTransfers"][0]["resolution"]["strategy"],
            "native_fork"
        );
        merge(&c, "fork", "parent", &child, "merge").unwrap();
        let before = projection(&c, "parent");
        let ptid = before["thread"]["activeProviderThreadId"].clone();
        let native = before["providerThreads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pt| pt["id"] == ptid)
            .unwrap()["nativeThreadRef"]["nativeId"]
            .clone();
        cmd(&c,json!({"type":"message.dispatch","commandId":"abandoned","threadId":"parent","messageId":"abandoned","text":"This turn was interrupted before provider admission.","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
        {
            let db = s.state.db.lock().unwrap();
            db.execute("UPDATE records SET body=json_set(body,'$.delivery',json(?)) WHERE kind='contextHandoffs' AND thread_id='parent'",[json!({"nativeThreadId":native,"status":"pending","itemIds":[],"omittedItemIds":[]}).to_string()]).unwrap();
            db.execute("UPDATE records SET body=json_set(body,'$.status','failed') WHERE kind='runs' AND thread_id='parent' AND json_extract(body,'$.userMessageId')='abandoned'",[]).unwrap();
        }
        turn(
            "parent",
            "consume",
            "Reply with only the second codeword from the fork. Do not use any tools.".into(),
        );
        let p = projection(&c, "parent");
        assert!(
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant"
                    && m["text"].as_str().is_some_and(|s| s.contains(&second))),
            "{instance}: merge delta not read: {p}"
        );
        let after_native = p["providerThreads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pt| pt["id"] == ptid)
            .unwrap()["nativeThreadRef"]["nativeId"]
            .clone();
        assert_ne!(native, after_native);
        assert_eq!(p["thread"]["activeProviderThreadId"], ptid);
        assert!(p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["strategy"] == "full_thread_summary"
                && h["targetRunId"] == p["runs"].as_array().unwrap().last().unwrap()["id"]));
        assert!(matches!(
            p["contextHandoffs"][0]["delivery"]["status"].as_str(),
            Some("injected" | "inline")
        ));
        println!("{instance}: native fork preserves private first word; merge delivery={} preserves private second word",p["contextHandoffs"][0]["delivery"]["status"]);
        if instance == "claudeAgent" {
            let source = p["runs"].as_array().unwrap().last().unwrap();
            cmd(&c,json!({"type":"thread.fork","commandId":"live-side","sourceThreadId":"parent","targetThreadId":"side","sourcePoint":{"type":"run","runId":source["id"]},"sideChat":true})).unwrap();
            turn(
                "side",
                "side-question",
                "Reply with only the second codeword. Do not use any tools.".into(),
            );
            let side = projection(&c, "side");
            assert_eq!(
                side["contextTransfers"][0]["resolution"]["strategy"],
                "native_fork"
            );
            assert_eq!(side["contextHandoffs"][0]["delivery"]["status"], "inline");
            assert!(
                side["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["role"] == "assistant"
                        && m["text"].as_str().is_some_and(|s| s.contains(&second))),
                "{side}"
            );
            println!("claudeAgent: latest side chat forks into a preassigned native id and confirms inline framing");
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                if let Some(claude) = claude {
                    claude.shutdown().await;
                }
                codex.shutdown().await;
            });
    }
}

#[test]
fn uncertain_delivery_journal_survives_reopened_state() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "inject-ambiguous");
    let parent = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "fork", &parent);
    let child = send(&c, "fork", "FORK-ONLY", "completed");
    merge(&c, "fork", "parent", &child, "merge").unwrap();
    send(&c, "parent", "APPLY-MERGE", "failed");
    let h = projection(&c, "parent")["contextHandoffs"][0].clone();
    let reopened = o3_server::store::State::open(&s.dir).unwrap();
    let body: String = reopened
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT body FROM records WHERE kind='contextHandoffs' AND id=?",
            [h["id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["delivery"],
        h["delivery"]
    );
    send(&c, "parent", "AFTER-REOPEN", "completed");
    assert_eq!(
        trace(&s)
            .iter()
            .filter(|f| f["method"] == "thread/inject_items")
            .count(),
        2
    );
    let p = projection(&c, "parent");
    assert_eq!(
        p["thread"]["activeProviderThreadId"],
        h["toProviderThreadId"]
    );
    assert_ne!(
        p["contextHandoffs"][0]["delivery"]["nativeThreadId"],
        h["delivery"]["nativeThreadId"]
    );
    assert_eq!(p["contextHandoffs"][0]["delivery"]["status"], "injected");
}

#[test]
fn latest_stable_requires_checkpoint_and_new_fork_clears_private_state_and_watches() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    let seed = send(&c, "parent", "PARENT-SEED", "completed");
    assert!(cmd(&c,json!({"type":"thread.fork","commandId":"no-stable","sourceThreadId":"parent","targetThreadId":"missing","sourcePoint":{"type":"latest_stable"}})).unwrap_err().to_string().contains("No stable"));
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.checkpointId','stable') WHERE kind='runs' AND id=?",[seed["id"].as_str().unwrap()]).unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.guestShare',json('{\"id\":\"guest\"}'),'$.opens',9,'$.pullRequests',json('[{\"host\":\"github.com\",\"repository\":\"owner/repo\",\"number\":1,\"watch\":{\"status\":\"watching\"}}]')) WHERE kind='thread' AND id='parent'",[]).unwrap();
    }
    send(&c, "parent", "LATER-SECRET", "completed");
    cmd(&c,json!({"type":"thread.fork","commandId":"stable","sourceThreadId":"parent","targetThreadId":"fork","sourcePoint":{"type":"latest_stable"}})).unwrap();
    let p = projection(&c, "fork");
    assert_eq!(p["contextTransfers"][0]["sourcePoint"]["runId"], seed["id"]);
    for field in [
        "guestShare",
        "opens",
        "pinnedAt",
        "pinOrderKey",
        "activeOrderKey",
        "snoozedAt",
        "titleRegeneration",
    ] {
        assert_eq!(p["thread"][field], Value::Null);
        assert!(p["thread"].get(field).is_some(), "missing nullable {field}");
    }
    assert!(p["thread"]["pullRequests"][0].get("watch").is_none());
    assert_eq!(p["thread"]["pullRequests"][0]["number"], 1);
}

#[test]
fn switching_provider_while_merging_keeps_parent_history_and_deduplicates_delta() {
    for (original, target) in [("codex", "claudeAgent"), ("claudeAgent", "codex")] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", original, "test");
        let seed = send(&c, "parent", "PARENT-SEED", "completed");
        fork(&c, "parent", "fork", &seed);
        let child = send(&c, "fork", "FORK-ONLY", "completed");
        merge(&c, "fork", "parent", &child, "merge").unwrap();
        cmd(&c,json!({"type":"thread.model-selection.set","commandId":"switch","threadId":"parent","modelSelection":{"instanceId":target,"model":"test"}})).unwrap();
        send(&c, "parent", "APPLY-MERGE", "completed");
        let p = projection(&c, "parent");
        assert_eq!(p["contextHandoffs"].as_array().unwrap().len(), 2);
        assert!(p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| matches!(
                h["delivery"]["status"].as_str(),
                Some("injected" | "inline")
            )));
        let logs = trace(&s);
        let received = if target == "codex" {
            logs.iter()
                .find(|f| f["method"] == "thread/inject_items")
                .unwrap()["params"]["items"]
                .clone()
        } else {
            logs.iter().rfind(|f| f["method"] == "claude/user").unwrap()["input"].clone()
        };
        assert!(received.to_string().contains("PARENT-SEED"));
        assert!(received.to_string().contains("FORK-ONLY"));
        let item_ids = p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|h| h["delivery"]["itemIds"].as_array().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert!(!item_ids.is_empty());
    }
}

#[test]
fn portable_history_filters_rolled_back_queue_and_superseded_interrupt_items() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    {
        let db = s.state.db.lock().unwrap();
        for (id, n, status) in [
            ("rolled", 1, "rolled_back"),
            ("cancelled", 2, "cancelled"),
            ("boundary", 3, "failed"),
            ("later", 4, "completed"),
        ] {
            db.execute("INSERT INTO records VALUES ('runs',?,'parent',?)",rusqlite::params![id,json!({"id":id,"threadId":"parent","ordinal":n,"status":status,"providerInstanceId":"codex","activeAttemptId":id}).to_string()]).unwrap();
        }
        for (id, run, kind, text, intent, node) in [
            ("import", None, "user_message", "IMPORTED", None, None),
            (
                "rolled-item",
                Some("rolled"),
                "user_message",
                "ROLLED",
                None,
                None,
            ),
            (
                "queue",
                Some("cancelled"),
                "user_message",
                "QUEUED",
                Some("queued_turn"),
                None,
            ),
            (
                "cancelled-answer",
                Some("cancelled"),
                "assistant_message",
                "VISIBLE_CANCELLED",
                None,
                None,
            ),
            (
                "selected",
                Some("boundary"),
                "user_message",
                "BOUNDARY",
                None,
                None,
            ),
            (
                "interrupt",
                Some("boundary"),
                "run_interrupt_result",
                "STALE_INTERRUPT",
                None,
                Some("old-root"),
            ),
            (
                "later-item",
                Some("later"),
                "user_message",
                "LATER",
                None,
                None,
            ),
            (
                "native-tool",
                Some("boundary"),
                "dynamic_tool",
                "NATIVE_TOOL",
                None,
                None,
            ),
        ] {
            db.execute("INSERT INTO records VALUES ('turnItems',?,'parent',?)",rusqlite::params![id,json!({"id":id,"threadId":"parent","runId":run,"nodeId":node,"type":kind,"text":text,"message":text,"inputIntent":intent,"status":"completed","ordinal":0}).to_string()]).unwrap();
        }
        db.execute("INSERT INTO records VALUES ('attempts','old','parent',?)",[json!({"id":"old","runId":"boundary","rootNodeId":"old-root","status":"superseded"}).to_string()]).unwrap();
    }
    fork(&c, "parent", "fork", &json!({"id":"boundary"}));
    send(&c, "fork", "CURRENT", "completed");
    let p = projection(&c, "fork");
    let messages = p["contextHandoffs"][0]["history"]["messages"]
        .as_array()
        .unwrap();
    let ids = messages
        .iter()
        .filter_map(|m| m["itemId"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from(["import", "cancelled-answer", "selected"])
    );
}

#[test]
fn side_chats_keep_stable_context_and_send_later_digest_only_as_current_input() {
    for instance in ["codex", "claudeAgent"] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", instance, "test");
        let seed = send(&c, "parent", "PARENT-SEED", "completed");
        cmd(&c,json!({"type":"thread.fork","commandId":"side","sourceThreadId":"parent","targetThreadId":"side","sourcePoint":{"type":"run","runId":seed["id"]},"sideChat":true})).unwrap();
        send(&c, "parent", "LATER-DIGEST", "completed");
        send(&c, "side", "SIDE-QUESTION", "completed");
        let p = projection(&c, "side");
        assert_eq!(p["thread"]["runtimeMode"], "approval-required");
        assert_eq!(p["thread"]["lineage"]["sideChat"], true);
        let h = p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["strategy"] == "manual_context")
            .unwrap();
        let summary = h["summaryText"].as_str().unwrap();
        assert!(summary.contains("LATER-DIGEST"));
        assert!(!summary.contains("PARENT-SEED"));
        assert!(summary.contains("do not continue or redo"));
        assert_eq!(h["delivery"]["status"], "inline");
        assert!(p["visibleTurnItems"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["visibility"] != "inherited"));
        let logs = trace(&s);
        let text = if instance == "codex" {
            logs.iter().rfind(|f| f["method"] == "turn/start").unwrap()["params"]["input"][0]
                ["text"]
                .as_str()
                .unwrap()
        } else {
            logs.iter().rfind(|f| f["method"] == "claude/user").unwrap()["input"][0]["text"]
                .as_str()
                .unwrap()
        };
        assert!(text.contains("LATER-DIGEST"));
        assert!(text.contains("SIDE-QUESTION"));
        assert!(logs
            .iter()
            .filter(|f| f["method"] == "thread/inject_items")
            .all(|f| !f["params"]["items"]
                .to_string()
                .contains("do not continue or redo")));
        assert!(cmd(&c,json!({"type":"thread.fork","commandId":"nested-side","sourceThreadId":"side","targetThreadId":"nested","sourcePoint":{"type":"latest_stable"},"sideChat":true})).unwrap_err().to_string().contains("side chat"));
    }
    let s = server();
    let c = s.connect();
    create(&c, "empty", "codex", "test");
    cmd(&c,json!({"type":"thread.fork","commandId":"empty-side","sourceThreadId":"empty","targetThreadId":"side","sourcePoint":{"type":"latest_stable"},"sideChat":true})).unwrap();
    send(&c, "side", "FIRST-QUESTION", "completed");
    let p = projection(&c, "side");
    assert_eq!(p["thread"]["forkedFrom"], Value::Null);
    assert_eq!(p["contextHandoffs"][0]["strategy"], "manual_context");
    assert_eq!(
        p["contextTransfers"][0]["sourcePoint"],
        json!({"threadId":"empty"})
    );
    assert_eq!(p["contextTransfers"][0]["status"], "consumed");
}

#[test]
fn missing_native_resume_reconstructs_and_busy_native_is_preserved_in_both_adapters() {
    for instance in ["codex", "claudeAgent"] {
        for model in ["resume-missing", "resume-in-use"] {
            let (s, claude, codex) = server_adapters();
            let c = s.connect();
            create(&c, "parent", instance, "test");
            let seed = send(&c, "parent", "PARENT-SEED", "completed");
            let p = projection(&c, "parent");
            let before = p["providerThreads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|pt| pt["id"] == seed["providerThreadId"])
                .unwrap()["nativeThreadRef"]
                .clone();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    if instance == "codex" {
                        codex.shutdown().await
                    } else {
                        claude.shutdown().await
                    }
                });
            // Imported records from older o3 versions can lack the handoff list.
            {
                let db = s.state.db.lock().unwrap();
                db.execute("UPDATE records SET body=json_remove(body,'$.handoffIds') WHERE kind='providerThreads' AND id=?", [seed["providerThreadId"].as_str().unwrap()]).unwrap();
            }
            cmd(&c,json!({"type":"thread.model-selection.set","commandId":"model","threadId":"parent","modelSelection":{"instanceId":instance,"model":model}})).unwrap();
            send(
                &c,
                "parent",
                "CURRENT",
                if model == "resume-missing" {
                    "completed"
                } else {
                    "failed"
                },
            );
            let p = projection(&c, "parent");
            assert_eq!(
                p["thread"]["activeProviderThreadId"],
                seed["providerThreadId"]
            );
            let pt = p["providerThreads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|pt| pt["id"] == seed["providerThreadId"])
                .unwrap();
            if model == "resume-in-use" {
                assert_eq!(pt["nativeThreadRef"], before);
                assert!(p["contextHandoffs"].as_array().unwrap().is_empty());
            } else {
                assert_ne!(pt["nativeThreadRef"], before);
                let recovery = p["contextHandoffs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|h| h["strategy"] == "full_thread_summary")
                    .unwrap();
                assert!(recovery["history"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["text"] == "PARENT-SEED"));
                assert_eq!(recovery["toProviderThreadId"], seed["providerThreadId"]);
                assert_eq!(p["contextTransfers"][0]["createdBy"], "system");
                assert_eq!(p["contextTransfers"][0]["status"], "consumed");
            }
        }
    }
}

#[test]
fn native_budget_rotation_reconstructs_but_does_not_interrupt_background_work() {
    for background in [false, true] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", "codex", "test");
        let seed = send(&c, "parent", "PARENT-SEED", "completed");
        fork(&c, "parent", "fork", &seed);
        let child = send(&c, "fork", "FORK-ONLY", "completed");
        merge(&c, "fork", "parent", &child, "merge").unwrap();
        {
            let db = s.state.db.lock().unwrap();
            db.execute("UPDATE records SET body=json_set(body,'$.contextUsage',json('{\"usedTokens\":128000,\"maxTokens\":128000,\"autoCompactThreshold\":120000}')) WHERE kind='providerThreads' AND id=?",[seed["providerThreadId"].as_str().unwrap()]).unwrap();
            if background {
                db.execute("UPDATE records SET body=json_set(body,'$.pendingBackgroundTasks',json('[{\"id\":\"native-background\"}]')) WHERE kind='providerThreads' AND id=?",[seed["providerThreadId"].as_str().unwrap()]).unwrap();
            }
        }
        let result = send(
            &c,
            "parent",
            "BUDGET-CURRENT",
            if background { "failed" } else { "completed" },
        );
        let logs = trace(&s);
        let starts = logs
            .iter()
            .filter(|f| f["method"] == "thread/start")
            .count();
        assert_eq!(starts, if background { 1 } else { 2 });
        let p = projection(&c, "parent");
        assert_eq!(result["providerThreadId"], seed["providerThreadId"]);
        if background {
            assert!(result["error"]
                .as_str()
                .unwrap()
                .contains("background work"));
            assert_eq!(p["contextHandoffs"].as_array().unwrap().len(), 1);
        } else {
            assert_eq!(p["contextHandoffs"].as_array().unwrap().len(), 2);
            assert!(p["contextHandoffs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|h| h["delivery"]["status"] == "injected"));
            let injection = logs
                .iter()
                .find(|f| f["method"] == "thread/inject_items")
                .unwrap()["params"]["items"]
                .to_string();
            assert!(injection.contains("PARENT-SEED"));
            assert!(injection.contains("FORK-ONLY"));
        }
    }
}

#[test]
fn rejected_unaccepted_input_is_added_as_retry_history_without_rotating_native() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    let seed = send(&c, "parent", "PARENT-SEED", "completed");
    send(&c, "parent", "REJECT-ME", "failed");
    send(&c, "parent", "CONTINUE", "completed");
    let p = projection(&c, "parent");
    assert_eq!(
        p["contextHandoffs"][0]["strategy"],
        "delta_since_target_last_seen"
    );
    assert_eq!(p["contextHandoffs"][0]["transferId"], Value::Null);
    assert!(p["contextHandoffs"][0]["history"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "REJECT-ME"));
    assert_eq!(
        p["contextHandoffs"][0]["toProviderThreadId"],
        seed["providerThreadId"]
    );
    let logs = trace(&s);
    assert_eq!(
        logs.iter()
            .filter(|f| f["method"] == "thread/start")
            .count(),
        1
    );
    assert!(logs.iter().any(|f| f["method"] == "thread/inject_items"
        && f["params"]["items"].to_string().contains("REJECT-ME")));
    assert_eq!(
        logs.iter().rfind(|f| f["method"] == "turn/start").unwrap()["params"]["input"][0]["text"],
        "CONTINUE"
    );
}

#[test]
fn maintenance_defers_inline_history_and_the_next_turn_consumes_it() {
    for instance in ["codex", "claudeAgent"] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", instance, "inject-inline");
        let seed = send(&c, "parent", "PARENT-SEED", "completed");
        fork(&c, "parent", "fork", &seed);
        let child = send(&c, "fork", "FORK-ONLY", "completed");
        merge(&c, "fork", "parent", &child, "merge").unwrap();
        send(&c, "parent", "/compact", "completed");
        let p = projection(&c, "parent");
        assert!(p["contextHandoffs"][0].get("delivery").is_none(), "{p}");
        let logs = trace(&s);
        let received = if instance == "codex" {
            &logs.iter().rfind(|f| f["method"] == "turn/start").unwrap()["params"]["input"]
        } else {
            &logs.iter().rfind(|f| f["method"] == "claude/user").unwrap()["input"]
        };
        assert_eq!(received[0]["text"], "/compact");
        send(&c, "parent", "NEXT", "completed");
        let p = projection(&c, "parent");
        assert_eq!(p["contextHandoffs"][0]["delivery"]["status"], "inline");
        let logs = trace(&s);
        let current = if instance == "codex" {
            &logs.iter().rfind(|f| f["method"] == "turn/start").unwrap()["params"]["input"]
        } else {
            &logs.iter().rfind(|f| f["method"] == "claude/user").unwrap()["input"]
        };
        assert!(current[0]["text"].as_str().unwrap().contains("FORK-ONLY"));
    }
}

#[test]
fn failed_native_cleanup_is_journaled_and_retried_before_fresh_admission() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "inject-ambiguous-unload-error");
    let seed = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "fork", &seed);
    let child = send(&c, "fork", "FORK-ONLY", "completed");
    merge(&c, "fork", "parent", &child, "merge").unwrap();
    send(&c, "parent", "UNKNOWN", "failed");
    let first = send(&c, "parent", "FIRST-RECOVERY", "failed");
    assert!(first["error"]
        .as_str()
        .unwrap()
        .contains("unload temporarily failed"));
    assert_eq!(
        trace(&s)
            .iter()
            .filter(|f| f["method"] == "thread/start")
            .count(),
        1
    );
    {
        let db = s.state.db.lock().unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM native_recoveries WHERE released_at IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    send(&c, "parent", "NEXT-RECOVERY", "completed");
    let logs = trace(&s);
    assert_eq!(
        logs.iter()
            .filter(|f| f["method"] == "thread/start")
            .count(),
        2
    );
    let injected = logs
        .iter()
        .rfind(|f| f["method"] == "thread/inject_items")
        .unwrap()["params"]["items"]
        .to_string();
    assert!(injected.contains("FIRST-RECOVERY"));
    assert!(injected.contains("PARENT-SEED"));
    assert!(injected.contains("FORK-ONLY"));
    let db = s.state.db.lock().unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM native_recoveries WHERE released_at IS NULL OR new_native IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn recovery_does_not_rotate_when_current_message_cannot_fit_a_fresh_window() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    let seed = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "fork", &seed);
    let child = send(&c, "fork", "FORK-ONLY", "completed");
    merge(&c, "fork", "parent", &child, "merge").unwrap();
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.contextUsage',json('{\"usedTokens\":128000,\"maxTokens\":128000}')) WHERE kind='providerThreads' AND id=?",[seed["providerThreadId"].as_str().unwrap()]).unwrap();
    }
    {
        let db = s.state.db.lock().unwrap();
        db.execute("UPDATE records SET body=json_set(body,'$.pendingBackgroundTasks',json('[{\"id\":\"background\"}]')) WHERE kind='providerThreads' AND id=?",[seed["providerThreadId"].as_str().unwrap()]).unwrap();
    }
    let text = "x".repeat(110000);
    cmd(&c,json!({"type":"message.dispatch","commandId":"large","threadId":"parent","messageId":"large","text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(10), || projection(
        &c, "parent"
    )["runs"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |r| r["userMessageId"] == "large" && r["status"] == "failed"
        )));
    let p = projection(&c, "parent");
    assert!(p["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["userMessageId"] == "large")
        .unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("Current input leaves no room"));
    assert_eq!(
        p["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "large")
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .len(),
        text.len()
    );
    assert_eq!(p["contextHandoffs"].as_array().unwrap().len(), 1);
    assert_eq!(
        trace(&s)
            .iter()
            .filter(|f| f["method"] == "thread/start")
            .count(),
        1
    );
    assert_eq!(
        s.state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM native_recoveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
#[ignore = "four short installed Claude Haiku/Codex turns; missing-native resume recovery"]
fn live_missing_native_resume_preserves_app_history() {
    for (instance, model) in [("claudeAgent", "haiku"), ("codex", "gpt-6.1-sol")] {
        let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
        let codex = Arc::new(o3_server::codex::CodexProvider::default());
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
                claude: claude.clone(),
                codex: codex.clone(),
            }));
        });
        let c = s.connect();
        let cwd = s.dir.join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Resume live","workspaceRoot":cwd,"scripts":[]})).unwrap();
        create(&c, "parent", instance, model);
        let word = format!(
            "RESTORE{}",
            o3_server::store::random_id().unwrap().replace('-', "")
        );
        for (id, text) in [
            (
                "seed",
                format!("Remember codeword {word}. Reply READY only. Do not use tools."),
            ),
            (
                "continue",
                "Reply with only the codeword from earlier. Do not use tools.".into(),
            ),
        ] {
            if id == "continue" {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        if let Some(claude) = &claude {
                            claude.shutdown().await;
                        }
                        codex.shutdown().await;
                    });
                let fake = o3_server::store::random_id().unwrap();
                let db = s.state.db.lock().unwrap();
                db.execute("UPDATE records SET body=json_set(body,'$.nativeThreadRef.nativeId',?) WHERE kind='providerThreads' AND thread_id='parent'",[&fake]).unwrap();
                if instance == "codex" {
                    db.execute("UPDATE records SET body=json_set(body,'$.codexNativeThreadId',?) WHERE kind='thread' AND id='parent'",[&fake]).unwrap();
                }
            }
            cmd(&c,json!({"type":"message.dispatch","commandId":id,"threadId":"parent","messageId":id,"text":text,"attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
            assert!(
                support::wait_for(Duration::from_secs(90), || {
                    let p = projection(&c, "parent");
                    let r = p["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["userMessageId"] == id)
                        .unwrap();
                    assert_ne!(r["status"], "failed", "{instance}: {}", r["error"]);
                    r["status"] == "completed"
                }),
                "{instance}: native resume did not settle"
            );
        }
        let p = projection(&c, "parent");
        assert!(
            p["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "assistant"
                    && m["text"].as_str().is_some_and(|s| s.contains(&word))),
            "{instance}: restored app history was not consumed"
        );
        assert!(p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["strategy"] == "full_thread_summary"));
        println!("{instance}: unavailable native reference recovers a private codeword in a fresh conversation");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                if let Some(claude) = &claude {
                    claude.shutdown().await;
                }
                codex.shutdown().await;
            });
    }
}

#[test]
fn failed_native_forks_fall_back_at_the_selected_app_boundary() {
    for instance in ["codex", "claudeAgent"] {
        let s = server();
        let c = s.connect();
        create(&c, "parent", instance, "test");
        let seed = send(&c, "parent", "PARENT-SEED", "completed");
        cmd(&c,json!({"type":"thread.model-selection.set","commandId":"change","threadId":"parent","modelSelection":{"instanceId":instance,"model":"resume-missing"}})).unwrap();
        if instance == "codex" {
            send(&c, "parent", "AFTER-RECOVERY", "completed");
        }
        fork(&c, "parent", "fork", &seed);
        send(&c, "fork", "FORK-ONLY", "completed");
        let p = projection(&c, "fork");
        assert_eq!(
            p["contextTransfers"][0]["resolution"]["strategy"],
            "portable_context"
        );
        assert_eq!(p["contextTransfers"][0]["status"], "consumed");
        let messages = p["contextHandoffs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["strategy"] == "full_thread_summary")
            .unwrap()["history"]["messages"]
            .as_array()
            .unwrap();
        assert!(messages.iter().any(|m| m["text"] == "PARENT-SEED"));
        assert!(!messages.iter().any(|m| m["text"] == "AFTER-RECOVERY"));
    }
}

#[test]
fn pending_native_fork_base_and_merge_delta_are_both_retained() {
    let s = server();
    let c = s.connect();
    create(&c, "parent", "codex", "test");
    let seed = send(&c, "parent", "PARENT-SEED", "completed");
    fork(&c, "parent", "middle", &seed);
    cmd(&c,json!({"type":"thread.fork","commandId":"side","sourceThreadId":"middle","targetThreadId":"side","sideChat":true,"sourcePoint":{"type":"latest_stable"}})).unwrap();
    let side = send(&c, "side", "FORK-DELTA", "completed");
    merge(&c, "side", "middle", &side, "merge").unwrap();
    send(&c, "middle", "START-MIDDLE", "completed");
    let p = projection(&c, "middle");
    assert_eq!(
        p["contextTransfers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["type"] == "fork")
            .unwrap()["resolution"]["strategy"],
        "native_fork"
    );
    assert_eq!(p["contextHandoffs"][0]["strategy"], "fork_delta_summary");
    assert!(p["contextHandoffs"][0]["history"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "FORK-DELTA"));
    assert_eq!(p["contextHandoffs"][0]["delivery"]["status"], "injected");
}
