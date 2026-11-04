//! Server operations through the real client: activity, secrets, scheduled tasks, diagnostics, cleanup.
mod support;
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use support::{start, subscribe, wait_for, Server};
use t3_protocol::Connection;

const T: Duration = Duration::from_secs(10);

fn with_provider() -> (Server, Connection) {
    let s = start(|state| {
        *state.provider.lock().unwrap() = Some(Arc::new(o3_server::provider::ScriptedProvider));
    });
    let c = s.connect();
    (s, c)
}
fn plain() -> (Server, Connection) {
    let s = start(|_| {});
    let c = s.connect();
    (s, c)
}
fn err(r: t3_protocol::Result<Value>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn client_activity_and_provider_refresh() {
    let (s, c) = plain();
    let report = json!({"clientId":"phone","clientKind":"mobile","visible":true,"focused":true,"recentlyInteracted":false,"appState":"active","scopes":[{"type":"server-config"}],"observedAt":"2026-01-01T00:00:00.000Z","ttlMs":60000});
    assert_eq!(
        c.rpc("server.reportClientActivity", report.clone())
            .unwrap(),
        Value::Null
    );
    assert_eq!(s.state.streams.ops.active_clients(), 1);
    let mut hidden = report.clone();
    hidden["visible"] = json!(false);
    c.rpc("server.reportClientActivity", hidden).unwrap();
    assert_eq!(
        s.state.streams.ops.active_clients(),
        0,
        "a newer report replaces the old one"
    );
    let mut bad = report;
    bad["clientKind"] = json!("toaster");
    assert!(c.rpc("server.reportClientActivity", bad).is_err());
    // No provider adapters are installed, so the refreshed list is the configured one: empty.
    assert_eq!(
        c.rpc("server.refreshProviders", json!({"fresh":true}))
            .unwrap(),
        json!({"providers":[]})
    );
}

#[test]
fn secret_answers_are_stored_privately_and_recorded_without_the_value() {
    let (s, c) = plain();
    c.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":"c","threadId":"t1","projectId":"p1","title":"T","modelSelection":{"instanceId":"x","model":"m","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
    let item = |id: &str| json!({"id":id,"threadId":"t1","runId":null,"nodeId":null,"ordinal":0,"status":"waiting","type":"secret_request","label":"API key","reason":"needed","delivery":"file","secretStatus":"pending","updatedAt":"2026-01-01T00:00:00.000Z"});
    for id in ["sec-1", "sec-2"] {
        s.state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('turnItems',?,'t1',?)",
                [id.to_string(), item(id).to_string()],
            )
            .unwrap();
    }
    let answer = |id: &str, a: Value| {
        c.rpc(
            "secrets.answerRequest",
            json!({"threadId":"t1","turnItemId":id,"answer":a}),
        )
    };
    // The client reports the error's tag; the failure reason is on the wire, so look at the raw exit.
    let reason = |id: &str, a: Value| {
        support::raw_exit(
            &s,
            "secrets.answerRequest",
            json!({"threadId":"t1","turnItemId":id,"answer":a}),
        )["exit"]["cause"][0]["error"]
            .clone()
    };
    assert_eq!(
        reason("missing", json!({"type":"decline"})),
        json!({"_tag":"SecretRequestError","reason":"not_found"})
    );
    assert!(answer("sec-1", json!({"type":"save","secret":"  "})).is_err());
    answer("sec-1", json!({"type":"save","secret":"hunter2-value"})).unwrap();
    let body: String = s
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT body FROM records WHERE kind='turnItems' AND id='sec-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["secretStatus"],
        "saved"
    );
    let file = o3_server::ops::secret_path(&s.state, "t1", "sec-1");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hunter2-value");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // The value is in the file only: not in the database, not in the event log.
    let leaked: i64 = s.state.db.lock().unwrap().query_row("SELECT (SELECT count(*) FROM records WHERE body LIKE '%hunter2%') + (SELECT count(*) FROM events WHERE body LIKE '%hunter2%')", [], |r| r.get(0)).unwrap();
    assert_eq!(leaked, 0);
    assert_eq!(
        reason("sec-1", json!({"type":"decline"}))["reason"],
        "already_answered"
    );
    answer("sec-2", json!({"type":"decline"})).unwrap();
    assert!(!o3_server::ops::secret_path(&s.state, "t1", "sec-2").exists());
    let body: String = s
        .state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT body FROM records WHERE kind='turnItems' AND id='sec-2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["secretStatus"],
        "declined"
    );
}

