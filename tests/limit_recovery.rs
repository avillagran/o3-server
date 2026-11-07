//! Durable recovery and real RPC receipts with an explicitly configured local provider.
mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn put(db: &rusqlite::Connection, kind: &str, id: &str, value: Value) {
    db.execute("INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,'t',?3) ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body",
        rusqlite::params![kind,id,value.to_string()]).unwrap();
}

#[test]
fn unsent_usage_resume_has_one_new_receipt_after_recovery_without_replaying_started_or_legacy_work()
{
    for (tagged, started, stop_after_resume) in [
        (true, false, false),
        (true, true, false),
        (false, false, false),
        (true, false, true),
    ] {
        let server = support::start(|state| {
            *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
        });
        let client = server.connect();
        let workspace = server.dir.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        client.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p","title":"Recovery","workspaceRoot":workspace})).unwrap();
        client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"thread","threadId":"t","projectId":"p","title":"Recovery","modelSelection":{"instanceId":"scripted","model":"fixture"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let now = o3_server::store::now_ms();
        let reset_ms = now - 1000;
        let reset = o3_server::store::iso(reset_ms);
        let before = o3_server::store::iso(now - 10_000);
        let old_id = format!("limit-resume:t:source:{reset_ms}:old");
        let old_command = json!({"type":"message.dispatch","commandId":old_id,"messageId":old_id,"threadId":"t","usageLimitContinuationOfRunId":"source","usageLimitRecoveryRequestId":"old","text":"Continue where you left off.","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"agent","creationSource":"server"});
        {
            let mut db = server.state.db.lock().unwrap();
            let body: String = db
                .query_row(
                    "SELECT body FROM records WHERE kind='thread' AND id='t'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut thread: Value = serde_json::from_str(&body).unwrap();
            thread["usageLimitResetAt"] = json!(reset);
            thread["limitRecovery"] = json!({"runId":"source","resetAt":reset,"autoResume":true,"snooze":false,"requestId":"old"});
            put(&db, "thread", "t", thread);
            put(
                &db,
                "runs",
                "source",
                json!({"id":"source","threadId":"t","providerInstanceId":"scripted","ordinal":1,"status":"failed","rootNodeId":"source-root","requestedAt":before,"startedAt":before,"completedAt":before,"errorClass":"usage_limit"}),
            );
            put(
                &db,
                "turnItems",
                "failure",
                json!({"id":"failure","threadId":"t","runId":"source","nodeId":"source-root","ordinal":1,"type":"error","status":"failed","updatedAt":before,"failure":{"class":"usage_limit","message":"Synthetic limit","resetAt":reset}}),
            );
            let mut run = json!({"id":"resume","threadId":"t","providerInstanceId":"scripted","ordinal":2,"status":"starting","rootNodeId":"resume-root","requestedAt":before,"startedAt":null,"completedAt":null});
            if tagged {
                run["usageLimitContinuationOfRunId"] = json!("source");
            }
            if started {
                run["status"] = json!("running");
                run["startedAt"] = json!(before);
            }
            put(&db, "runs", "resume", run);
            put(
                &db,
                "turnItems",
                "prompt",
                json!({"id":"prompt","threadId":"t","runId":"resume","nodeId":null,"ordinal":2,"type":"user_message","status":"pending","updatedAt":before}),
            );
            db.execute(
                "INSERT INTO receipts VALUES(?1,?2)",
                rusqlite::params![old_id, json!({"sequence":0}).to_string()],
            )
            .unwrap();
            o3_server::recovery::recover(&mut db).unwrap();
        }
        server.state.notify();
        // The fixture reuses a live listener; wake the parked worker as startup would.
        server.state.streams.ops.scheduler_wake.notify_one();
        let read = || {
            client
                .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
                .unwrap()
        };
        if tagged && !started {
            assert!(
                support::wait_for(Duration::from_secs(10), || read()["runs"]
                    .as_array()
                    .is_some_and(
                        |runs| runs.len() == 3 && runs.last().unwrap()["status"] == "waiting"
                    )),
                "{:?}",
                read()
            );
            if stop_after_resume {
                client
                    .rpc(
                        "orchestration.dispatchCommand",
                        json!({"type":"thread.stop","commandId":"stop-resume","threadId":"t"}),
                    )
                    .unwrap();
                assert_eq!(read()["thread"]["limitRecovery"]["autoResume"], false);
                assert_eq!(
                    read()["thread"]["limitRecovery"]["requestId"],
                    "stop-resume"
                );
                o3_server::recovery::recover(&mut server.state.db.lock().unwrap()).unwrap();
                let projection = read();
                assert_eq!(projection["runs"].as_array().unwrap().len(), 3);
                assert_eq!(projection["thread"]["limitRecovery"]["autoResume"], false);
                assert_eq!(
                    projection["thread"]["limitRecovery"]["requestId"],
                    "stop-resume"
                );
                continue;
            }
            client.rpc("orchestration.dispatchCommand",json!({"type":"runtime-request.respond","commandId":"approve-new-resume","threadId":"t","requestId":"approval-1","decision":"accept"})).unwrap();
            assert!(support::wait_for(Duration::from_secs(10), || read()
                ["runs"][2]["status"]
                == "completed"));
            let projection = read();
            let request = projection["thread"]["limitRecovery"]["requestId"]
                .as_str()
                .unwrap();
            assert_ne!(request, "old");
            let mut retry = old_command.clone();
            let id = format!("limit-resume:t:source:{reset_ms}:{request}");
            retry["commandId"] = json!(id);
            retry["messageId"] = json!(id);
            retry["usageLimitRecoveryRequestId"] = json!(request);
            client
                .rpc("orchestration.dispatchCommand", retry.clone())
                .unwrap();
            client.rpc("orchestration.dispatchCommand", retry).unwrap();
            let latest = read();
            assert_eq!(latest["runs"].as_array().unwrap().len(), 3);
            assert_eq!(latest["runs"][2]["usageLimitContinuationOfRunId"], "source");
            let prompt = latest["turnItems"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == "prompt")
                .unwrap();
            assert_eq!(prompt["status"], "cancelled");
            assert_eq!(prompt["inputIntent"], "queued_turn");
        } else {
            client
                .rpc("orchestration.dispatchCommand", old_command.clone())
                .unwrap();
            client
                .rpc("orchestration.dispatchCommand", old_command)
                .unwrap();
            assert_eq!(read()["runs"].as_array().unwrap().len(), 2);
            assert_eq!(read()["thread"]["limitRecovery"]["requestId"], "old");
        }
    }
}
