//! Receipt/event admission survives engine handover without importing provider effects.
#[path = "support/reference.rs"]
mod reference;
mod support;
use o3_server::{events, provider::ScriptedProvider, store::State, t3_shared};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{path::PathBuf, sync::Arc};

struct Counter(Arc<AtomicUsize>);
struct BeforeStarted {
    calls: Arc<AtomicUsize>,
    database: PathBuf,
}
impl o3_server::provider::Provider for BeforeStarted {
    fn execute<'a>(
        &'a self,
        input: o3_server::provider::RunInput,
        output: tokio::sync::mpsc::Sender<o3_server::provider::ProviderEvent>,
        _control: tokio::sync::mpsc::Receiver<o3_server::provider::ProviderControl>,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let native = Connection::open(&self.database).unwrap();
            let status: String = native
                .query_row(
                    "SELECT status FROM orchestration_v2_projection_runs WHERE run_id=?1",
                    [&input.run_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(status, "starting");
            let claimed:i64=native.query_row("SELECT count(*) FROM orchestration_v2_effect_outbox WHERE effect_type='provider-turn.start' AND status='running' AND attempt_count=1 AND lease_owner='o3' AND json_extract(payload_json,'$.runId')=?1",[&input.run_id],|r|r.get(0)).unwrap();
            assert_eq!(claimed, 1);
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _output = output;
            std::future::pending::<Result<(), String>>().await
        })
    }
}
impl o3_server::provider::Provider for Counter {
    fn execute<'a>(
        &'a self,
        _input: o3_server::provider::RunInput,
        output: tokio::sync::mpsc::Sender<o3_server::provider::ProviderEvent>,
        _control: tokio::sync::mpsc::Receiver<o3_server::provider::ProviderControl>,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            output
                .send(o3_server::provider::ProviderEvent::Started)
                .await
                .map_err(|e| e.to_string())?;
            output
                .send(o3_server::provider::ProviderEvent::Completed)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}

async fn serve(state: Arc<State>) -> tokio::task::JoinHandle<std::io::Result<()>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(o3_server::serve(listener, state));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = vec![];
    client.read_to_end(&mut response).await.unwrap();
    assert!(!response.is_empty());
    task
}

async fn completed(state: &State, run: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let status: String = state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT json_extract(body,'$.status') FROM records WHERE kind='runs' AND id=?1",
                    [run],
                    |row| row.get(0),
                )
                .unwrap();
            if status == "completed" {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Recovered native admission never executed");
}

