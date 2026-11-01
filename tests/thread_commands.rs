//! Thread commands beyond the lifecycle basics, through the real client.
mod support;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use support::{start, subscribe, wait_for, Server};
use t3_protocol::Connection;

const T: Duration = Duration::from_secs(10);

fn server(provider: bool) -> (Server, Connection) {
    let s = start(|state| {
        if provider {
            *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
        }
    });
    let c = s.connect();
    (s, c)
}
fn cmd(c: &Connection, body: Value) -> t3_protocol::Result<Value> {
    c.rpc("orchestration.dispatchCommand", body)
}
fn create(c: &Connection, id: &str) {
    cmd(c, json!({"type":"thread.create","commandId":format!("create-{id}"),"threadId":id,"projectId":"p1","title":"Test","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
}
fn snap(c: &Connection, id: &str) -> Value {
    c.request_json(
        "GET",
        &format!("/api/orchestration/threads/{id}/bounded"),
        None,
        true,
    )
    .unwrap()
}
fn thread(c: &Connection, id: &str) -> Value {
    snap(c, id)["projection"]["thread"].clone()
}
fn send(c: &Connection, id: &str, msg: &str, mode: Value) -> t3_protocol::Result<Value> {
    cmd(
        c,
        json!({"type":"message.dispatch","commandId":format!("send-{msg}"),"messageId":msg,"threadId":id,"text":format!("text of {msg}"),"attachments":[],"dispatchMode":mode}),
    )
}
fn runs(c: &Connection, id: &str) -> Vec<Value> {
    snap(c, id)["projection"]["runs"]
        .as_array()
        .unwrap()
        .clone()
}
fn run_of(c: &Connection, id: &str, message: &str) -> Value {
    runs(c, id)
        .into_iter()
        .find(|r| r["userMessageId"] == message)
        .unwrap()
}
fn wait_run(c: &Connection, id: &str, message: &str, status: &str) {
    assert!(
        wait_for(T, || run_of(c, id, message)["status"] == status),
        "run of {message} never reached {status}: {:?}",
        run_of(c, id, message)
    );
}
fn complete_first_run(c: &Connection, id: &str) {
    send(c, id, "m1", json!({"type":"start_immediately"})).unwrap();
    wait_run(c, id, "m1", "waiting");
    cmd(c, json!({"type":"runtime-request.respond","commandId":"approve-1","threadId":id,"requestId":"approval-1","decision":"accept"})).unwrap();
    wait_run(c, id, "m1", "completed");
}

#[test]
fn scheduling_flags_ordering_and_modes() {
    let (_s, c) = server(true);
    create(&c, "t1");
    let past = "2001-01-01T00:00:00.000Z";
    let err = cmd(
        &c,
        json!({"type":"thread.snooze","commandId":"s0","threadId":"t1","snoozedUntil":past}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not in the future"), "{err}");
    let until = "2999-01-01T00:00:00.000Z";
    let a = cmd(
        &c,
        json!({"type":"thread.snooze","commandId":"s1","threadId":"t1","snoozedUntil":until}),
    )
    .unwrap();
    assert_eq!(
        a,
        cmd(
            &c,
            json!({"type":"thread.snooze","commandId":"s1","threadId":"t1","snoozedUntil":until})
        )
        .unwrap(),
        "commandId is idempotent"
    );
    let t = thread(&c, "t1");
    assert_eq!(t["snoozedUntil"], until);
    assert!(t["snoozedAt"].is_string());
    cmd(
        &c,
        json!({"type":"thread.unsnooze","commandId":"s2","threadId":"t1","reason":"user"}),
    )
    .unwrap();
    let t = thread(&c, "t1");
    assert_eq!(t["snoozedUntil"], Value::Null);
    assert!(t["lastSnoozeWakeAt"].is_string());

    cmd(
        &c,
        json!({"type":"thread.auto-settle.set","commandId":"a1","threadId":"t1","enabled":false}),
    )
    .unwrap();
    assert!(thread(&c, "t1")["autoSettleDisabledAt"].is_string());
    cmd(
        &c,
        json!({"type":"thread.auto-settle.set","commandId":"a2","threadId":"t1","enabled":true}),
    )
    .unwrap();
    assert_eq!(thread(&c, "t1")["autoSettleDisabledAt"], Value::Null);

    // Ordering keys: a pinned thread reorders in the pin list, an active one in the active list.
    let err = cmd(
        &c,
        json!({"type":"thread.pin.reorder","commandId":"r0","threadId":"t1","orderKey":"a"}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not pinned"), "{err}");
    cmd(
        &c,
        json!({"type":"thread.active.reorder","commandId":"r1","threadId":"t1","orderKey":"m"}),
    )
    .unwrap();
    assert_eq!(thread(&c, "t1")["activeOrderKey"], "m");
    cmd(
        &c,
        json!({"type":"thread.pin","commandId":"p1","threadId":"t1"}),
    )
    .unwrap();
    cmd(
        &c,
        json!({"type":"thread.pin.reorder","commandId":"r2","threadId":"t1","orderKey":"b"}),
    )
    .unwrap();
    assert_eq!(thread(&c, "t1")["pinOrderKey"], "b");
    let err = cmd(
        &c,
        json!({"type":"thread.active.reorder","commandId":"r3","threadId":"t1","orderKey":"z"}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not active"), "{err}");

    // Modes and model selection.
    cmd(&c, json!({"type":"thread.runtime-mode.set","commandId":"mo1","threadId":"t1","runtimeMode":"approval-required"})).unwrap();
    cmd(&c, json!({"type":"thread.interaction-mode.set","commandId":"mo2","threadId":"t1","interactionMode":"plan"})).unwrap();
    cmd(&c, json!({"type":"thread.model-selection.set","commandId":"mo3","threadId":"t1","modelSelection":{"instanceId":"other","model":"big","options":[]}})).unwrap();
    let t = thread(&c, "t1");
    assert_eq!(
        (t["runtimeMode"].as_str(), t["interactionMode"].as_str()),
        (Some("approval-required"), Some("plan"))
    );
    assert_eq!(
        (
            t["providerInstanceId"].as_str(),
            t["modelSelection"]["model"].as_str()
        ),
        (Some("other"), Some("big"))
    );

    // The shell stream carries these as thread updates.
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert_eq!(shell["threads"][0]["runtimeMode"], "approval-required");
    let mut sub = subscribe(
        c.clone(),
        "orchestration.subscribeShell",
        json!({"afterSequence":shell["snapshotSequence"]}),
    );
    cmd(
        &c,
        json!({"type":"thread.snooze","commandId":"s3","threadId":"t1","snoozedUntil":until}),
    )
    .unwrap();
    let e = sub.next(T);
    assert_eq!(
        (e["kind"].as_str(), e["thread"]["snoozedUntil"].as_str()),
        (Some("thread.updated"), Some(until))
    );

    // Archived threads refuse scheduling commands.
    cmd(
        &c,
        json!({"type":"thread.archive","commandId":"ar","threadId":"t1"}),
    )
    .unwrap();
    let err = cmd(
        &c,
        json!({"type":"thread.auto-settle.set","commandId":"a3","threadId":"t1","enabled":false}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("archived"), "{err}");
    let err = cmd(
        &c,
        json!({"type":"thread.mark-unread","commandId":"mu0","threadId":"nope"}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("was not found"), "{err}");
}

#[test]
fn mark_unread_rewinds_the_visit_watermark_before_the_last_completion() {
    let (_s, c) = server(true);
    create(&c, "t1");
    let err = cmd(
        &c,
        json!({"type":"thread.mark-unread","commandId":"mu0","threadId":"t1"}),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("no completed run"), "{err}");
    complete_first_run(&c, "t1");
    cmd(&c, json!({"type":"thread.visit","commandId":"v","threadId":"t1","visitedAt":"2999-01-01T00:00:00.000Z"})).unwrap();
    cmd(
        &c,
        json!({"type":"thread.mark-unread","commandId":"mu1","threadId":"t1"}),
    )
    .unwrap();
    let completed = run_of(&c, "t1", "m1")["completedAt"]
        .as_str()
        .unwrap()
        .to_owned();
    let visited = thread(&c, "t1")["lastVisitedAt"]
        .as_str()
        .unwrap()
        .to_owned();
    let ms = |t: &str| {
        time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
            .unwrap()
            .unix_timestamp_nanos()
            / 1_000_000
    };
    assert_eq!(ms(&completed) - ms(&visited), 1);
}

#[test]
fn fork_creates_a_linked_thread_with_inherited_history() {
    let (_s, c) = server(true);
    create(&c, "t1");
    let err = cmd(&c, json!({"type":"thread.fork","commandId":"f0","sourceThreadId":"t1","targetThreadId":"t2","sourcePoint":{"type":"latest_stable"}})).unwrap_err().to_string();
    assert!(err.contains("No stable source run"), "{err}");
    complete_first_run(&c, "t1");
    let run_id = run_of(&c, "t1", "m1")["id"].clone();
    let a = cmd(&c, json!({"type":"thread.fork","commandId":"f1","sourceThreadId":"t1","targetThreadId":"t2","sourcePoint":{"type":"latest_stable"},"title":"Branch","createdBy":"user","creationSource":"web"})).unwrap();
    assert_eq!(a, cmd(&c, json!({"type":"thread.fork","commandId":"f1","sourceThreadId":"t1","targetThreadId":"t2","sourcePoint":{"type":"latest_stable"}})).unwrap());
    let t = thread(&c, "t2");
    assert_eq!(t["title"], "Branch");
    assert_eq!(
        t["lineage"],
        json!({"parentThreadId":"t1","relationshipToParent":"fork","rootThreadId":"t1"})
    );
    assert_eq!(
        t["forkedFrom"],
        json!({"type":"run","threadId":"t1","runId":run_id})
    );
    assert_eq!(t["activeProviderThreadId"], Value::Null);
    assert_eq!(t["codexNativeThreadId"], Value::Null);
    assert!(t["providerHandoff"].as_str().unwrap().contains("text of"));
    let items = snap(&c, "t2")["projection"]["visibleTurnItems"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        items.len() >= 2
            && items
                .iter()
                .all(|i| i["visibility"] == "inherited" && i["sourceThreadId"] == "t1"),
        "{items:?}"
    );
    assert!(items.iter().any(|i| i["item"]["text"] == "text of m1"));
    // The source is untouched, and a fork of an explicit run or an unknown one behaves.
    assert_eq!(thread(&c, "t1")["lineage"]["parentThreadId"], Value::Null);
    cmd(&c, json!({"type":"thread.fork","commandId":"f2","sourceThreadId":"t1","targetThreadId":"t3","sourcePoint":{"type":"run","runId":run_id}})).unwrap();
    assert!(cmd(&c, json!({"type":"thread.fork","commandId":"f3","sourceThreadId":"t1","targetThreadId":"t4","sourcePoint":{"type":"run","runId":"nope"}})).is_err());
    assert!(cmd(&c, json!({"type":"thread.fork","commandId":"f4","sourceThreadId":"t1","targetThreadId":"t2","sourcePoint":{"type":"latest_stable"}})).is_err(), "the target id is taken");
    // The shell lists the fork.
    let shell = c
        .request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap();
    assert!(shell["threads"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == "t2"));
}

#[test]
fn queue_orders_edits_cancels_holds_and_advances() {
    let (_s, c) = server(true);
    create(&c, "t1");
    // Nothing active: a queued message simply starts.
    send(&c, "t1", "m1", json!({"type":"queue_after_active"})).unwrap();
    wait_run(&c, "t1", "m1", "waiting");
    send(&c, "t1", "m2", json!({"type":"queue_after_active"})).unwrap();
    send(&c, "t1", "m3", json!({"type":"queue_after_active"})).unwrap();
    send(&c, "t1", "m4", json!({"type":"queue_after_active"})).unwrap();
    let pos = |m: &str| {
        let r = run_of(&c, "t1", m);
        (
            r["status"].as_str().unwrap().to_owned(),
            r["queuePosition"].clone(),
        )
    };
    assert_eq!(pos("m2"), ("queued".into(), json!(1)));
    assert_eq!(pos("m4"), ("queued".into(), json!(3)));

    let r4 = run_of(&c, "t1", "m4")["id"].clone();
    let r2 = run_of(&c, "t1", "m2")["id"].clone();
    cmd(&c, json!({"type":"queued-run.reorder","commandId":"q1","threadId":"t1","runId":r4,"beforeRunId":r2})).unwrap();
    assert_eq!(
        (pos("m4").1, pos("m2").1, pos("m3").1),
        (json!(1), json!(2), json!(3))
    );
    cmd(&c, json!({"type":"queued-run.reorder","commandId":"q2","threadId":"t1","runId":r4,"beforeRunId":null})).unwrap();
    assert_eq!(
        (pos("m2").1, pos("m3").1, pos("m4").1),
        (json!(1), json!(2), json!(3))
    );

    let r3 = run_of(&c, "t1", "m3")["id"].clone();
    cmd(&c, json!({"type":"queued-run.edit","commandId":"q3","threadId":"t1","runId":r3,"text":"edited three"})).unwrap();
    let messages = snap(&c, "t1")["projection"]["messages"]
        .as_array()
        .unwrap()
        .clone();
    assert!(messages
        .iter()
        .any(|m| m["id"] == "m3" && m["text"] == "edited three"));
    cmd(
        &c,
        json!({"type":"queued-run.cancel","commandId":"q4","threadId":"t1","runId":r2}),
    )
    .unwrap();
    assert_eq!(pos("m2").0, "cancelled");
    assert_eq!((pos("m3").1, pos("m4").1), (json!(1), json!(2)));
    assert!(cmd(
        &c,
        json!({"type":"queued-run.cancel","commandId":"q5","threadId":"t1","runId":r2})
    )
    .unwrap_err()
    .to_string()
    .contains("not queued"));
    assert!(cmd(&c, json!({"type":"queued-message.promote-to-steer","commandId":"q6","threadId":"t1","queuedRunId":r3,"targetRunId":"missing"})).is_err());
    assert!(
        cmd(
            &c,
            json!({"type":"queued-run.start","commandId":"q7","threadId":"t1","runId":r3})
        )
        .unwrap_err()
        .to_string()
        .contains("stopped"),
        "cannot jump the queue while a run is active"
    );

    // Finishing the active run starts the next queued message by itself.
    cmd(&c, json!({"type":"runtime-request.respond","commandId":"ok1","threadId":"t1","requestId":"approval-1","decision":"accept"})).unwrap();
    wait_run(&c, "t1", "m1", "completed");
    wait_run(&c, "t1", "m3", "waiting");
    assert_eq!(pos("m4"), ("queued".into(), json!(1)));

    // Interrupting with holdQueue keeps the rest waiting until the queue is resumed.
    let r3 = run_of(&c, "t1", "m3")["id"].clone();
    cmd(&c, json!({"type":"run.interrupt","commandId":"i1","threadId":"t1","runId":r3,"holdQueue":true})).unwrap();
    wait_run(&c, "t1", "m3", "interrupted");
    std::thread::sleep(Duration::from_millis(300));
    let r4 = run_of(&c, "t1", "m4");
    assert_eq!(
        (r4["status"].as_str(), r4["queueHeld"].as_bool()),
        (Some("queued"), Some(true))
    );
    cmd(
        &c,
        json!({"type":"queue.resume","commandId":"q8","threadId":"t1"}),
    )
    .unwrap();
    wait_run(&c, "t1", "m4", "waiting");
    assert_eq!(run_of(&c, "t1", "m4")["queuePosition"], Value::Null);
}

#[test]
fn queued_run_start_sends_a_held_message_now() {
    let (_s, c) = server(true);
    create(&c, "t1");
    send(&c, "t1", "m1", json!({"type":"start_immediately"})).unwrap();
    wait_run(&c, "t1", "m1", "waiting");
    send(&c, "t1", "m2", json!({"type":"queue_after_active"})).unwrap();
    let r1 = run_of(&c, "t1", "m1")["id"].clone();
    cmd(
        &c,
        json!({"type":"run.interrupt","commandId":"i","threadId":"t1","runId":r1,"holdQueue":true}),
    )
    .unwrap();
    wait_run(&c, "t1", "m1", "interrupted");
    let r2 = run_of(&c, "t1", "m2")["id"].clone();
    cmd(
        &c,
        json!({"type":"queued-run.start","commandId":"qs","threadId":"t1","runId":r2}),
    )
    .unwrap();
    wait_run(&c, "t1", "m2", "waiting");
}

#[test]
fn rollback_and_subagent_stop_validate_and_record() {
    let (s, c) = server(false);
    create(&c, "t1");
    let err = cmd(&c, json!({"type":"checkpoint.rollback","commandId":"rb0","threadId":"t1","scopeId":"scope-1","checkpointId":"cp-1"})).unwrap_err().to_string();
    assert!(err.contains("Checkpoint cp-1 was not found"), "{err}");
    let insert = |kind: &str, id: &str, body: Value| {
        s.state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES(?,?,?,?)",
                rusqlite_params(kind, id, &body),
            )
            .unwrap();
    };
    insert(
        "checkpoints",
        "cp-1",
        json!({"id":"cp-1","threadId":"t1","scopeId":"scope-1","status":"stale"}),
    );
    let err = cmd(&c, json!({"type":"checkpoint.rollback","commandId":"rb1","threadId":"t1","scopeId":"scope-1","checkpointId":"cp-1"})).unwrap_err().to_string();
    assert!(err.contains("stale"), "{err}");
    insert(
        "checkpoints",
        "cp-2",
        json!({"id":"cp-2","threadId":"t1","scopeId":"scope-1","status":"ready"}),
    );
    let err = cmd(&c, json!({"type":"checkpoint.rollback","commandId":"rb2","threadId":"t1","scopeId":"other","checkpointId":"cp-2"})).unwrap_err().to_string();
    assert!(err.contains("belongs to scope"), "{err}");
    assert!(cmd(&c, json!({"type":"checkpoint.rollback","commandId":"rb3","threadId":"t1","scopeId":"scope-1","checkpointId":"cp-2"})).is_err());

    insert(
        "subagents",
        "sub-1",
        json!({"id":"sub-1","threadId":"t1","runId":"run-1","status":"running","prompt":"x","result":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-01-01T00:00:00.000Z"}),
    );
    let err = cmd(&c, json!({"type":"subagent.stop","commandId":"ss0","threadId":"t1","runId":"run-9","subagentId":"sub-1"})).unwrap_err().to_string();
    assert!(err.contains("does not belong"), "{err}");
    let (control, mut controls) = tokio::sync::mpsc::channel(1);
    s.state
        .active
        .lock()
        .unwrap()
        .insert("run-1".into(), control);
    cmd(&c, json!({"type":"subagent.stop","commandId":"ss1","threadId":"t1","runId":"run-1","subagentId":"sub-1"})).unwrap();
    let sub: Value = {
        let body: String = s
            .state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT body FROM records WHERE kind='subagents' AND id='sub-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&body).unwrap()
    };
    assert_eq!(sub["status"], "cancelled");
    assert!(matches!(
        controls.try_recv().unwrap(),
        o3_server::provider::ProviderControl::Interrupt
    ));
    let err = cmd(&c, json!({"type":"subagent.stop","commandId":"ss2","threadId":"t1","runId":"run-1","subagentId":"sub-1"})).unwrap_err().to_string();
    assert!(err.contains("already cancelled"), "{err}");
}

fn rusqlite_params(kind: &str, id: &str, body: &Value) -> [String; 4] {
    [
        kind.into(),
        id.into(),
        body["threadId"].as_str().unwrap_or("t1").into(),
        body.to_string(),
    ]
}

#[test]
fn pull_request_links_are_metadata_on_the_thread() {
    let (_s, c) = server(false);
    create(&c, "t1");
    let key = json!({"host":"github.com","repository":"Org/Repo","number":7});
    let with = |ty: &str, id: &str, extra: Value| {
        let mut body = json!({"type":ty,"commandId":id,"threadId":"t1"});
        body.as_object_mut()
            .unwrap()
            .extend(key.as_object().unwrap().clone());
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        body
    };
    cmd(
        &c,
        with(
            "thread.pull-request.link",
            "l1",
            json!({"url":"https://github.com/Org/Repo/pull/7","source":"manual"}),
        ),
    )
    .unwrap();
    let links = thread(&c, "t1")["pullRequests"].clone();
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            links[0]["source"].as_str(),
            links[0]["number"].as_i64(),
            links[0]["snapshot"].is_null()
        ),
        (Some("manual"), Some(7), true)
    );
    cmd(
        &c,
        with(
            "thread.pull-request.link",
            "l2",
            json!({"url":"https://github.com/Org/Repo/pull/7","source":"manual"}),
        ),
    )
    .unwrap();
    assert_eq!(
        thread(&c, "t1")["pullRequests"].as_array().unwrap().len(),
        1,
        "linking twice keeps one link"
    );

    cmd(
        &c,
        with("thread.pull-request.watch", "w1", json!({"watching":true})),
    )
    .unwrap();
    let watch = thread(&c, "t1")["pullRequests"][0]["watch"].clone();
    assert_eq!(
        (watch["passed"].as_bool(), watch["wakes"].as_i64()),
        (Some(false), Some(0))
    );
    cmd(
        &c,
        with("thread.pull-request.watch", "w2", json!({"watching":false})),
    )
    .unwrap();
    let link = thread(&c, "t1")["pullRequests"][0].clone();
    assert!(
        link.get("watch").is_none() && link["endedWatch"].is_object(),
        "{link}"
    );

    let snapshot = json!({"state":"open","title":"A change","headBranch":"x","baseBranch":"main","isDraft":false,"updatedAt":null,"syncedAt":"2026-01-01T00:00:00.000Z"});
    cmd(
        &c,
        with(
            "thread.pull-request-link.sync",
            "y1",
            json!({"snapshot":snapshot,"stack":null}),
        ),
    )
    .unwrap();
    assert_eq!(
        thread(&c, "t1")["pullRequests"][0]["snapshot"]["title"],
        "A change"
    );
    cmd(&c, with("thread.pull-request.unlink", "u1", json!({}))).unwrap();
    assert!(thread(&c, "t1")["pullRequests"]
        .as_array()
        .unwrap()
        .is_empty());

    // Watching settled threads is refused, as upstream does.
    cmd(
        &c,
        json!({"type":"thread.settle","commandId":"st","threadId":"t1"}),
    )
    .unwrap();
    let err = cmd(&c, with("thread.pull-request.watch", "w3", json!({"watching":true,"link":{"url":"https://github.com/Org/Repo/pull/7","source":"agent"}}))).unwrap_err().to_string();
    assert!(err.contains("settled or archived"), "{err}");
}

#[test]
fn thread_find_navigates_occurrences_wraps_and_confines_to_thread() {
    let (_s, c) = server(false);
    create(&c, "find");
    create(&c, "other");
    for (id, text) in [("a", "Needle needle"), ("b", "one NEEDLE")] {
        cmd(&c,json!({"type":"message.dispatch","commandId":id,"messageId":id,"threadId":"find","text":text,"attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    }
    let find = |extra: Value| {
        let mut p = json!({"threadId":"find","query":"needle"});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        c.rpc("orchestration.searchThread", p).unwrap()
    };
    let first = find(json!({}));
    assert_eq!(first["totalMatches"], 3);
    assert_eq!(first["match"]["entryId"], "a");
    assert_eq!(first["navigation"][1]["startIndex"], 2);
    assert_eq!(find(json!({"offset":-1}))["match"]["entryId"], "b");
    assert_eq!(find(json!({"index":100}))["activeIndex"], 2);
    assert_eq!(
        find(json!({"start":{"entryId":"a","occurrence":1},"offset":1}))["match"]["entryId"],
        "b"
    );
    assert_eq!(find(json!({"threadId":"other"}))["totalMatches"], 0);
    assert!(c
        .rpc(
            "orchestration.searchThread",
            json!({"threadId":"missing","query":"needle"})
        )
        .is_err());
    let page = c
        .request_json(
            "GET",
            "/api/orchestration/threads/find/history?cursor=9223372036854775807",
            None,
            true,
        )
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
}
#[test]
fn source_control_discovery_has_honest_git_probe_and_option_encoding() {
    let (_s, c) = server(false);
    let v = c.rpc("server.discoverSourceControl", json!({})).unwrap();
    let git = &v["versionControlSystems"][0];
    assert_eq!(git["kind"], "git");
    assert_eq!(git["status"], "available");
    assert_eq!(git["version"]["_tag"], "Some");
    assert_eq!(git["detail"]["_tag"], "None");
    assert_eq!(v["versionControlSystems"][1]["implemented"], false);
    assert!(v["sourceControlProviders"].is_array());
}
