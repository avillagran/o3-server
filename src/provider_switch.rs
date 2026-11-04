//! Provider-thread reuse, portable deltas and exact native release at selection boundaries.
//! Adapted from nightly4179 Orchestrator/ProviderSwitchService (MIT).
use crate::{
    context_transfer::{list, own_items},
    events::{append, get, put},
    handoff_history as history,
    store::{iso, now_ms, random_id, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS provider_selection_plans(thread_id TEXT PRIMARY KEY,payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS provider_switches(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,payload TEXT NOT NULL,released_at TEXT,last_error TEXT); CREATE INDEX IF NOT EXISTS provider_switches_pending ON provider_switches(thread_id) WHERE released_at IS NULL;").map_err(|e|e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS provider_native_bindings(thread_id TEXT NOT NULL,session_id TEXT NOT NULL,provider_thread_id TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(thread_id,session_id,provider_thread_id));").map_err(|e|e.to_string())?;
    db.execute("UPDATE records AS p SET body=json_set(body,'$.ownerNodeId',NULL) WHERE kind='providerThreads' AND json_extract(body,'$.driver')='claudeAgent' AND json_extract(body,'$.appThreadId')=thread_id AND EXISTS(SELECT 1 FROM records n WHERE n.kind='nodes' AND n.thread_id=p.thread_id AND n.id=json_extract(p.body,'$.ownerNodeId') AND json_extract(n.body,'$.kind')='root_turn')",[]).map_err(|e|e.to_string())?;
    Ok(())
}
fn identity(s: &State, id: &str) -> Result<(String, PathBuf)> {
    let settings = crate::settings::read(s)?;
    let driver = crate::provider_input::driver(s, &json!(id))?;
    let configured = crate::provider_policy::configured_id(&settings, id);
    let mut entry = settings["providerInstances"][configured].clone();
    if !matches!(id, "codex" | "claudeAgent" | "claude" | "claude-code")
        && !entry["driver"].is_string()
    {
        return Ok((format!("unavailable:{id}"), PathBuf::new()));
    }
    entry["driver"] = json!(driver);
    match crate::provider_home::resolve(&entry)? {
        Some(home) => Ok((home.driver, home.shared)),
        None => Ok((driver, PathBuf::from(format!("instance:{id}")))),
    }
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub transition: crate::selection_transition::Transition,
    pub release_sessions: Vec<String>,
}
/// Native continuation can outlive a stopped session record. Prefer the newest live
/// record, retaining the newest negotiated capabilities when only native history remains.
pub(crate) fn plan(s: &State, db: &Connection, thread: &Value, target: &Value) -> Result<Plan> {
    use crate::selection_transition::{decide, model_selections_equal, Transition};
    let provider = s.provider.lock().unwrap().clone();
    let caps = |id: &str| {
        provider
            .as_ref()
            .and_then(|p| p.capabilities(id))
            .or_else(|| {
                if provider.is_none() {
                    crate::provider_input::driver(s, &json!(id))
                        .ok()
                        .and_then(|driver| {
                            crate::selection_transition::native_capabilities(&driver)
                        })
                } else {
                    None
                }
            })
    };
    let from = thread["modelSelection"]["instanceId"]
        .as_str()
        .unwrap_or("");
    let to = target["instanceId"].as_str().unwrap_or("");
    let current_caps = caps(from);
    let target_caps = caps(to);
    let mut sessions: Vec<_> = list(db, "providerSessions", thread["id"].as_str().unwrap_or(""))?
        .into_iter()
        .filter(|v| v["providerInstanceId"] == from)
        .collect();
    sessions.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
    let live = |session: &&Value| !matches!(session["status"].as_str(), Some("stopped" | "error"));
    let session = sessions.iter().find(live);
    let negotiated = session
        .or_else(|| sessions.first())
        .map(|s| &s["capabilities"])
        .or(current_caps.as_ref());
    let native = current(db, thread)?
        .filter(|pt| pt["providerInstanceId"] == from && pt["nativeThreadRef"].is_object());
    let current_identity = identity(s, from).ok();
    let target_identity = identity(s, to)?;
    let selection = if from == to
        && !model_selections_equal(&thread["modelSelection"], target)
        && current_caps.is_some()
        && (session.is_some() || native.is_some())
    {
        match &provider {
            Some(provider) => provider.plan_selection_transition(
                to,
                &thread["modelSelection"],
                target,
                negotiated.unwrap_or(&Value::Null),
            )?,
            None => None,
        }
    } else {
        None
    };
    let workspace = thread["worktreePath"]
        .as_str()
        .or_else(|| session.and_then(|s| s["cwd"].as_str()))
        .unwrap_or("<unresolved-workspace>");
    let current_workspace = session
        .and_then(|s| s["cwd"].as_str())
        .or_else(|| thread["worktreePath"].as_str())
        .unwrap_or("<unresolved-workspace>");
    let state = |model: &Value, identity: &(String, PathBuf), workspace: &str| json!({"driver":identity.0,"continuationIdentity":{"driverKind":identity.0,"continuationKey":identity.1},"modelSelection":model,"runtimeMode":thread["runtimeMode"],"interactionMode":thread["interactionMode"],"workspace":workspace});
    let current = current_identity
        .as_ref()
        .filter(|_| current_caps.is_some() && (session.is_some() || native.is_some()))
        .map(|id| state(&thread["modelSelection"], id, current_workspace));
    let mut target_state = state(target, &target_identity, workspace);
    let settings = crate::settings::read(s)?;
    target_state["available"] = json!(
        target_caps.is_some()
            && crate::provider_policy::project_enabled(&settings, thread["projectId"].as_str(), to)
    );
    let transition =
        decide(&json!({"current":current,"target":target_state,"selectionTransition":selection}));
    if let Transition::Reject { reason } = &transition {
        return Err(reason.clone());
    }
    let release_sessions = sessions
        .iter()
        .filter(live)
        .filter(|s| match transition {
            Transition::RestartAndResume => session.is_some_and(|current| current["id"] == s["id"]),
            _ => false,
        })
        .filter_map(|s| s["id"].as_str().map(str::to_owned))
        .collect();
    // Handoffs release other instances, including their pooled bindings, rather than
    // the target's existing reusable session. The outgoing exact native job is staged below.
    let release_sessions = if transition == Transition::CreateWithHandoff {
        list(db, "providerSessions", thread["id"].as_str().unwrap_or(""))?
            .iter()
            .filter(live)
            .filter(|s| s["providerInstanceId"] != to)
            .filter_map(|s| s["id"].as_str().map(str::to_owned))
            .collect()
    } else {
        release_sessions
    };
    Ok(Plan {
        transition,
        release_sessions,
    })
}
fn current(db: &Connection, thread: &Value) -> Result<Option<Value>> {
    let Some(id) = thread["activeProviderThreadId"].as_str() else {
        return Ok(None);
    };
    let body: Option<String> = db
        .query_row(
            "SELECT body FROM records WHERE kind='providerThreads' AND id=?",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    body.map(|body| serde_json::from_str(&body).map_err(|e| e.to_string()))
        .transpose()
}
fn guard(db: &Connection, thread: &Value, target: &Value) -> Result<()> {
    if let Some(pt) =
        current(db, thread)?.filter(|pt| pt["providerInstanceId"] != target["instanceId"])
    {
        if crate::native_recovery::background_work(db, thread["id"].as_str().unwrap_or(""), &pt)? {
            return Err("The previous provider is still running background agents or commands; wait for them or stop before switching.".into());
        }
    }
    Ok(())
}
// Owner keys belong to the native binding, not the thread's latest preference generation.
// Keep this index private: its values are runtime routing metadata, not credentials.
pub(crate) fn bound(db: &Connection, thread: &Value, run: &Value) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let pt = get(
        db,
        "providerThreads",
        run["providerThreadId"]
            .as_str()
            .ok_or("provider thread missing")?,
    )?;
    let Some(session) = pt["providerSessionId"].as_str() else {
        return Ok(());
    };
    let Some(native) = pt["nativeThreadRef"]["nativeId"].as_str() else {
        return Ok(());
    };
    let payload = json!({"bindingId":random_id()?,"providerInstanceId":pt["providerInstanceId"],"providerSessionId":session,"providerThreadId":pt["id"],"nativeId":native,"key":format!("{}:{}",tid,thread["providerGeneration"].as_str().unwrap_or("")),"driver":pt["driver"]});
    db.execute("INSERT INTO provider_native_bindings VALUES(?1,?2,?3,?4) ON CONFLICT(thread_id,session_id,provider_thread_id) DO UPDATE SET payload=excluded.payload",params![tid,session,pt["id"].as_str(),payload.to_string()]).map_err(|e|e.to_string())?;
    Ok(())
}
fn stage_payload(db: &Connection, thread: &Value, mut payload: Value) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let pt = payload["providerThreadId"].as_str().map(str::to_owned);
    payload["runIds"] = json!(list(db, "runs", tid)?
        .into_iter()
        .filter(|r| pt.is_some() && r["providerThreadId"] == payload["providerThreadId"])
        .map(|r| r["id"].clone())
        .collect::<Vec<_>>());
    let id = format!(
        "provider-switch:{}:{}{}",
        pt.as_deref()
            .or(payload["providerSessionId"].as_str())
            .unwrap_or(""),
        payload["key"].as_str().unwrap_or(""),
        payload["bindingId"]
            .as_str()
            .map(|id| format!(":{id}"))
            .unwrap_or_default()
    );
    db.execute(
        "INSERT OR IGNORE INTO provider_switches(id,thread_id,payload) VALUES(?,?,?)",
        params![id, tid, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn stage(db: &Connection, thread: &Value, pt: &Value) -> Result<()> {
    let saved:Option<String>=db.query_row("SELECT payload FROM provider_native_bindings WHERE thread_id=?1 AND provider_thread_id=?2 AND json_extract(payload,'$.providerInstanceId')=?3 AND json_extract(payload,'$.nativeId') IS ?4 ORDER BY (session_id IS ?5) DESC,rowid DESC LIMIT 1",params![thread["id"].as_str(),pt["id"].as_str(),pt["providerInstanceId"].as_str(),pt["nativeThreadRef"]["nativeId"].as_str(),pt["providerSessionId"].as_str()],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let payload = match saved {
        Some(saved) => serde_json::from_str(&saved).map_err(|e| e.to_string())?,
        None => {
            json!({"providerInstanceId":pt["providerInstanceId"],"key":format!("{}:{}",thread["id"].as_str().unwrap_or(""),thread["providerGeneration"].as_str().unwrap_or("")),"nativeId":pt["nativeThreadRef"]["nativeId"],"providerSessionId":pt["providerSessionId"],"providerThreadId":pt["id"],"driver":pt["driver"]})
        }
    };
    stage_payload(db, thread, payload)
}
fn stage_sessions(db: &Connection, thread: &Value, sessions: &[String]) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let threads = list(db, "providerThreads", tid)?;
    for session in sessions {
        let owned = threads
            .iter()
            .filter(|pt| pt["providerSessionId"] == *session)
            .collect::<Vec<_>>();
        for pt in &owned {
            if crate::native_recovery::background_work(db, tid, pt)? {
                return Err("The previous provider is still running background agents or commands; wait for them or stop before switching.".into());
            }
        }
        let bindings = {
            let mut q=db.prepare("SELECT payload FROM provider_native_bindings WHERE thread_id=? AND session_id=? AND json_extract(payload,'$.releasedAt') IS NULL").map_err(|e|e.to_string())?;
            let rows = q
                .query_map(params![tid, session], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        if !bindings.is_empty() {
            for binding in bindings {
                stage_payload(
                    db,
                    thread,
                    serde_json::from_str(&binding).map_err(|e| e.to_string())?,
                )?;
            }
        } else if !owned.is_empty() {
            for pt in owned {
                stage(db, thread, pt)?;
            }
        } else {
            let session = get(db, "providerSessions", session)?;
            stage_payload(
                db,
                thread,
                json!({"providerInstanceId":session["providerInstanceId"],"providerSessionId":session["id"],"providerThreadId":null,"nativeId":null,"key":format!("{}:{}",tid,thread["providerGeneration"].as_str().unwrap_or("")),"driver":session["driver"]}),
            )?;
        }
    }
    Ok(())
}
/// Metadata selection preserves native ownership until a run chooses a compatible row or a handoff.
pub(crate) fn selection(
    s: &State,
    db: &Connection,
    thread: &mut Value,
    target: &Value,
    command_id: &Value,
) -> Result<crate::selection_transition::Transition> {
    let plan = plan(s, db, thread, target)?;
    guard(db, thread, target)?;
    stage_sessions(db, thread, &plan.release_sessions)?;
    db.execute("INSERT INTO provider_selection_plans VALUES(?,?) ON CONFLICT(thread_id) DO UPDATE SET payload=excluded.payload",params![thread["id"].as_str(),json!({"target":target,"transition":plan.transition,"providerThreadId":thread["activeProviderThreadId"],"sourceInstanceId":thread["modelSelection"]["instanceId"],"releaseSessionIds":plan.release_sessions}).to_string()]).map_err(|e|e.to_string())?;
    if let Some(pt) = current(db, thread)? {
        if pt["providerInstanceId"] != target["instanceId"]
            || (pt["providerInstanceId"] == thread["modelSelection"]["instanceId"]
                && matches!(
                    plan.transition,
                    crate::selection_transition::Transition::RestartAndResume
                        | crate::selection_transition::Transition::CreateWithHandoff
                ))
        {
            identity(s, pt["providerInstanceId"].as_str().unwrap_or(""))?;
            identity(s, target["instanceId"].as_str().unwrap_or(""))?;
            stage(db, thread, &pt)?;
        }
    } else if thread["providerInstanceId"] != target["instanceId"] {
        // Imported pre-row bindings keep their established portable migration path.
        let tid = thread["id"].as_str().ok_or("thread id missing")?.to_owned();
        let ordinal=db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),0) FROM records WHERE kind='runs' AND thread_id=?",[&tid],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?;
        crate::thread_commands::capture_handoff(db, thread, &tid, ordinal)?;
        thread["providerGeneration"] = command_id.clone();
        for key in [
            "activeProviderThreadId",
            "codexNativeThreadId",
            "nativeForkSource",
        ] {
            thread[key] = Value::Null;
        }
    }
    Ok(plan.transition)
}

fn source_run(r: &Value) -> bool {
    matches!(
        r["status"].as_str(),
        Some("completed" | "failed" | "interrupted")
    ) && r["contextDiscarded"] != true
}
pub(crate) fn admit(
    s: &State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    at: &str,
) -> Result<()> {
    // A queued message keeps the model it recorded, even if idle metadata changed later.
    if run["modelSelection"]["instanceId"].is_string()
        && (thread["providerInstanceId"] != run["providerInstanceId"]
            || !crate::selection_transition::model_selections_equal(
                &thread["modelSelection"],
                &run["modelSelection"],
            ))
    {
        let switched = thread["providerInstanceId"] != run["providerInstanceId"];
        selection(s, db, thread, &run["modelSelection"], &run["id"])?;
        thread["providerInstanceId"] = run["providerInstanceId"].clone();
        thread["modelSelection"] = run["modelSelection"].clone();
        let tid = thread["id"].as_str().ok_or("thread id missing")?;
        put(db, "thread", tid, tid, thread)?;
        append(
            db,
            tid,
            if switched {
                "thread.provider-switched"
            } else {
                "thread.model-selection-updated"
            },
            thread.clone(),
            thread,
        )?;
    }
    let active = current(db, thread)?;
    let pending: Option<String> = db
        .query_row(
            "SELECT payload FROM provider_selection_plans WHERE thread_id=?",
            [thread["id"].as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    db.execute(
        "DELETE FROM provider_selection_plans WHERE thread_id=?",
        [thread["id"].as_str()],
    )
    .map_err(|e| e.to_string())?;
    let pending = pending
        .map(|p| serde_json::from_str::<Value>(&p).map_err(|e| e.to_string()))
        .transpose()?;
    let transition = pending
        .filter(|p| {
            p["providerThreadId"] == thread["activeProviderThreadId"]
                && active
                    .as_ref()
                    .is_some_and(|pt| pt["providerInstanceId"] == p["sourceInstanceId"])
                && crate::selection_transition::model_selections_equal(
                    &p["target"],
                    &run["modelSelection"],
                )
        })
        .map(|p| {
            serde_json::from_value::<crate::selection_transition::Transition>(
                p["transition"].clone(),
            )
            .map_err(|e| e.to_string())
        })
        .transpose()?;
    let Some(active) = active else {
        return Ok(());
    };
    let generation = run["id"].clone();
    move_binding(
        s,
        db,
        thread,
        run,
        BindingMove {
            active: &active,
            transition,
            source: None,
            generation,
            actor: None,
            at,
        },
    )
}
struct BindingMove<'a> {
    active: &'a Value,
    transition: Option<crate::selection_transition::Transition>,
    source: Option<&'a Value>,
    generation: Value,
    actor: Option<&'a str>,
    at: &'a str,
}
pub(crate) fn active_plan(
    s: &State,
    db: &Connection,
    thread: &Value,
    source: &Value,
    target: &Value,
) -> Result<Plan> {
    let mut owner = thread.clone();
    owner["providerInstanceId"] = source["providerInstanceId"].clone();
    owner["modelSelection"] = source["modelSelection"].clone();
    owner["activeProviderThreadId"] = source["providerThreadId"].clone();
    plan(s, db, &owner, target)
}
pub(crate) struct RestartBinding<'a> {
    pub source: &'a Value,
    pub transition: crate::selection_transition::Transition,
    pub generation: &'a str,
    pub actor: &'a str,
    pub at: &'a str,
}
pub(crate) fn restart_binding(
    s: &State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    context: RestartBinding<'_>,
) -> Result<()> {
    use crate::selection_transition::Transition;
    if !matches!(
        context.transition,
        Transition::RestartAndResume | Transition::CreateWithHandoff
    ) {
        return Ok(());
    }
    if context.transition == Transition::CreateWithHandoff {
        let provider = s
            .provider
            .lock()
            .unwrap()
            .clone()
            .ok_or("Target provider unavailable")?;
        let caps = provider
            .capabilities(run["providerInstanceId"].as_str().unwrap_or(""))
            .ok_or("Target provider unavailable")?;
        for (field, reason) in [
            (
                "canConsumeHandoffSummaries",
                "cannot consume handoff summaries",
            ),
            (
                "acceptsSyntheticUserContext",
                "cannot receive synthetic user context",
            ),
            (
                "supportsFullThreadHandoff",
                "does not support full-thread handoff",
            ),
        ] {
            if caps["context"][field] != true {
                return Err(format!("Target provider {reason}"));
            }
        }
    }
    let active = get(
        db,
        "providerThreads",
        context.source["providerThreadId"]
            .as_str()
            .ok_or("Source provider thread missing")?,
    )?;
    move_binding(
        s,
        db,
        thread,
        run,
        BindingMove {
            active: &active,
            transition: Some(context.transition),
            source: Some(context.source),
            generation: json!(context.generation),
            actor: Some(context.actor),
            at: context.at,
        },
    )
}
fn move_binding(
    s: &State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    context: BindingMove<'_>,
) -> Result<()> {
    let active = context.active;
    let transition = context.transition;
    let at = context.at;
    let target = run["providerInstanceId"]
        .as_str()
        .ok_or("run provider missing")?
        .to_owned();
    if active["providerInstanceId"] == target
        && !matches!(
            transition,
            Some(
                crate::selection_transition::Transition::RestartAndResume
                    | crate::selection_transition::Transition::CreateWithHandoff
            )
        )
    {
        return Ok(());
    }
    let tid = thread["id"].as_str().ok_or("thread id missing")?.to_owned();
    if context.source.is_none() {
        guard(db, thread, &json!({"instanceId":target}))?;
    }
    let source = active["providerInstanceId"]
        .as_str()
        .ok_or("source provider missing")?;
    let source_identity = identity(s, source)?;
    let target_identity = identity(s, &target)?;
    let compatible = transition != Some(crate::selection_transition::Transition::CreateWithHandoff)
        && active["nativeThreadRef"].is_object()
        && active["driver"] == target_identity.0
        && source_identity == target_identity;
    stage(db, thread, active)?;
    let all = list(db, "providerThreads", &tid)?;
    let mut candidates = all
        .iter()
        .filter(|pt| {
            pt["providerInstanceId"] == target
                && pt["id"] != active["id"]
                && pt["appThreadId"] == tid
                && pt["ownerNodeId"].is_null()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b["lastRunOrdinal"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["lastRunOrdinal"].as_i64().unwrap_or(0))
            .then_with(|| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()))
    });
    let existing = if compatible {
        Some(active.clone())
    } else {
        candidates.first().map(|pt| (*pt).clone())
    };
    let mut pt = if let Some(pt) = &existing {
        pt.clone()
    } else {
        let id = format!("provider-thread-{}", random_id()?);
        json!({"id":id,"appThreadId":tid,"driver":crate::provider_input::driver(s,&json!(target))?,"providerInstanceId":target,"providerSessionId":null,"ownerNodeId":null,"nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"not_loaded","firstRunOrdinal":run["ordinal"],"lastRunOrdinal":run["ordinal"],"handoffIds":[],"forkedFrom":null,"contextUsage":null,"goal":null,"nativeMetadata":null,"pendingBackgroundTasks":[],"createdAt":at,"updatedAt":at})
    };
    pt["status"] = json!(if context.source.is_some() {
        "not_loaded"
    } else {
        "active"
    });
    if !pt["firstRunOrdinal"].is_number() {
        pt["firstRunOrdinal"] = run["ordinal"].clone();
    }
    pt["providerInstanceId"] = json!(target);
    pt["providerSessionId"] = Value::Null;
    pt["lastRunOrdinal"] = run["ordinal"].clone();
    pt["updatedAt"] = json!(at);
    let runs = list(db, "runs", &tid)?;
    let turns = list(db, "providerTurns", &tid)?;
    let merge = list(db, "contextTransfers", &tid)?
        .iter()
        .any(|t| t["type"] == "merge_back" && t["status"] == "pending");
    let last_delivered = runs
        .iter()
        .filter(|run| {
            context.source.is_none()
                && source_run(run)
                && run["providerThreadId"] == pt["id"]
                && (run["status"] == "completed"
                    || turns
                        .iter()
                        .any(|turn| turn["runAttemptId"] == run["activeAttemptId"]))
        })
        .max_by_key(|run| run["ordinal"].as_i64());
    let since = if merge {
        0
    } else {
        last_delivered
            .and_then(|run| run["ordinal"].as_i64())
            .unwrap_or(0)
    };
    let mut covered = runs
        .iter()
        .filter(|r| {
            !compatible
                && (source_run(r) || context.source.is_some_and(|source| source["id"] == r["id"]))
                && r["ordinal"].as_i64().is_some_and(|ordinal| {
                    ordinal > since
                        && if context.source.is_some() {
                            ordinal <= run["ordinal"].as_i64().unwrap_or(i64::MAX)
                        } else {
                            ordinal < run["ordinal"].as_i64().unwrap_or(i64::MAX)
                        }
                })
        })
        .collect::<Vec<_>>();
    covered.sort_by_key(|run| run["ordinal"].as_i64().unwrap_or(0));
    if !covered.is_empty() {
        let items = own_items(
            db,
            &tid,
            run["ordinal"].as_i64().unwrap_or(1) - if context.source.is_some() { 0 } else { 1 },
        )?
        .into_iter()
        .filter(|i| {
            covered.iter().any(|r| r["id"] == i["runId"])
                || (i["runId"].is_null()
                    && (existing.is_none() || merge || context.source.is_some()))
        })
        .collect::<Vec<_>>();
        let from = covered
            .iter()
            .filter_map(|r| r["ordinal"].as_i64())
            .min()
            .unwrap_or(1);
        let to = covered
            .iter()
            .filter_map(|r| r["ordinal"].as_i64())
            .max()
            .unwrap_or(from);
        let coverage = history::coverage(&tid, from, to, &items);
        let messages = items
            .iter()
            .filter_map(history::message)
            .collect::<Vec<_>>();
        let selected = history::select(&messages, &coverage, 0, history::cap());
        let id = random_id()?;
        let transfer_id = random_id()?;
        let strategy = if existing.is_none() || merge || context.source.is_some() {
            "full_thread_summary"
        } else {
            "delta_since_target_last_seen"
        };
        let mut from_ids = Vec::new();
        for id in covered
            .iter()
            .filter_map(|run| run["providerThreadId"].as_str())
        {
            if !from_ids.contains(&id) {
                from_ids.push(id);
            }
        }
        if context.source.is_some() {
            from_ids = vec![active["id"]
                .as_str()
                .ok_or("Source provider thread id missing")?];
        }
        let handoff = json!({"id":id,"transferId":transfer_id,"threadId":tid,"targetRunId":run["id"],"fromProviderThreadIds":from_ids,"toProviderThreadId":pt["id"],"coveredRunOrdinals":{"from":from,"to":to},"strategy":strategy,"status":"ready","summaryMessageId":null,"summaryText":history::render(selected["messages"].as_array().unwrap(),selected["context"].as_str().unwrap()),"history":{"messages":selected["messages"],"coverage":coverage,"omittedItems":selected["omittedItems"],"omittedItemIds":selected["omittedItemIds"]},"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at});
        let latest = covered
            .iter()
            .max_by_key(|r| r["ordinal"].as_i64())
            .unwrap();
        let base = if merge {
            Value::Null
        } else {
            last_delivered
                .map(|run| crate::context_transfer::canonical(db, &tid, run))
                .unwrap_or(Value::Null)
        };
        let actor = if let Some(actor) = context.actor {
            actor.to_owned()
        } else {
            get(db, "messages", run["userMessageId"].as_str().unwrap_or(""))?["createdBy"]
                .as_str()
                .unwrap_or("user")
                .to_owned()
        };
        // Admission consumes a transfer; the delivery ACK independently records native acceptance.
        let transfer = json!({"id":transfer_id,"type":"provider_handoff","sourceThreadId":tid,"targetThreadId":tid,"sourcePoint":crate::context_transfer::canonical(db,&tid,latest),"basePoint":base,"sourceProviderInstanceId":latest["providerInstanceId"],"targetProviderInstanceId":target,"targetRunId":run["id"],"status":"consumed","resolution":{"strategy":if strategy=="full_thread_summary"{"portable_context"}else{"delta_context"},"contextHandoffId":id},"createdBy":actor,"error":null,"createdAt":at,"updatedAt":at,"consumedAt":at});
        put(db, "contextHandoffs", &id, &tid, &handoff)?;
        append(db, &tid, "context-handoff.updated", handoff, thread)?;
        put(db, "contextTransfers", &transfer_id, &tid, &transfer)?;
        append(db, &tid, "context-transfer.created", transfer, thread)?;
        pt["handoffIds"]
            .as_array_mut()
            .ok_or("provider handoff ids missing")?
            .push(json!(id));
        run["contextHandoffId"] = json!(id);
    }
    let ptid = pt["id"]
        .as_str()
        .ok_or("target provider row missing")?
        .to_owned();
    thread["activeProviderThreadId"] = json!(ptid);
    thread["providerGeneration"] = context.generation;
    run["providerThreadId"] = json!(ptid);
    thread["providerInstanceId"] = json!(target);
    thread["modelSelection"] = run["modelSelection"].clone();
    thread["codexNativeThreadId"] = if pt["driver"] == "codex" {
        pt["nativeThreadRef"]["nativeId"].clone()
    } else {
        Value::Null
    };
    thread["providerHandoff"] = Value::Null;
    thread["providerHandoffAttachments"] = json!([]);
    put(db, "providerThreads", &ptid, &tid, &pt)?;
    append(db, &tid, "provider-thread.updated", pt, thread)?;
    put(db, "thread", &tid, &tid, thread)?;
    append(db, &tid, "thread.metadata-updated", thread.clone(), thread)?;
    Ok(())
}
fn gate(s: &State, tid: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut gates = s.provider_switch_gates.lock().unwrap();
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(tid).and_then(std::sync::Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    gates.insert(tid.to_owned(), Arc::downgrade(&gate));
    gate
}
pub(crate) async fn release_pending(
    s: &State,
    provider: &dyn crate::provider::Provider,
    tid: &str,
) -> Result<()> {
    let gate = gate(s, tid);
    let _guard = gate.lock().await;
    let jobs = {
        let db = s.db.lock().unwrap();
        let mut q=db.prepare("SELECT id,payload FROM provider_switches WHERE thread_id=? AND released_at IS NULL ORDER BY rowid").map_err(|e|e.to_string())?;
        let jobs = q
            .query_map([tid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        jobs
    };
    for (id, payload) in jobs {
        let p: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let work = async {
            for run in p["runIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                s.executions.wait_run(run).await;
            }
            provider
                .detach(
                    p["providerInstanceId"].as_str().unwrap_or(""),
                    p["key"].as_str().unwrap_or(""),
                    p["nativeId"].as_str(),
                    true,
                )
                .await
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), work)
            .await
            .unwrap_or_else(|_| Err("Provider switch release timed out".into()));
        if let Err(error) = result {
            s.db.lock()
                .unwrap()
                .execute(
                    "UPDATE provider_switches SET last_error=? WHERE id=?",
                    params![error, id],
                )
                .map_err(|e| e.to_string())?;
            return Err(error);
        }
        let mut db = s.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let at = iso(now_ms());
        if let Some(ptid) = p["providerThreadId"].as_str() {
            if let Ok(mut pt) = get(&tx, "providerThreads", ptid) {
                if pt["nativeThreadRef"]["nativeId"] == p["nativeId"] {
                    pt["status"] = json!("not_loaded");
                    pt["pendingBackgroundTasks"] = json!([]);
                    if pt["providerSessionId"] == p["providerSessionId"] {
                        pt["providerSessionId"] = Value::Null;
                    }
                    pt["updatedAt"] = json!(at);
                    put(&tx, "providerThreads", ptid, tid, &pt)?;
                    let thread = get(&tx, "thread", tid)?;
                    append(&tx, tid, "provider-thread.updated", pt, &thread)?;
                }
            }
        }
        tx.execute("UPDATE provider_native_bindings SET payload=json_set(payload,'$.releasedAt',?4) WHERE thread_id=?1 AND json_extract(payload,'$.key')=?2 AND json_extract(payload,'$.nativeId') IS ?3 AND json_extract(payload,'$.bindingId') IS ?5",params![tid,p["key"].as_str(),p["nativeId"].as_str(),at,p["bindingId"].as_str()]).map_err(|e|e.to_string())?;
        tx.execute(
            "UPDATE provider_switches SET released_at=?,last_error=NULL WHERE id=?",
            params![at, id],
        )
        .map_err(|e| e.to_string())?;
        if let Some(session) = p["providerSessionId"].as_str() {
            let removed=tx.execute("DELETE FROM records WHERE kind='providerSessions' AND thread_id=? AND id=? AND json_extract(body,'$.providerInstanceId')=?",params![tid,session,p["providerInstanceId"].as_str()]).map_err(|e|e.to_string())?;
            if removed > 0 {
                let thread = get(&tx, "thread", tid)?;
                let sequence = append(
                    &tx,
                    tid,
                    "provider-session.detached",
                    json!({"providerSessionId":session,"detachedAt":at,"reason":"Provider switched."}),
                    &thread,
                )?;
                tx.execute("UPDATE events SET body=json_set(body,'$.driver',?1,'$.providerInstanceId',?2) WHERE sequence=?3",params![p["driver"].as_str(),p["providerInstanceId"].as_str(),sequence]).map_err(|e|e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) fn start_release(s: Arc<State>, tid: String) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let provider = s.provider.lock().unwrap().clone();
            if let Some(provider) = provider {
                let _ = release_pending(&s, provider.as_ref(), &tid).await;
                s.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_claude_root_rows_upgrade_without_claiming_native_subagent_rows() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        for (id, kind, owner) in [
            ("root", "root_turn", "root"),
            ("child", "subagent_turn", "child"),
        ] {
            put(&db, "nodes", id, "t", &json!({"id":id,"kind":kind})).unwrap();
            put(
                &db,
                "providerThreads",
                id,
                "t",
                &json!({"id":id,"appThreadId":"t","driver":"claudeAgent","ownerNodeId":owner}),
            )
            .unwrap();
        }
        for _ in 0..2 {
            initialize(&db).unwrap();
            assert!(get(&db, "providerThreads", "root").unwrap()["ownerNodeId"].is_null());
            assert_eq!(
                get(&db, "providerThreads", "child").unwrap()["ownerNodeId"],
                "child"
            );
        }
        db.execute(
            "UPDATE records SET body='invalid json' WHERE kind='providerThreads' AND id='root'",
            [],
        )
        .unwrap();
        assert!(current(&db, &json!({"activeProviderThreadId":"root"})).is_err());
        assert!(current(&db, &json!({"activeProviderThreadId":"missing"}))
            .unwrap()
            .is_none());
    }
    #[test]
    fn continuation_identity_uses_resolved_homes_and_keeps_literal_inherited_tilde() {
        let dir = std::env::temp_dir().join(format!("provider-identity-{}", random_id().unwrap()));
        let s = State::open(&dir).unwrap();
        std::fs::write(s.dir.join("settings.json"),json!({"providerInstances":{
            "first":{"driver":"codex","config":{"homePath":"/shared/home","shadowHomePath":"/account-a"}},
            "second":{"driver":"codex","config":{"homePath":"/shared/./home","shadowHomePath":"/account-b"}},
            "foreign":{"driver":"codex","config":{"homePath":"/other/home"}},
            "claude-a":{"driver":"claudeAgent","config":{"homePath":"/claude/home"}},
            "claude-b":{"driver":"claudeAgent","environment":[{"name":"CLAUDE_CONFIG_DIR","value":"/claude/./home"}]},
            "literal":{"driver":"claudeAgent","environment":[{"name":"CLAUDE_CONFIG_DIR","value":"~/literal"}]},
            "expanded":{"driver":"claudeAgent","config":{"homePath":"~/literal"}},
            "unsupported-a":{"driver":"cursor"},"unsupported-b":{"driver":"cursor"}
        }}).to_string()).unwrap();
        assert_eq!(
            identity(&s, "first").unwrap(),
            identity(&s, "second").unwrap()
        );
        assert_ne!(
            identity(&s, "first").unwrap(),
            identity(&s, "foreign").unwrap()
        );
        assert_eq!(
            identity(&s, "claude-a").unwrap(),
            identity(&s, "claude-b").unwrap()
        );
        assert_ne!(
            identity(&s, "literal").unwrap(),
            identity(&s, "expanded").unwrap()
        );
        assert_ne!(
            identity(&s, "unsupported-a").unwrap(),
            identity(&s, "unsupported-b").unwrap()
        );
        assert_ne!(
            identity(&s, "missing").unwrap(),
            identity(&s, "claudeAgent").unwrap()
        );
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::{
        provider::{Provider, ProviderControl, ProviderEvent, RunInput},
        selection_transition::{Selection, Transition},
    };
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };
    struct Adapter {
        acp: AtomicBool,
        seen: Mutex<Vec<Value>>,
    }
    impl Provider for Adapter {
        fn plan_selection_transition(
            &self,
            _instance: &str,
            current: &Value,
            target: &Value,
            caps: &Value,
        ) -> Result<Option<Selection>> {
            self.seen.lock().unwrap().push(caps.clone());
            Ok(Some(if self.acp.load(Ordering::SeqCst) {
                crate::selection_transition::acp(
                    &json!({"current":current,"target":target,"sessionCapabilities":caps}),
                )
            } else {
                Selection::RestartSession
            }))
        }
        fn execute<'a>(
            &'a self,
            _input: RunInput,
            _output: tokio::sync::mpsc::Sender<ProviderEvent>,
            _control: tokio::sync::mpsc::Receiver<ProviderControl>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async { Err("planner tests never open a native session".into()) })
        }
    }
    #[test]
    fn live_session_selection_and_dead_negotiation_follow_the_pinned_service() {
        let dir = std::env::temp_dir().join(format!("selection-plan-{}", random_id().unwrap()));
        let s = State::open(&dir).unwrap();
        let adapter = Arc::new(Adapter {
            acp: AtomicBool::new(false),
            seen: Mutex::new(vec![]),
        });
        *s.provider.lock().unwrap() = Some(adapter.clone());
        let thread = json!({"id":"t","projectId":"p","modelSelection":{"instanceId":"codex","model":"one"},"providerInstanceId":"codex","activeProviderThreadId":"native-row","runtimeMode":"full-access","interactionMode":"default","worktreePath":"/repo"});
        let target = json!({"instanceId":"codex","model":"two"});
        let mut db = s.db.lock().unwrap();
        put(&db, "thread", "t", "t", &thread).unwrap();
        put(&db,"providerThreads","native-row","t",&json!({"id":"native-row","appThreadId":"t","providerInstanceId":"codex","driver":"codex","nativeThreadRef":{"nativeId":"native-id"}})).unwrap();
        for (id, status, updated, supports) in [
            ("live-older", "ready", "2026-01-01T00:00:00Z", false),
            ("live-newer", "ready", "2026-01-02T00:00:00Z", true),
            ("dead-newest", "error", "2026-01-03T00:00:00Z", false),
        ] {
            put(&db,"providerSessions",id,"t",&json!({"id":id,"providerInstanceId":"codex","status":status,"updatedAt":updated,"cwd":"/repo","capabilities":{"sessions":{"supportsModelSwitchInSession":supports}}})).unwrap();
        }
        let p = plan(&s, &db, &thread, &target).unwrap();
        assert_eq!(p.transition, Transition::RestartAndResume);
        assert_eq!(p.release_sessions, vec!["live-newer"]);
        assert_eq!(
            adapter.seen.lock().unwrap().last().unwrap()["sessions"]
                ["supportsModelSwitchInSession"],
            true
        );
        db.execute("UPDATE records SET body=json_set(body,'$.status','stopped') WHERE kind='providerSessions'",[]).unwrap();
        let p = plan(&s, &db, &thread, &target).unwrap();
        assert_eq!(p.transition, Transition::RestartAndResume);
        assert!(p.release_sessions.is_empty());
        assert_eq!(
            adapter.seen.lock().unwrap().last().unwrap()["sessions"]
                ["supportsModelSwitchInSession"],
            false
        );
        adapter.acp.store(true, Ordering::SeqCst);
        assert!(plan(&s, &db, &thread, &target)
            .unwrap_err()
            .contains("does not expose a model-switch capability"));
        let options =
            json!({"instanceId":"codex","model":"one","options":[{"id":"effort","value":"high"}]});
        assert_eq!(
            plan(&s, &db, &thread, &options).unwrap().transition,
            Transition::SwitchModelInSession
        );
        db.execute("UPDATE records SET body=json_set(body,'$.nativeThreadRef',NULL) WHERE kind='providerThreads'",[]).unwrap();
        let calls = adapter.seen.lock().unwrap().len();
        assert_eq!(
            plan(&s, &db, &thread, &target).unwrap().transition,
            Transition::CreateWithHandoff
        );
        assert_eq!(adapter.seen.lock().unwrap().len(), calls);
        db.execute("UPDATE records SET body=json_set(body,'$.nativeThreadRef',json('{\"nativeId\":\"native-id\"}')) WHERE kind='providerThreads'",[]).unwrap();
        db.execute("INSERT INTO provider_selection_plans VALUES('t',?)",[json!({"target":thread["modelSelection"],"providerThreadId":"native-row","sourceInstanceId":"codex","transition":{"type":"corrupted"}}).to_string()]).unwrap();
        {
            let tx = db.transaction().unwrap();
            let mut t = thread.clone();
            let mut run = json!({"id":"r","providerInstanceId":"codex","modelSelection":thread["modelSelection"]});
            assert!(admit(&s, &tx, &mut t, &mut run, "now").is_err());
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM provider_selection_plans", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
        drop(db);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
    };
    type DetachCall = (String, String, Option<String>);
    #[derive(Default)]
    struct Peer {
        calls: Mutex<Vec<DetachCall>>,
        fail: AtomicBool,
    }
    impl Provider for Peer {
        fn detach<'a>(
            &'a self,
            instance: &'a str,
            key: &'a str,
            native: Option<&'a str>,
            _: bool,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((
                    instance.into(),
                    key.into(),
                    native.map(str::to_owned),
                ));
                if self.fail.load(Ordering::Acquire) {
                    Err("native detach failed".into())
                } else {
                    Ok(())
                }
            })
        }
        fn execute<'a>(
            &'a self,
            _: RunInput,
            _: tokio::sync::mpsc::Sender<ProviderEvent>,
            _: tokio::sync::mpsc::Receiver<ProviderControl>,
            _: tokio::sync::watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
    fn fixture() -> (State, Value, Arc<Peer>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("o3-binding-test-{}", random_id().unwrap()));
        let state = State::open(&dir).unwrap();
        let peer = Arc::new(Peer::default());
        *state.provider.lock().unwrap() = Some(peer.clone());
        let thread = json!({"id":"t","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"one"},"activeProviderThreadId":"current","providerGeneration":"current-owner","runtimeMode":"full-access","interactionMode":"default"});
        {
            let db = state.db.lock().unwrap();
            put(&db, "thread", "t", "t", &thread).unwrap();
            for (pt, sid, instance, generation, status) in [
                (
                    "current",
                    "current-session",
                    "codex",
                    "current-owner",
                    "ready",
                ),
                (
                    "history",
                    "history-session",
                    "codex",
                    "historical-owner",
                    "ready",
                ),
                (
                    "target",
                    "target-session",
                    "claudeAgent",
                    "target-owner",
                    "ready",
                ),
                (
                    "stopped",
                    "stopped-session",
                    "codex",
                    "stopped-owner",
                    "stopped",
                ),
            ] {
                put(&db,"providerSessions",sid,"t",&json!({"id":sid,"driver":instance,"providerInstanceId":instance,"status":status,"updatedAt":if pt=="current"{"2026-11-04T03:00:00Z"}else{"2026-11-04T01:00:00Z"},"cwd":"/workspace","capabilities":crate::selection_transition::native_capabilities(instance).unwrap()})).unwrap();
                put(&db,"providerThreads",pt,"t",&json!({"id":pt,"appThreadId":"t","providerInstanceId":instance,"driver":instance,"providerSessionId":sid,"nativeThreadRef":{"driver":instance,"nativeId":format!("native-{pt}"),"strength":"strong"},"pendingBackgroundTasks":[],"status":"active"})).unwrap();
                let mut owner = thread.clone();
                owner["providerGeneration"] = json!(generation);
                bound(&db, &owner, &json!({"providerThreadId":pt})).unwrap();
            }
        }
        (state, thread, peer, dir)
    }
    #[test]
    fn changed_workspace_compares_the_live_session_cwd_with_the_target_thread_workspace() {
        let (state, mut thread, _, dir) = fixture();
        thread["worktreePath"] = json!("/new-workspace");
        {
            let db = state.db.lock().unwrap();
            let result = plan(&state, &db, &thread, &thread["modelSelection"]).unwrap();
            assert_eq!(
                result.transition,
                crate::selection_transition::Transition::RestartAndResume
            );
            assert_eq!(result.release_sessions, vec!["current-session"]);
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn selected_historical_bindings_keep_their_owner_keys_across_reopen_and_preserve_target_peers(
    ) {
        let (state, mut thread, peer, dir) = fixture();
        {
            let db = state.db.lock().unwrap();
            selection(
                &state,
                &db,
                &mut thread,
                &json!({"instanceId":"claudeAgent","model":"haiku"}),
                &json!("switch"),
            )
            .unwrap();
        }
        drop(state);
        let state = State::open(&dir).unwrap();
        release_pending(&state, peer.as_ref(), "t").await.unwrap();
        let mut calls = peer.calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                (
                    "codex".into(),
                    "t:current-owner".into(),
                    Some("native-current".into())
                ),
                (
                    "codex".into(),
                    "t:historical-owner".into(),
                    Some("native-history".into())
                )
            ]
        );
        {
            let db = state.db.lock().unwrap();
            assert!(get(&db, "providerSessions", "target-session").is_ok());
            assert!(get(&db, "providerSessions", "stopped-session").is_ok());
            assert!(get(&db, "providerSessions", "current-session").is_err());
            assert!(get(&db, "providerSessions", "history-session").is_err());
            assert_eq!(db.query_row("SELECT count(*) FROM provider_native_bindings WHERE json_extract(payload,'$.releasedAt') IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn historical_selected_session_background_work_refuses_the_whole_selection_atomically() {
        let (state, mut thread, _, dir) = fixture();
        {
            let mut db = state.db.lock().unwrap();
            let tx = db.transaction().unwrap();
            let mut pt = get(&tx, "providerThreads", "history").unwrap();
            pt["pendingBackgroundTasks"] = json!(["native-child"]);
            put(&tx, "providerThreads", "history", "t", &pt).unwrap();
            assert!(selection(
                &state,
                &tx,
                &mut thread,
                &json!({"instanceId":"claudeAgent","model":"haiku"}),
                &json!("switch")
            )
            .unwrap_err()
            .contains("background"));
        }
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM provider_switches", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn failed_ack_preserves_binding_and_retry_then_rebind_does_not_reuse_an_old_release_receipt(
    ) {
        let (state, thread, peer, dir) = fixture();
        {
            let db = state.db.lock().unwrap();
            stage(
                &db,
                &thread,
                &get(&db, "providerThreads", "current").unwrap(),
            )
            .unwrap();
        }
        peer.fail.store(true, Ordering::Release);
        assert!(release_pending(&state, peer.as_ref(), "t").await.is_err());
        {
            let db = state.db.lock().unwrap();
            let payload:String=db.query_row("SELECT payload FROM provider_native_bindings WHERE provider_thread_id='current'",[],|r|r.get(0)).unwrap();
            assert!(serde_json::from_str::<Value>(&payload).unwrap()["releasedAt"].is_null());
        }
        peer.fail.store(false, Ordering::Release);
        release_pending(&state, peer.as_ref(), "t").await.unwrap();
        {
            let db = state.db.lock().unwrap();
            let mut pt = get(&db, "providerThreads", "current").unwrap();
            pt["providerSessionId"] = json!("current-session");
            put(&db, "providerThreads", "current", "t", &pt).unwrap();
            bound(&db, &thread, &json!({"providerThreadId":"current"})).unwrap();
            stage(&db, &thread, &pt).unwrap();
        }
        release_pending(&state, peer.as_ref(), "t").await.unwrap();
        assert_eq!(peer.calls.lock().unwrap().len(), 3);
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM provider_switches WHERE released_at IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
