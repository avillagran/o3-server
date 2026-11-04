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
fn server() -> support::Server {
    support::start(|s| {
        let peer = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transfer/native.py");
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::Providers {
            claude: Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                peer.clone(),
            ))),
            codex: Arc::new(o3_server::codex::CodexProvider::new(peer, None)),
        }));
    })
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
            send(&c, "parent", "RETRY", "failed");
            let logs = trace(&s);
            assert_eq!(
                logs.iter()
                    .filter(|f| f["method"] == "thread/inject_items")
                    .count(),
                1
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
                            .any(|r| r["status"] == "failed"),
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
    send(&c, "parent", "AFTER-REOPEN", "failed");
    assert_eq!(
        trace(&s)
            .iter()
            .filter(|f| f["method"] == "thread/inject_items")
            .count(),
        1
    );
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