fn task(title: &str, schedule: Value) -> Value {
    json!({"title":title,"prompt":"do the thing","enabled":true,"schedule":schedule,"projectId":"p1","threadId":null,"workspaceStrategy":{"type":"root"},"modelSelection":{"instanceId":"scripted","model":"m","options":[]},"runtimeMode":"full-access","interactionMode":"default"})
}
fn shell_threads(c: &Connection) -> Vec<Value> {
    c.request_json("GET", "/api/orchestration/shell", None, true)
        .unwrap()["threads"]
        .as_array()
        .unwrap()
        .clone()
}
fn launch_project(s: &Server, c: &Connection) {
    let root = s.dir.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"project","projectId":"p1","title":"Scheduled","workspaceRoot":root})).unwrap();
}

#[test]
fn scheduled_tasks_are_stored_validated_and_launch_threads() {
    let (s, c) = with_provider();
    launch_project(&s, &c);
    let created = c
        .rpc(
            "scheduledTasks.upsert",
            task("Nightly", json!({"type":"interval","everyMs":60000})),
        )
        .unwrap()["task"]
        .clone();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            created["lastRunStatus"].as_str(),
            created["runCount"].as_i64(),
            created["enabled"].as_bool()
        ),
        (Some("never"), Some(0), Some(true))
    );
    assert!(created["nextRunAt"].is_string());
    assert_eq!(
        c.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert!(err(c.rpc(
        "scheduledTasks.upsert",
        task("Fast", json!({"type":"interval","everyMs":1000}))
    ))
    .contains("60000"));
    assert!(err(c.rpc(
        "scheduledTasks.upsert",
        task("Hook", json!({"type":"webhook","signature":null}))
    ))
    .contains("Webhook"));
    assert!(err(c.rpc(
        "scheduledTasks.upsert",
        task("Time", json!({"type":"fixed_time","timeOfDay":"25:00"}))
    ))
    .contains("HH:MM"));
    let mut missing = task("Ghost", json!({"type":"interval","everyMs":60000}));
    missing["id"] = json!("nope");
    missing["requireExisting"] = json!(true);
    assert!(err(c.rpc("scheduledTasks.upsert", missing)).contains("no longer exists"));

    // Editing keeps identity and history.
    let mut edit = task("Renamed", json!({"type":"interval","everyMs":120000}));
    edit["id"] = json!(id);
    let edited = c.rpc("scheduledTasks.upsert", edit).unwrap()["task"].clone();
    assert_eq!(
        (
            edited["id"].as_str(),
            edited["title"].as_str(),
            edited["createdAt"].clone()
        ),
        (
            Some(id.as_str()),
            Some("Renamed"),
            created["createdAt"].clone()
        )
    );

    let off = c
        .rpc(
            "scheduledTasks.setEnabled",
            json!({"id":id,"enabled":false}),
        )
        .unwrap()["task"]
        .clone();
    assert_eq!(
        (off["enabled"].as_bool(), off["nextRunAt"].is_null()),
        (Some(false), true)
    );

    // Run now: a thread appears carrying the prompt, and the task records the run.
    let ran = c.rpc("scheduledTasks.runNow", json!({"id":id})).unwrap()["task"].clone();
    assert_eq!(
        (ran["lastRunStatus"].as_str(), ran["runCount"].as_i64()),
        (Some("succeeded"), Some(1))
    );
    assert!(
        ran["lastRunAt"].is_string() && ran["nextRunAt"].is_null(),
        "disabled stays unscheduled"
    );
    let threads = shell_threads(&c);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["title"], "Renamed");
    let tid = threads[0]["id"].as_str().unwrap();
    let snap = c
        .request_json(
            "GET",
            &format!("/api/orchestration/threads/{tid}/bounded"),
            None,
            true,
        )
        .unwrap();
    assert!(snap["projection"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "do the thing"));

    // The timer: a due task fires without anyone asking.
    c.rpc("scheduledTasks.setEnabled", json!({"id":id,"enabled":true}))
        .unwrap();
    {
        let db = s.state.db.lock().unwrap();
        let body: String = db
            .query_row(
                "SELECT body FROM records WHERE kind='scheduledTasks' AND id=?",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let mut t: Value = serde_json::from_str(&body).unwrap();
        t["nextRunAt"] = json!("2001-01-01T00:00:00.000Z");
        db.execute(
            "UPDATE records SET body=? WHERE kind='scheduledTasks' AND id=?",
            [t.to_string(), id.clone()],
        )
        .unwrap();
    }
    s.state.streams.ops.wake_scheduler();
    assert!(
        wait_for(T, || shell_threads(&c).len() == 2
            && c.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"][0]
                ["runCount"]
                == 2),
        "the timer never launched the due task"
    );
    let after = c.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"][0].clone();
    assert_eq!(after["runCount"], 2);
    assert!(after["nextRunAt"].as_str().unwrap() > "2026");

    c.rpc("scheduledTasks.delete", json!({"id":id})).unwrap();
    assert!(c.rpc("scheduledTasks.list", json!({})).unwrap()["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(err(c.rpc("scheduledTasks.runNow", json!({"id":id}))).contains("does not exist"));
    assert!(err(c.rpc("scheduledTasks.delete", json!({"id":id}))).contains("does not exist"));
}

#[test]
fn a_task_that_cannot_launch_records_the_failure() {
    let (s, c) = plain();
    launch_project(&s, &c);
    let id = c
        .rpc(
            "scheduledTasks.upsert",
            task(
                "Orphan",
                json!({"type":"fixed_time","timeOfDay":"03:30","weekdays":[1,3]}),
            ),
        )
        .unwrap()["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ran = c.rpc("scheduledTasks.runNow", json!({"id":id})).unwrap()["task"].clone();
    assert_eq!(ran["lastRunStatus"], "failed");
    assert!(
        ran["lastRunError"].as_str().unwrap().contains("provider"),
        "{ran}"
    );
    assert!(shell_threads(&c).is_empty(), "nothing was half-created");
}

#[test]
fn process_diagnostics_and_signals_cover_only_the_servers_own_tree() {
    let s = start(|state| state.streams.terminals.set_shell("/bin/sh"));
    let c = s.connect();
    let cwd = s.dir.join("w");
    std::fs::create_dir_all(&cwd).unwrap();
    let snap = c
        .rpc(
            "terminal.open",
            json!({"threadId":"t1","terminalId":"term-1","cwd":cwd}),
        )
        .unwrap();
    let shell_pid = snap["pid"].as_i64().unwrap();
    let d = c.rpc("server.getProcessDiagnostics", json!({})).unwrap();
    assert_eq!(d["serverPid"].as_u64(), Some(std::process::id() as u64));
    assert_eq!(d["error"], json!({"_tag":"None"}));
    let procs = d["processes"].as_array().unwrap();
    assert_eq!(
        (procs[0]["pid"].as_u64(), procs[0]["depth"].as_u64()),
        (Some(std::process::id() as u64), Some(0))
    );
    assert!(d["totalRssBytes"].as_u64().unwrap() > 1_000_000);
    let shell = procs
        .iter()
        .find(|p| p["pid"] == shell_pid)
        .expect("the terminal shell is listed");
    assert_eq!(
        (shell["ppid"].as_u64(), shell["depth"].as_u64()),
        (Some(std::process::id() as u64), Some(1))
    );
    assert_eq!(shell["pgid"], json!({"_tag":"Some","value":shell_pid}));
    assert!(procs[0]["childPids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| *p == shell_pid));
    let started = shell["startTimeMs"].as_i64().unwrap();

    let signal = |pid: i64, start: i64, sig: &str| {
        c.rpc(
            "server.signalProcess",
            json!({"pid":pid,"startTimeMs":start,"signal":sig}),
        )
        .unwrap()
    };
    assert_eq!(
        signal(shell_pid, started + 5, "SIGKILL")["signaled"],
        false,
        "a stale start time protects a recycled pid"
    );
    assert_eq!(
        signal(1, 0, "SIGKILL")["signaled"],
        false,
        "pid 1 is not ours"
    );
    assert_eq!(
        signal(std::process::id() as i64, 0, "SIGKILL")["signaled"],
        false,
        "never the server itself"
    );
    assert!(c
        .rpc(
            "server.signalProcess",
            json!({"pid":shell_pid,"startTimeMs":started,"signal":"SIGTERM"})
        )
        .is_err());
    let ok = signal(shell_pid, started, "SIGKILL");
    assert_eq!(
        (ok["signaled"].as_bool(), ok["message"].clone()),
        (Some(true), json!({"_tag":"None"}))
    );
    let mut obs = subscribe(
        c.clone(),
        "terminal.observe",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    obs.next(T);
    assert!(wait_for(T, || {
        let d = c.rpc("server.getProcessDiagnostics", json!({})).unwrap();
        d["processes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["pid"] != shell_pid || p["status"] == "Z")
    }));

    // Diagnostics need their own scopes.
    let reader = s.limited(&c, &["orchestration:read"]);
    assert!(err(reader.rpc("server.getProcessDiagnostics", json!({}))).contains("diagnostics:read"));
    assert!(err(reader.rpc(
        "server.signalProcess",
        json!({"pid":2,"startTimeMs":0,"signal":"SIGINT"})
    ))
    .contains("environment:maintain"));
}

#[test]
fn resource_telemetry_usage_and_cleanup() {
    let (s, c) = plain();
    let h = c
        .rpc(
            "server.getResourceTelemetryHistory",
            json!({"windowMs":60000,"bucketMs":10000}),
        )
        .unwrap();
    assert_eq!(
        (h["windowMs"].as_u64(), h["bucketMs"].as_u64()),
        (Some(60000), Some(10000))
    );
    let buckets = h["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 6);
    assert!(h["retainedSampleCount"].as_u64().unwrap() >= 1);
    assert!(
        buckets
            .iter()
            .any(|b| b["maxRssBytes"].as_u64().unwrap() > 1_000_000),
        "the present sample lands in a bucket"
    );
    assert_eq!(h["topProcesses"][0]["category"], "server");
    assert_eq!(h["health"]["native"]["status"], "healthy");
    let clamped = c
        .rpc(
            "server.getResourceTelemetryHistory",
            json!({"windowMs":10,"bucketMs":99999999}),
        )
        .unwrap();
    assert_eq!(
        (clamped["windowMs"].as_u64(), clamped["bucketMs"].as_u64()),
        (Some(1000), Some(1000))
    );

    let u = c
        .rpc(
            "server.getUsageSummary",
            json!({"sinceDay":"2026-01-01","untilDay":"2026-01-31","timeZone":"UTC"}),
        )
        .unwrap();
    assert_eq!(
        (
            u["contractVersion"].as_i64(),
            u["buckets"].clone(),
            u["pricing"]["status"].as_str()
        ),
        (Some(6), json!([]), Some("unavailable"))
    );

    // Only old log files of this profile are removed.
    let logs = s.dir.join("logs/terminals");
    std::fs::create_dir_all(&logs).unwrap();
    let old = logs.join("old.log");
    let fresh = logs.join("fresh.log");
    std::fs::write(&old, "x".repeat(1234)).unwrap();
    std::fs::write(&fresh, "y").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(20 * 86400))
        .unwrap();
    let outside = s.dir.join("keep.log");
    std::fs::write(&outside, "z").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&outside)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(90 * 86400))
        .unwrap();
    let mut report = subscribe(c.clone(), "server.getStorageCleanupReport", json!({}));
    assert_eq!(report.next(T), Value::Null, "no cleanup has run yet");
    let r = c.rpc("server.runStorageCleanup", json!({})).unwrap();
    assert_eq!(r["trigger"], "manual");
    assert_eq!(
        (
            r["counts"]["removed"].as_i64(),
            r["counts"]["kept"].as_i64(),
            r["counts"]["failed"].as_i64(),
            r["bytesFreed"].as_i64()
        ),
        (Some(1), Some(1), Some(0), Some(1234))
    );
    assert_eq!(
        (
            r["entries"][0]["kind"].as_str(),
            r["entries"][0]["outcome"].as_str()
        ),
        (Some("logs"), Some("removed"))
    );
    assert!(!old.exists() && fresh.exists() && outside.exists());
    assert_eq!(report.next(T), r);
    let reader = s.limited(&c, &["orchestration:read"]);
    assert!(err(reader.rpc("server.runStorageCleanup", json!({}))).contains("settings:write"));
}
