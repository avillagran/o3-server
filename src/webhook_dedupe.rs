//! Pinned native webhook replay and coalescing SQL. The caller owns admission's
//! SQLite transaction and must roll it back on every error returned here.
use crate::webhook_payload::Request;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

const REPLAY_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Run,
    Pending,
    Coalesced { owner: String, logged: bool },
    Duplicate,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    FirstRejected,
    Rejected,
    QueueFull,
    Persistence(String),
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Persistence(error.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Persistence(error.to_string())
    }
}

/// Memory reservations intentionally have the same order as native admission.
/// A coalesced request takes no new run/capacity slot, but still updates the log
/// owner's request even when its own suppression-log allowance is exhausted.
pub(crate) trait Admission {
    fn check_capacity(&mut self, db: &Connection) -> Result<()>;
    fn reserve(&mut self) -> Result<()>;
    fn reserve_coalesced_log(&mut self) -> bool;
    fn replace_request(&mut self, db: &Connection, owner: &str) -> Result<()>;
}

fn task_id(task: &Value) -> Result<&str> {
    task["id"]
        .as_str()
        .ok_or_else(|| Error::Persistence("Validated webhook task has no text id".into()))
}

fn dedupe(task: &Value) -> Option<&Value> {
    (task["schedule"]["type"] == "webhook")
        .then(|| task["schedule"].get("dedupe"))
        .flatten()
}

