mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn projection(c: &t3_protocol::Connection) -> Value {
    c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap()
}

#[test]
fn withdrawn_permissions_and_questions_close_before_the_turn_finishes() {
    for text in ["withdraw-approval", "withdraw-question"] {
        let s = support::start(|s| {
            *s.provider.lock().unwrap() = Some(Arc::new(o3_server::claude::ClaudeProvider::new(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/claude/withdrawal.py"),
            )));
        });
        let cwd = s.dir.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let c = s.connect();
        c.rpc("projects.mutate", json!({"type":"project.create","commandId":"project","projectId":"p","title":"Fixture","workspaceRoot":cwd})).unwrap();
        c.rpc("orchestration.launchThread", json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Withdrawals","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":text,"attachments":[]}})).unwrap();
        assert!(support::wait_for(Duration::from_secs(5), || {
            projection(&c)["runtimeRequests"]
                .as_array()
                .is_some_and(|r| r.len() == 2)
        }));
        let initial = projection(&c);
        let requests = initial["runtimeRequests"].as_array().unwrap();
        let first = requests
            .iter()
            .find(|r| r["nativeRequestRef"]["nativeId"] == "native-1")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = requests
            .iter()
            .find(|r| r["nativeRequestRef"]["nativeId"] == "native-2")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        std::fs::write(cwd.join("withdraw-first"), "go").unwrap();
        assert!(
            support::wait_for(Duration::from_secs(3), || {
                projection(&c)["runtimeRequests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["id"] == first && r["status"] == "cancelled")
            }),
            "provider withdrawal must cancel the pending card"
        );
        let mid = projection(&c);
        assert_eq!(
            mid["runs"][0]["status"], "waiting",
            "another request is still pending"
        );
        assert!(mid["runtimeRequests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == second && r["status"] == "pending"));
        assert!(c.rpc("orchestration.dispatchCommand", json!({"type":"runtime-request.respond","commandId":"late-response","threadId":"t","requestId":first,"decision":"accept"})).is_err());
        std::fs::write(cwd.join("withdraw-last"), "go").unwrap();
        assert!(support::wait_for(Duration::from_secs(5), || projection(&c)
            ["runs"][0]["status"]
            == "completed"));
        let final_state = projection(&c);
        for request in final_state["runtimeRequests"].as_array().unwrap() {
            assert_eq!(request["status"], "cancelled");
            assert!(request["resolvedAt"].is_string());
            let node = final_state["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"] == request["nodeId"])
                .unwrap();
            assert_eq!(node["status"], "cancelled");
            let item = final_state["turnItems"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["requestId"] == request["id"])
                .unwrap();
            assert_eq!(item["status"], "cancelled");
            assert!(item["completedAt"].is_string());
        }
        {
            let db = s.state.db.lock().unwrap();
            let completed: i64 = db.query_row("SELECT min(sequence) FROM events WHERE thread_id='t' AND json_extract(body,'$.type')='run.updated' AND json_extract(body,'$.payload.status')='completed'", [], |r| r.get(0)).unwrap();
            for id in [&first, &second] {
                let (count, sequence): (i64, i64) = db.query_row("SELECT count(*), max(sequence) FROM events WHERE thread_id='t' AND json_extract(body,'$.type')='runtime-request.updated' AND json_extract(body,'$.payload.id')=? AND json_extract(body,'$.payload.status')='cancelled'", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
                assert_eq!(count, 1, "duplicate withdrawals must be inert");
                assert!(
                    sequence < completed,
                    "cancellation must precede terminal settlement"
                );
            }
        }
        assert!(t3_protocol::questions::pending(&final_state)
            .as_array()
            .unwrap()
            .is_empty());
        assert!(t3_protocol::approvals::pending(&final_state)
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn unrelated_zero_turn_notification_keeps_the_prompt_reader_attached() {
    let s = support::start(|s| {
        *s.provider.lock().unwrap() = Some(Arc::new(o3_server::claude::ClaudeProvider::new(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/notification-result.py"),
        )));
    });
    let cwd = s.dir.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let c = s.connect();
    c.rpc("projects.mutate", json!({"type":"project.create","commandId":"project","projectId":"p","title":"Fixture","workspaceRoot":cwd})).unwrap();
    c.rpc("orchestration.launchThread", json!({"commandId":"launch","threadId":"t","projectId":"p","title":"Resume boundary","modelSelection":{"instanceId":"claudeAgent","model":"test"},"runtimeMode":"approval-required","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"text":"new prompt","attachments":[]}})).unwrap();
    assert!(support::wait_for(Duration::from_secs(5), || {
        projection(&c)["runs"][0]["status"] == "completed"
    }));
    let final_projection = projection(&c);
    assert!(
        final_projection["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| { m["role"] == "assistant" && m["text"] == "ACTUAL_PROMPT_REPLY" }),
        "the real reply after the unrelated result must reach the projection"
    );
    assert_eq!(final_projection["runs"].as_array().unwrap().len(), 1);
}