fn profile() -> (PathBuf, Arc<State>, Connection) {
    profile_schema(include_str!("fixtures/t3-statev2-80.sql"))
}
fn profile_schema(schema: &str) -> (PathBuf, Arc<State>, Connection) {
    assert!(
        std::path::Path::new("/run/.containerenv").is_file()
            || std::path::Path::new("/.dockerenv").is_file()
    );
    let root = std::env::temp_dir().join(format!(
        "o3-shared-commands-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let state = Arc::new(State::open(&root).unwrap());
    *state.provider.lock().unwrap() = Some(Arc::new(ScriptedProvider));
    let native = Connection::open(t3_shared::database(&root)).unwrap();
    native.execute_batch(schema).unwrap();
    let _ = t3_shared::start(&state).unwrap();
    events::mutate_project(&state,&json!({"type":"project.create","commandId":"project","projectId":"p","title":"Synthetic commands","workspaceRoot":root})).unwrap();
    (root, state, native)
}
fn launch(state: &Arc<State>, id: &str) -> Value {
    events::launch(state, &launch_input(id)).unwrap()
}
fn launch_input(id: &str) -> Value {
    json!({"commandId":id,"projectId":"p","title":"Accepted launch","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"},"initialMessage":{"messageId":format!("message:{id}"),"scheduledTaskId":"synthetic-task","text":"Synthetic admission only","attachments":[]}})
}
fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}
fn snapshot_native(native: &Connection) -> (PathBuf, Arc<State>, Connection) {
    let root = std::env::temp_dir().join(format!(
        "o3-native-commands-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = t3_shared::database(&root);
    native
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let copy = Connection::open(path).unwrap();
    // A native-created profile has no previous O3 history cursor. Keep all real
    // projections, receipts and events; only reset this private exporter cursor.
    copy.execute("DELETE FROM o3_sync", []).unwrap();
    let state = Arc::new(State::open(&root).unwrap());
    *state.provider.lock().unwrap() = Some(Arc::new(ScriptedProvider));
    let _ = t3_shared::start(&state).unwrap();
    (root, state, copy)
}

#[tokio::test]
async fn accepted_launch_exports_native_receipts_with_original_run_creation_and_replays_after_import(
) {
    let (_root, state, mut native) = profile();
    let accepted = launch(&state, "scheduled-task:task:webhook:delivery:one");
    let thread = accepted["threadId"].as_str().unwrap();
    let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
    // The immutable native run.created must remain the original admission after
    // a later export replaces the current projection with its terminal state.
    state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.status','completed','$.completedAt','2026-11-06T23:00:00.000Z') WHERE kind='runs' AND id=?1",[run]).unwrap();
    t3_shared::export(&state, &mut native).unwrap();
    for (cid, kind, event) in [
        (
            "scheduled-task:task:webhook:delivery:one",
            "thread.create",
            "thread.created",
        ),
        (
            "scheduled-task:task:webhook:delivery:one:initial-message",
            "message.dispatch",
            "run.created",
        ),
    ] {
        let receipt:(String,String,String,String,i64,String,Option<String>)=native.query_row("SELECT aggregate_kind,aggregate_id,command_type,accepted_at,result_sequence,status,error FROM orchestration_command_receipts WHERE command_id=?1",[cid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!(receipt.0, "thread");
        assert_eq!(receipt.1, thread);
        assert_eq!(receipt.2, kind);
        assert_eq!(receipt.5, "accepted");
        assert!(receipt.6.is_none());
        let (sequence,payload):(i64,String)=native.query_row("SELECT sequence,payload_json FROM orchestration_events WHERE command_id=?1 AND event_type=?2",params![cid,event],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(receipt.4 >= sequence);
        if event == "run.created" {
            assert_eq!(
                serde_json::from_str::<Value>(&payload).unwrap()["status"],
                "preparing"
            );
        }
    }
    assert_eq!(count(&native, "orchestration_v2_effect_outbox"), 0);
    assert_eq!(t3_shared::export(&state, &mut native).unwrap(), 0);
    let (_root, imported, _copy) = snapshot_native(&native);
    let before = count(&imported.db.lock().unwrap(), "records");
    let replay = launch(&imported, "scheduled-task:task:webhook:delivery:one");
    assert_eq!(replay["threadId"], thread);
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["projection"]["runs"].as_array().unwrap().len(), 1);
    assert_eq!(count(&imported.db.lock().unwrap(), "records"), before);
    assert_eq!(count(&imported.db.lock().unwrap(), "thread_launches"), 0);
    assert!(imported.active.lock().unwrap().is_empty());
    let mismatch=events::dispatch(&imported,&json!({"type":"message.dispatch","commandId":"scheduled-task:task:webhook:delivery:one:initial-message","threadId":"another-thread","messageId":"other","text":"must not run","attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap_err();
    assert!(mismatch.contains("cannot be replayed"), "{mismatch}");
}

#[tokio::test]
async fn failed_native_receipt_commit_replays_existing_linked_events_without_duplicate_admission() {
    let (_root, state, mut native) = profile();
    native.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON orchestration_command_receipts WHEN NEW.command_id LIKE '%:initial-message' BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;").unwrap();
    let error = events::launch(&state, &launch_input("recover-receipt")).unwrap_err();
    assert!(error.contains("synthetic receipt failure"), "{error}");
    let events_before = count(&native, "orchestration_events");
    let records_before = count(&native, "orchestration_v2_projection_runs");
    assert_eq!(count(&native, "orchestration_command_receipts"), 1);
    native.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
    t3_shared::export(&state, &mut native).unwrap();
    assert_eq!(count(&native, "orchestration_events"), events_before);
    assert_eq!(
        count(&native, "orchestration_v2_projection_runs"),
        records_before
    );
    assert_eq!(count(&native, "orchestration_command_receipts"), 2);
    assert_eq!(t3_shared::export(&state, &mut native).unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_receipt_failure_holds_preparation_and_provider_until_durable_retry() {
    for preparation in [true, false] {
        let (_root, state, native) = profile();
        let calls = Arc::new(AtomicUsize::new(0));
        *state.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
        if !preparation {
            events::dispatch(&state,&json!({"type":"thread.create","commandId":"fence-thread","threadId":"fenced","projectId":"p","title":"Fenced thread","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        }
        native.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON orchestration_command_receipts WHEN NEW.command_type='message.dispatch' BEGIN SELECT RAISE(ABORT,'synthetic write-through failure'); END;").unwrap();
        let error=if preparation { events::launch(&state,&launch_input("fence-launch")) } else {events::dispatch(&state,&json!({"type":"message.dispatch","commandId":"fence-send","threadId":"fenced","messageId":"fenced-message","text":"Synthetic fence only","attachments":[],"dispatchMode":{"type":"start_immediately"}}))}.unwrap_err();
        assert!(
            error.starts_with("shared_admission_recovery_required:"),
            "{error}"
        );
        let run: String = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT id FROM records WHERE kind='runs'", [], |r| r.get(0))
            .unwrap();
        let task = serve(state.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        if preparation {
            let phase: String = state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT phase FROM thread_launches", [], |r| r.get(0))
                .unwrap();
            assert_eq!(phase, "admitted");
        }
        native.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
        completed(&state, &run).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(count(&state.db.lock().unwrap(), "o3_command_admissions"), 2);
        assert_eq!(count(&native, "orchestration_v2_projection_runs"), 1);
        task.abort();
    }
}

#[tokio::test]
async fn partial_launch_claim_import_does_not_claim_an_unaccepted_initial_message() {
    let (_root, state, mut native) = profile();
    let reply=events::launch(&state,&json!({"commandId":"claim-only","projectId":"p","title":"Accepted thread only","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default","workspaceStrategy":{"type":"root"}})).unwrap();
    t3_shared::export(&state, &mut native).unwrap();
    assert_eq!(count(&native, "orchestration_command_receipts"), 1);
    let (_root, imported, _copy) = snapshot_native(&native);
    let run: Option<String> = imported
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT run_id FROM launch_claims WHERE command_id='claim-only'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(run.is_none());
    let accepted = launch(&imported, "claim-only");
    assert_eq!(accepted["threadId"], reply["threadId"]);
    assert_eq!(accepted["projection"]["runs"].as_array().unwrap().len(), 1);
    assert!(imported.active.lock().unwrap().is_empty());
}

#[tokio::test]
async fn conflicting_native_receipt_is_retained_and_new_journal_is_receipt_atomic() {
    let (_root, state, native) = profile();
    native.execute("INSERT INTO orchestration_command_receipts VALUES('collision','thread','different-thread','2026-11-06T00:00:00.000Z',0,'accepted',NULL,'thread.create')",[]).unwrap();
    let error = events::launch(&state, &launch_input("collision")).unwrap_err();
    assert!(error.contains("receipt conflict"), "{error}");
    assert_eq!(native.query_row("SELECT aggregate_id FROM orchestration_command_receipts WHERE command_id='collision'",[],|r|r.get::<_,String>(0)).unwrap(),"different-thread");
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM o3_command_admissions WHERE command_id LIKE 'collision%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    state.db.lock().unwrap().execute_batch("CREATE TRIGGER refuse_admission BEFORE INSERT ON o3_command_admissions BEGIN SELECT RAISE(ABORT,'synthetic admission refusal'); END;").unwrap();
    let error=events::dispatch(&state,&json!({"type":"thread.create","commandId":"unaccepted","threadId":"unaccepted","projectId":"p","title":"must roll back","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap_err();
    assert!(error.contains("synthetic admission refusal"), "{error}");
    let db = state.db.lock().unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM receipts WHERE command_id='unaccepted'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM records WHERE kind='thread' AND id='unaccepted'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_preparing_and_unclaimed_start_recover_existing_run_once() {
    for starting in [false, true] {
        let (_root, state, mut native) = profile();
        let accepted = launch(&state, "recover-unstarted");
        let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
        t3_shared::export(&state, &mut native).unwrap();
        if starting {
            native.execute("UPDATE orchestration_v2_projection_runs SET status='starting',payload_json=json_set(payload_json,'$.status','starting') WHERE run_id=?1",[run]).unwrap();
            native.execute("INSERT INTO orchestration_v2_effect_outbox(effect_id,command_id,thread_id,effect_type,payload_json,status,attempt_count,available_at,created_at,updated_at) VALUES('pending-native-start','recover-unstarted:initial-message',?1,'provider-turn.start',?2,'pending',0,'2026-11-06T00:00:00.000Z','2026-11-06T00:00:00.000Z','2026-11-06T00:00:00.000Z')",params![accepted["threadId"].as_str().unwrap(),json!({"type":"provider-turn.start","runId":run}).to_string()]).unwrap();
        }
        let (_root, imported, _copy) = snapshot_native(&native);
        assert_eq!(count(&imported.db.lock().unwrap(), "thread_launches"), 1);
        let calls = Arc::new(AtomicUsize::new(0));
        *imported.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
        let task = serve(imported.clone()).await;
        completed(&imported, run).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            imported
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM records WHERE kind='runs'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let replay = launch(&imported, "recover-unstarted");
        assert_eq!(replay["threadId"], accepted["threadId"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_native_start_or_workspace_remains_explicitly_held_without_execution() {
    for workspace in [false, true] {
        let (_root, state, mut native) = profile();
        let accepted = launch(&state, "held-native");
        let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
        t3_shared::export(&state, &mut native).unwrap();
        if workspace {
            native.execute("UPDATE orchestration_v2_projection_runs SET payload_json=json_set(payload_json,'$.workspacePreparation',json('{\"type\":\"worktree\",\"baseRef\":\"main\"}')) WHERE run_id=?1",[run]).unwrap();
        } else {
            native.execute("UPDATE orchestration_v2_projection_runs SET status='starting',payload_json=json_set(payload_json,'$.status','starting') WHERE run_id=?1",[run]).unwrap();
            native.execute("INSERT INTO orchestration_v2_effect_outbox(effect_id,command_id,thread_id,effect_type,payload_json,status,attempt_count,available_at,created_at,updated_at) VALUES('claimed-native-start','held-native:initial-message',?1,'provider-turn.start',?2,'running',1,'2026-11-06T00:00:00.000Z','2026-11-06T00:00:00.000Z','2026-11-06T00:00:00.000Z')",params![accepted["threadId"].as_str().unwrap(),json!({"type":"provider-turn.start","runId":run}).to_string()]).unwrap();
        }
        let (_root, imported, _copy) = snapshot_native(&native);
        assert_eq!(count(&imported.db.lock().unwrap(), "thread_launches"), 0);
        assert_eq!(
            imported
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT status FROM o3_shared_admission_recovery WHERE run_id=?1",
                    [run],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "blocked"
        );
        let calls = Arc::new(AtomicUsize::new(0));
        *imported.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
        let task = serve(imported.clone()).await;
        let error = events::launch(
            &imported,
            &json!({"commandId":"held-native","projectId":"p","workspaceStrategy":{"type":"root"}}),
        )
        .unwrap_err();
        assert!(
            error.starts_with("shared_admission_recovery_required:"),
            "{error}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(imported.active.lock().unwrap().is_empty());
        task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_queue_resumes_but_already_started_or_terminal_admissions_are_not_restarted() {
    for status in ["queued", "running", "completed"] {
        let (_root, state, mut native) = profile();
        let accepted = launch(&state, "native-lifecycle");
        let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
        t3_shared::export(&state, &mut native).unwrap();
        if status == "queued" {
            native.execute("UPDATE orchestration_v2_projection_runs SET status='queued',payload_json=json_set(payload_json,'$.status','queued','$.activeAttemptId',NULL,'$.rootNodeId',NULL,'$.providerThreadId',NULL,'$.queuePosition',1,'$.queueHeld',json('false')) WHERE run_id=?1",[run]).unwrap();
        } else {
            native.execute("UPDATE orchestration_v2_projection_runs SET status=?1,payload_json=json_set(payload_json,'$.status',?1,'$.startedAt','2026-11-06T00:00:00.000Z') WHERE run_id=?2",params![status,run]).unwrap();
        }
        let (_root, imported, _copy) = snapshot_native(&native);
        let calls = Arc::new(AtomicUsize::new(0));
        *imported.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
        let task = serve(imported.clone()).await;
        if status == "queued" {
            completed(&imported, run).await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(imported.active.lock().unwrap().is_empty());
        }
        assert_eq!(imported.db.lock().unwrap().query_row("SELECT count(*) FROM records WHERE kind='messages' AND json_extract(body,'$.role')='user'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        task.abort();
    }
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference --test shared_command_receipts in rootless Podman"]
fn official_nightly_replays_o3_launch_receipts_without_a_second_run() {
    let (root, state, mut db) = profile();
    let input = launch_input("official-receipt-replay");
    let accepted = events::launch(&state, &input).unwrap();
    let thread = accepted["threadId"].as_str().unwrap();
    let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
    // Terminal work is used so this acceptance cannot request inference even if
    // native restart recovery runs while its public replay RPC is evaluated.
    state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.status','completed','$.completedAt','2026-11-06T23:00:00.000Z') WHERE kind IN ('runs','attempts','turnItems')", []).unwrap();
    t3_shared::export(&state, &mut db).unwrap();
    let userdata = root.join("reference/userdata");
    std::fs::create_dir_all(&userdata).unwrap();
    db.execute(
        "VACUUM INTO ?1",
        [userdata.join("statev2.sqlite").to_str().unwrap()],
    )
    .unwrap();
    let (native, process) = reference::start(&root);
    let replay = native
        .rpc("orchestration.launchThread", input.clone())
        .unwrap();
    assert_eq!(replay["threadId"], thread);
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["projection"]["runs"].as_array().unwrap().len(), 1);
    assert_eq!(replay["projection"]["runs"][0]["id"], run);
    // A second replay through the native transport must also remain effect-free.
    let raw = process.raw_exit("orchestration.launchThread", input);
    assert!(
        !raw.to_string()
            .contains("Initial message was accepted without a durable run"),
        "{raw}"
    );
    let native_db = Connection::open(userdata.join("statev2.sqlite")).unwrap();
    assert_eq!(count(&native_db, "orchestration_v2_projection_runs"), 1);
    assert_eq!(
        count(&native_db, "orchestration_v2_projection_provider_turns"),
        0
    );
    assert_eq!(count(&native_db, "orchestration_command_receipts"), 2);
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference --test shared_command_receipts in rootless Podman"]
fn official_native_unstarted_admission_imports_and_recovers_the_same_run_once() {
    let root = std::env::temp_dir().join(format!(
        "o3-native-admission-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (native, process) = reference::start(&root);
    native.rpc("projects.mutate",json!({"type":"project.create","commandId":"native-project","projectId":"p","title":"Native admission fixture","workspaceRoot":root})).unwrap();
    native.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"native-create","createdBy":"user","creationSource":"web","threadId":"native-thread","projectId":"p","title":"Native accepted turn","modelSelection":{"instanceId":"codex","model":"gpt-6.1-sol","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
    let message = json!({"type":"message.dispatch","commandId":"native-message","createdBy":"user","creationSource":"web","threadId":"native-thread","messageId":"native-message-id","text":"Synthetic admission with no inference","attachments":[],"dispatchMode":{"type":"defer_start","workspaceStrategy":{"type":"root"}}});
    native
        .rpc("orchestration.dispatchCommand", message.clone())
        .unwrap();
    let projection = native
        .rpc(
            "orchestration.getThreadProjection",
            json!({"threadId":"native-thread"}),
        )
        .unwrap();
    let run = projection["runs"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(projection["runs"][0]["status"], "preparing");
    assert!(projection["providerTurns"].as_array().unwrap().is_empty());
    drop(native);
    drop(process);
    let state = Arc::new(State::open(&root.join("reference/userdata")).unwrap());
    let _ = t3_shared::start(&state).unwrap();
    assert_eq!(count(&state.db.lock().unwrap(), "o3_command_admissions"), 2);
    let calls = Arc::new(AtomicUsize::new(0));
    *state.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = serve(state.clone()).await;
        completed(&state, &run).await;
        events::dispatch(&state, &message).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(count(&state.db.lock().unwrap(), "o3_command_admissions"), 2);
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM records WHERE kind='runs'", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        server.abort();
    });
}

#[tokio::test]
async fn schema73_refuses_pending_webhook_sharing_without_losing_local_admission() {
    let (_root, state, native) = profile_schema(include_str!("fixtures/t3-statev2-73.sql"));
    let command = json!({"type":"thread.create","commandId":"schema-empty","threadId":"schema-empty","projectId":"p","title":"Legacy schema fence","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"default"});
    events::dispatch(&state, &command).unwrap();
    assert_eq!(count(&native, "orchestration_command_receipts"), 1);
    state.db.lock().unwrap().execute("INSERT INTO scheduled_task_webhook_pending VALUES('pending-native','task','',0,'pending','{}','{}','[]')",[]).unwrap();
    let mut blocked = command.clone();
    blocked["commandId"] = json!("schema-blocked");
    blocked["threadId"] = json!("schema-blocked");
    let error = events::dispatch(&state, &blocked).unwrap_err();
    assert!(
        error.starts_with("shared_admission_recovery_required:"),
        "{error}"
    );
    assert!(error.contains("schema 80"), "{error}");
    assert_eq!(
        count(&state.db.lock().unwrap(), "scheduled_task_webhook_pending"),
        1
    );
    assert_eq!(count(&state.db.lock().unwrap(), "o3_command_admissions"), 2);
    assert_eq!(count(&state.db.lock().unwrap(), "receipts"), 3); // Project and both accepted threads.
    assert_eq!(count(&native, "orchestration_command_receipts"), 1);
    assert!(state.active.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_effect_claim_failure_never_enters_the_provider_adapter() {
    let (_root, state, mut native) = profile();
    let accepted = launch(&state, "effect-fence");
    let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    *state.provider.lock().unwrap() = Some(Arc::new(Counter(calls.clone())));
    native.execute_batch("CREATE TRIGGER refuse_claim BEFORE INSERT ON orchestration_v2_effect_outbox BEGIN SELECT RAISE(ABORT,'synthetic effect claim failure'); END;").unwrap();
    let server = serve(state.clone()).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        count(&state.db.lock().unwrap(), "o3_shared_effect_claims"),
        1
    );
    assert_eq!(count(&native, "orchestration_v2_effect_outbox"), 0);
    native.execute_batch("DROP TRIGGER refuse_claim;").unwrap();
    completed(&state, run).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    t3_shared::export(&state, &mut native).unwrap();
    assert_eq!(count(&native, "orchestration_v2_effect_outbox"), 1);
    let status: String = native
        .query_row(
            "SELECT status FROM orchestration_v2_effect_outbox",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "succeeded");
    server.abort();
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference --test shared_command_receipts in rootless Podman"]
fn official_native_recovery_cancels_the_claim_before_provider_started_is_observed() {
    let (root, state, native) = profile();
    let calls = Arc::new(AtomicUsize::new(0));
    *state.provider.lock().unwrap() = Some(Arc::new(BeforeStarted {
        calls: calls.clone(),
        database: t3_shared::database(&root),
    }));
    let accepted = launch(&state, "claimed-crash");
    let thread = accepted["threadId"].as_str().unwrap();
    let run = accepted["projection"]["runs"][0]["id"].as_str().unwrap();
    let userdata = root.join("reference/userdata");
    std::fs::create_dir_all(&userdata).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = serve(state.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Provider did not reach its durable claimed boundary");
        native
            .execute(
                "VACUUM INTO ?1",
                [userdata.join("statev2.sqlite").to_str().unwrap()],
            )
            .unwrap();
        server.abort();
    });
    drop(runtime);
    let (client, process) = reference::start(&root);
    let replay = client
        .rpc("orchestration.launchThread", launch_input("claimed-crash"))
        .unwrap();
    assert_eq!(replay["resumed"], true);
    assert_eq!(replay["threadId"], thread);
    assert_eq!(replay["projection"]["runs"].as_array().unwrap().len(), 1);
    assert_eq!(replay["projection"]["runs"][0]["id"], run);
    assert_eq!(replay["projection"]["runs"][0]["status"], "cancelled");
    let db = Connection::open(userdata.join("statev2.sqlite")).unwrap();
    let status:String=db.query_row("SELECT status FROM orchestration_v2_effect_outbox WHERE effect_id LIKE 'effect:o3-start:%'",[],|r|r.get(0)).unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(count(&db, "orchestration_v2_projection_provider_turns"), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(process);
}