fn has_replay(db: &Connection, task: &str, key: &str, now: i64) -> Result<bool> {
    if key.is_empty() {
        return Ok(false);
    }
    Ok(db
        .query_row(
            "SELECT 1 FROM scheduled_task_webhook_replay_keys WHERE task_id=?1 AND replay_key=?2 AND expires_at>?3",
            params![task, key, now],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn claim_replay(db: &Connection, task: &str, key: &str, delivery: &str, now: i64) -> Result<()> {
    if key.is_empty() {
        return Ok(());
    }
    let expires = now
        .checked_add(REPLAY_TTL_MS)
        .ok_or_else(|| Error::Persistence("Webhook replay expiry exceeds SQLite range".into()))?;
    db.execute(
        "DELETE FROM scheduled_task_webhook_replay_keys WHERE expires_at<=?1",
        [now],
    )?;
    db.execute(
        "INSERT INTO scheduled_task_webhook_replay_keys(task_id,replay_key,delivery_id,expires_at) VALUES(?1,?2,?3,?4)",
        params![task, key, delivery, expires],
    )?;
    Ok(())
}

pub(crate) fn release_replay(db: &Connection, delivery: &str) -> Result<()> {
    db.execute(
        "DELETE FROM scheduled_task_webhook_replay_keys WHERE delivery_id=?1",
        [delivery],
    )?;
    Ok(())
}

/// Coalescing admission of an already validated task and request. `Run` reserves
/// and claims nothing; its caller proceeds through `accept_immediate` afterward.
pub(crate) fn accept(
    db: &Connection,
    task: &Value,
    request: &Request,
    delivery: &str,
    now: i64,
    admission: &mut impl Admission,
) -> Result<Outcome> {
    let Some(dedupe) = dedupe(task) else {
        return Ok(Outcome::Run);
    };
    let id = task_id(task)?;
    let replay = request.dedupe_key(dedupe["replayKey"].as_str());
    if has_replay(db, id, &replay, now)? {
        return Ok(Outcome::Duplicate);
    }
    let coalesce = request.dedupe_key(dedupe["coalesceKey"].as_str());
    if coalesce.is_empty() {
        return Ok(Outcome::Run);
    }
    let owner: Option<String> = db
        .query_row(
            "SELECT delivery_id FROM scheduled_task_webhook_pending WHERE task_id=?1 AND coalesce_key=?2 AND state='pending' AND due_at>?3",
            params![id, coalesce, now],
            |row| row.get(0),
        )
        .optional()?;
    let logged = if owner.is_some() {
        admission.reserve_coalesced_log()
    } else {
        admission.check_capacity(db)?;
        admission.reserve()?;
        true
    };
    claim_replay(db, id, &replay, owner.as_deref().unwrap_or(delivery), now)?;
    let request_json = serde_json::to_string(request)?;
    if let Some(owner) = owner {
        db.execute(
            "UPDATE scheduled_task_webhook_pending SET request_json=?1,delivery_ids_json=json_insert(delivery_ids_json,'$[#]',?2) WHERE delivery_id=?3",
            params![request_json, delivery, owner],
        )?;
        admission.replace_request(db, &owner)?;
        return Ok(Outcome::Coalesced { owner, logged });
    }
    // A window whose first deadline arrived cannot absorb another request, even
    // when no worker has claimed it yet. This also frees the partial unique key.
    db.execute(
        "UPDATE scheduled_task_webhook_pending SET state='dispatching' WHERE task_id=?1 AND coalesce_key=?2 AND state='pending' AND due_at<=?3",
        params![id, coalesce, now],
    )?;
    let window = dedupe["coalesceWindowSeconds"].as_i64().unwrap_or(120);
    let due = window
        .checked_mul(1000)
        .and_then(|delay| now.checked_add(delay))
        .ok_or_else(|| Error::Persistence("Webhook window deadline exceeds SQLite range".into()))?;
    let task_json = serde_json::to_string(task)?;
    db.execute(
        "INSERT INTO scheduled_task_webhook_pending(delivery_id,task_id,coalesce_key,due_at,task_json,request_json,delivery_ids_json) VALUES(?1,?2,?3,?4,?5,?6,json_array(?1))",
        params![delivery, id, coalesce, due, task_json, request_json],
    )?;
    Ok(Outcome::Pending)
}

pub(crate) fn accept_immediate(
    db: &Connection,
    task: &Value,
    request: &Request,
    delivery: &str,
    now: i64,
    mut reserve: impl FnMut() -> Result<()>,
) -> Result<Outcome> {
    let id = task_id(task)?;
    let replay = request.dedupe_key(dedupe(task).and_then(|dedupe| dedupe["replayKey"].as_str()));
    if has_replay(db, id, &replay, now)? {
        return Ok(Outcome::Duplicate);
    }
    reserve()?;
    claim_replay(db, id, &replay, delivery, now)?;
    Ok(Outcome::Run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        crate::task_shared::initialize(&db).unwrap();
        db
    }
    fn task(id: &str) -> Value {
        json!({"id":id,"title":"Synthetic hook","prompt":"Inspect {{body.action}}","schedule":{"type":"webhook","dedupe":{"replayKey":"{{body.id}}","coalesceKey":"{{body.group}}"}}})
    }
    fn request(id: &str, group: &str) -> Request {
        Request {
            method: "POST".into(),
            path: "/api/hooks/synthetic".into(),
            query: "safe=fixture".into(),
            headers: [
                ("z-first".into(), "one".into()),
                ("a-next".into(), "two".into()),
            ]
            .into(),
            body_text: json!({"id":id,"group":group,"action":id}).to_string(),
        }
    }
    #[derive(Default)]
    struct Probe {
        calls: Vec<String>,
        failure: Option<&'static str>,
        unlogged: bool,
    }
    impl Admission for Probe {
        fn check_capacity(&mut self, _: &Connection) -> Result<()> {
            self.calls.push("capacity".into());
            if self.failure == Some("capacity") {
                Err(Error::QueueFull)
            } else {
                Ok(())
            }
        }
        fn reserve(&mut self) -> Result<()> {
            self.calls.push("reserve".into());
            match self.failure {
                Some("first") => Err(Error::FirstRejected),
                Some("rate") => Err(Error::Rejected),
                _ => Ok(()),
            }
        }
        fn reserve_coalesced_log(&mut self) -> bool {
            self.calls.push("coalesced-log".into());
            !self.unlogged
        }
        fn replace_request(&mut self, _: &Connection, owner: &str) -> Result<()> {
            self.calls.push(format!("replace:{owner}"));
            if self.failure == Some("replace") {
                Err(Error::Persistence(
                    "synthetic receipt update failure".into(),
                ))
            } else {
                Ok(())
            }
        }
    }
    fn admit(
        db: &mut Connection,
        task: &Value,
        request: &Request,
        delivery: &str,
        now: i64,
        probe: &mut Probe,
    ) -> Result<Outcome> {
        let tx = db.transaction()?;
        let outcome = accept(&tx, task, request, delivery, now, probe)?;
        tx.commit()?;
        Ok(outcome)
    }
    fn text(db: &Connection, sql: &str) -> String {
        db.query_row(sql, [], |row| row.get(0)).unwrap()
    }
    fn count(db: &Connection, table: &str) -> i64 {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn coalescing_keeps_first_deadline_snapshot_and_owner_but_latest_raw_request() {
        let mut db = db();
        let original = task("task");
        let first = request("one", "batch");
        let second = request("two", "batch");
        let mut probe = Probe::default();
        assert_eq!(
            admit(
                &mut db,
                &original,
                &first,
                "delivery:owner",
                1000,
                &mut probe
            )
            .unwrap(),
            Outcome::Pending
        );
        assert_eq!(probe.calls, ["capacity", "reserve"]);
        let mut edited = original.clone();
        edited["prompt"] = json!("A later template");
        let mut probe = Probe {
            unlogged: true,
            ..Probe::default()
        };
        assert_eq!(
            admit(
                &mut db,
                &edited,
                &second,
                "delivery:member",
                2000,
                &mut probe
            )
            .unwrap(),
            Outcome::Coalesced {
                owner: "delivery:owner".into(),
                logged: false
            }
        );
        assert_eq!(probe.calls, ["coalesced-log", "replace:delivery:owner"]);
        let (due,task_json,request_json,members):(i64,String,String,String)=db.query_row("SELECT due_at,task_json,request_json,delivery_ids_json FROM scheduled_task_webhook_pending",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(due, 121000);
        assert_eq!(serde_json::from_str::<Value>(&task_json).unwrap(), original);
        assert_eq!(request_json, serde_json::to_string(&second).unwrap());
        assert!(request_json.find("z-first").unwrap() < request_json.find("a-next").unwrap());
        assert_eq!(
            serde_json::from_str::<Value>(&members).unwrap(),
            json!(["delivery:owner", "delivery:member"])
        );
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 2);
        assert_eq!(db.query_row("SELECT count(*) FROM scheduled_task_webhook_replay_keys WHERE delivery_id='delivery:owner'",[],|row|row.get::<_,i64>(0)).unwrap(),2);
        let mut probe = Probe::default();
        assert_eq!(
            admit(
                &mut db,
                &original,
                &first,
                "delivery:duplicate",
                3000,
                &mut probe
            )
            .unwrap(),
            Outcome::Duplicate
        );
        assert!(probe.calls.is_empty());
        assert_eq!(count(&db, "scheduled_task_webhook_pending"), 1);
        release_replay(&db, "delivery:owner").unwrap();
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 0);
        assert_eq!(count(&db, "scheduled_task_webhook_pending"), 1);
    }

    #[test]
    fn deadline_boundary_and_separate_tasks_groups_and_running_windows_do_not_coalesce() {
        let mut db = db();
        let mut probe = Probe::default();
        let task = task("a");
        admit(
            &mut db,
            &task,
            &request("one", "same"),
            "old",
            1000,
            &mut probe,
        )
        .unwrap();
        assert_eq!(
            admit(
                &mut db,
                &task,
                &request("two", "same"),
                "new",
                121000,
                &mut probe
            )
            .unwrap(),
            Outcome::Pending
        );
        assert_eq!(
            text(
                &db,
                "SELECT state FROM scheduled_task_webhook_pending WHERE delivery_id='old'"
            ),
            "dispatching"
        );
        assert_eq!(
            text(
                &db,
                "SELECT request_json FROM scheduled_task_webhook_pending WHERE delivery_id='old'"
            ),
            serde_json::to_string(&request("one", "same")).unwrap()
        );
        let mut other = task.clone();
        other["id"] = json!("b");
        assert_eq!(
            admit(
                &mut db,
                &other,
                &request("two", "same"),
                "other-task",
                121001,
                &mut probe
            )
            .unwrap(),
            Outcome::Pending
        );
        assert_eq!(
            admit(
                &mut db,
                &task,
                &request("three", "different"),
                "other-group",
                121002,
                &mut probe
            )
            .unwrap(),
            Outcome::Pending
        );
        db.execute(
            "UPDATE scheduled_task_webhook_pending SET state='running' WHERE delivery_id='new'",
            [],
        )
        .unwrap();
        assert_eq!(
            admit(
                &mut db,
                &task,
                &request("four", "same"),
                "after-running",
                121003,
                &mut probe
            )
            .unwrap(),
            Outcome::Pending
        );
        assert_eq!(count(&db, "scheduled_task_webhook_pending"), 5);
    }

    #[test]
    fn replay_expiry_is_exact_and_empty_or_missing_keys_claim_nothing() {
        let mut db = db();
        let task = task("a");
        let request = request("one", "group");
        let mut reservations = 0;
        let mut reserve = || {
            reservations += 1;
            Ok(())
        };
        assert_eq!(
            accept_immediate(&db, &task, &request, "first", 500, &mut reserve).unwrap(),
            Outcome::Run
        );
        assert_eq!(
            accept_immediate(
                &db,
                &task,
                &request,
                "duplicate",
                500 + REPLAY_TTL_MS - 1,
                &mut reserve
            )
            .unwrap(),
            Outcome::Duplicate
        );
        assert_eq!(
            accept_immediate(
                &db,
                &task,
                &request,
                "expired",
                500 + REPLAY_TTL_MS,
                &mut reserve
            )
            .unwrap(),
            Outcome::Run
        );
        assert_eq!(reservations, 2);
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 1);
        assert_eq!(
            text(
                &db,
                "SELECT delivery_id FROM scheduled_task_webhook_replay_keys"
            ),
            "expired"
        );
        for key in ["", "{{body.absent}}"] {
            let mut no_key = task.clone();
            no_key["schedule"]["dedupe"] = json!({"replayKey":key,"coalesceKey":key});
            let mut probe = Probe::default();
            assert_eq!(
                admit(&mut db, &no_key, &request, "unkeyed", 600, &mut probe).unwrap(),
                Outcome::Run
            );
            assert!(probe.calls.is_empty());
            accept_immediate(&db, &no_key, &request, "immediate-unkeyed", 600, || Ok(())).unwrap();
            assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 1);
        }
        let mut no_dedupe = task.clone();
        no_dedupe["schedule"]
            .as_object_mut()
            .unwrap()
            .remove("dedupe");
        let mut probe = Probe::default();
        assert_eq!(
            admit(&mut db, &no_dedupe, &request, "no-dedupe", 700, &mut probe).unwrap(),
            Outcome::Run
        );
        assert!(probe.calls.is_empty());
        assert_eq!(count(&db, "scheduled_task_webhook_pending"), 0);
    }

    #[test]
    fn admission_failures_precede_claims_and_receipt_failure_rolls_back_coalescence() {
        let mut db = db();
        let task = task("task");
        for (failure, error, calls) in [
            ("capacity", Error::QueueFull, vec!["capacity"]),
            ("first", Error::FirstRejected, vec!["capacity", "reserve"]),
            ("rate", Error::Rejected, vec!["capacity", "reserve"]),
        ] {
            let mut probe = Probe {
                failure: Some(failure),
                ..Probe::default()
            };
            assert_eq!(
                admit(
                    &mut db,
                    &task,
                    &request("one", "group"),
                    "refused",
                    0,
                    &mut probe
                )
                .unwrap_err(),
                error
            );
            assert_eq!(probe.calls, calls);
            assert_eq!(count(&db, "scheduled_task_webhook_pending"), 0);
            assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 0);
        }
        admit(
            &mut db,
            &task,
            &request("one", "group"),
            "owner",
            1000,
            &mut Probe::default(),
        )
        .unwrap();
        let old_request = text(
            &db,
            "SELECT request_json FROM scheduled_task_webhook_pending",
        );
        let mut probe = Probe {
            failure: Some("replace"),
            ..Probe::default()
        };
        assert!(matches!(
            admit(
                &mut db,
                &task,
                &request("two", "group"),
                "member",
                1001,
                &mut probe
            ),
            Err(Error::Persistence(_))
        ));
        assert_eq!(
            text(
                &db,
                "SELECT request_json FROM scheduled_task_webhook_pending"
            ),
            old_request
        );
        assert_eq!(
            text(
                &db,
                "SELECT delivery_ids_json FROM scheduled_task_webhook_pending"
            ),
            r#"["owner"]"#
        );
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 1);
        assert_eq!(
            text(
                &db,
                "SELECT delivery_id FROM scheduled_task_webhook_replay_keys"
            ),
            "owner"
        );
        let error = accept_immediate(
            &db,
            &task,
            &request("fresh", "fresh"),
            "rejected-immediate",
            1002,
            || Err(Error::QueueFull),
        )
        .unwrap_err();
        assert_eq!(error, Error::QueueFull);
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 1);
    }

    #[test]
    fn persistence_failure_rolls_back_expired_window_transition_and_claim() {
        let mut db = db();
        let task = task("task");
        let mut probe = Probe::default();
        admit(
            &mut db,
            &task,
            &request("first", "batch"),
            "first",
            1000,
            &mut probe,
        )
        .unwrap();
        db.execute_batch("CREATE TRIGGER refuse_pending BEFORE INSERT ON scheduled_task_webhook_pending BEGIN SELECT RAISE(ABORT,'synthetic pending write failure'); END;").unwrap();
        assert!(matches!(
            admit(
                &mut db,
                &task,
                &request("next", "batch"),
                "next",
                121000,
                &mut probe
            ),
            Err(Error::Persistence(_))
        ));
        assert_eq!(
            text(&db, "SELECT state FROM scheduled_task_webhook_pending"),
            "pending"
        );
        assert_eq!(count(&db, "scheduled_task_webhook_replay_keys"), 1);
        assert_eq!(
            text(
                &db,
                "SELECT delivery_id FROM scheduled_task_webhook_replay_keys"
            ),
            "first"
        );
    }
}
