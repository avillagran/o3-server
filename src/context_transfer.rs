//! Durable fork/merge-back plans and handoff admission. Source boundaries are
//! resolved under the same transaction as their command receipts.
use crate::{
    events::{append, get, put},
    handoff_history as history,
    store::{iso, now_ms, random_id},
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn list(db: &Connection, kind: &str, tid: &str) -> Result<Vec<Value>> {
    let mut q = db
        .prepare("SELECT body FROM records WHERE kind=?1 AND thread_id=?2 ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map([kind, tid], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}
/// Legacy o3 fork plans stored a request tag instead of the canonical source ref.
/// Missing native anchors stay missing, forcing safe portable resolution on admission.
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    // Older native imports predate the handoff list. Preserve malformed lists for validation.
    db.execute("UPDATE records SET body=json_set(body,'$.handoffIds',json('[]')) WHERE kind='providerThreads' AND (json_type(body,'$.handoffIds') IS NULL OR json_type(body,'$.handoffIds')='null')",[]).map_err(|e|e.to_string())?;
    db.execute("UPDATE records SET body=json_set(body,'$.sourcePoint',json_remove(json_set(json_extract(body,'$.sourcePoint'),'$.threadId',json_extract(body,'$.sourceThreadId')),'$.type')) WHERE kind='contextTransfers' AND json_type(body,'$.sourcePoint.type')='text'",[]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(crate) fn source_run(
    db: &Connection,
    tid: &str,
    point: &Value,
    operation: &str,
) -> Result<Option<Value>> {
    let runs = list(db, "runs", tid)?;
    let run = match point["type"].as_str() {
        Some("latest_stable") => runs
            .iter()
            .filter(|r| r["status"] == "completed" && r["checkpointId"].is_string())
            .max_by_key(|r| r["ordinal"].as_i64().unwrap_or(0)),
        Some("run") => runs.iter().find(|r| r["id"] == point["runId"]),
        Some("checkpoint") => {
            let cp = get(
                db,
                "checkpoints",
                point["checkpointId"].as_str().unwrap_or(""),
            )?;
            if cp["threadId"] != tid {
                return Err("Checkpoint belongs to another thread".into());
            }
            runs.iter().find(|r| r["id"] == cp["runId"])
        }
        _ => return Err("invalid sourcePoint".into()),
    };
    if let Some(run) = run {
        if !matches!(
            run["status"].as_str(),
            Some("completed" | "waiting" | "failed" | "interrupted" | "cancelled")
        ) {
            return Err(format!(
                "Fork source run {} is {}; in-progress and rolled-back runs cannot be forked.",
                run["id"].as_str().unwrap_or(""),
                run["status"].as_str().unwrap_or("")
            ));
        }
    }
    if run.is_none() && (operation != "side_chat" || point["type"] != "latest_stable") {
        return Err(format!(
            "No stable source run was found for {operation} source {}.",
            point["type"].as_str().unwrap_or("")
        ));
    }
    Ok(run.cloned())
}
pub(crate) fn canonical(db: &Connection, tid: &str, run: &Value) -> Value {
    let mut p = json!({"threadId":tid,"runId":run["id"]});
    if run["checkpointId"].is_string() {
        p["checkpointId"] = run["checkpointId"].clone()
    }
    if let Ok(thread) = get(
        db,
        "providerThreads",
        run["providerThreadId"].as_str().unwrap_or(""),
    ) {
        if thread["nativeThreadRef"].is_object() {
            p["providerThreadRef"] = thread["nativeThreadRef"].clone()
        }
    }
    let turns = list(db, "providerTurns", tid).unwrap_or_default();
    if let Some(turn) = turns
        .iter()
        .rev()
        .find(|v| v["runAttemptId"] == run["activeAttemptId"] && v["runAttemptId"].is_string())
    {
        if turn["nativeTurnRef"].is_object() {
            p["providerTurnRef"] = turn["nativeTurnRef"].clone()
        }
    }
    p
}
pub(crate) fn fork_record(
    db: &Connection,
    source: &Value,
    target: &Value,
    run: Option<&Value>,
    at: &str,
) -> Result<i64> {
    let source_id = source["id"].as_str().ok_or("source id missing")?;
    let tid = target["id"].as_str().ok_or("target id missing")?;
    let id = random_id()?;
    let point = run
        .map(|r| canonical(db, source_id, r))
        .unwrap_or(json!({"threadId":source_id}));
    let strong = point["providerThreadRef"]["strength"] == "strong";
    let mut transfer = json!({"id":id,"type":"fork","sourceThreadId":source_id,"targetThreadId":tid,"sourcePoint":point,"basePoint":null,"sourceProviderInstanceId":run.map(|r|r["providerInstanceId"].clone()).unwrap_or(source["providerInstanceId"].clone()),"targetProviderInstanceId":null,"targetRunId":null,"status":"pending","resolution":null,"createdBy":target["createdBy"],"error":if strong{Value::Null}else{json!("Source provider thread does not expose a strong native thread ref.")},"createdAt":at,"updatedAt":at,"consumedAt":null});
    if strong {
        if let Some(r) =
            run.filter(|r| matches!(r["status"].as_str(), Some("completed" | "waiting")))
        {
            if let Some(turn) = list(db, "providerTurns", source_id)?
                .iter()
                .rev()
                .find(|t| t["runAttemptId"] == r["activeAttemptId"] && t["tokenUsage"].is_object())
            {
                transfer["sourceTokenUsage"] = turn["tokenUsage"].clone()
            }
        }
    }
    put(db, "contextTransfers", &id, tid, &transfer)?;
    append(db, tid, "context-transfer.created", transfer, target)
}
pub(crate) fn merge_back(db: &Connection, c: &Value) -> Result<i64> {
    let sid = c["sourceThreadId"]
        .as_str()
        .ok_or("sourceThreadId required")?;
    let tid = c["targetThreadId"]
        .as_str()
        .ok_or("targetThreadId required")?;
    let source = get(db, "thread", sid)?;
    let target = get(db, "thread", tid)?;
    if !source["deletedAt"].is_null() {
        return Err("The source thread was deleted".into());
    }
    if source["projectId"] != target["projectId"] {
        return Err("Context can only be merged within the same project".into());
    }
    if source["lineage"]["relationshipToParent"] != "fork"
        || source["lineage"]["parentThreadId"] != tid
    {
        return Err(format!("Thread {sid} is not a fork of {tid}."));
    }
    if !target["deletedAt"].is_null() {
        return Err("The target thread was deleted".into());
    }
    if list(db, "runs", tid)?.iter().any(|r| {
        matches!(
            r["status"].as_str(),
            Some("preparing" | "starting" | "running" | "waiting" | "queued")
        )
    }) {
        return Err("Finish or stop the target thread's active work and clear its queue before merging back.".into());
    }
    let transfers = list(db, "contextTransfers", tid)?;
    if transfers.iter().any(|t| {
        t["type"] == "merge_back"
            && t["status"] == "pending"
            && t["targetThreadId"] == tid
            && t["sourceThreadId"] != sid
    }) {
        return Err(format!(
            "Thread {tid} already has pending context from another side chat or fork. Send a message there first."
        ));
    }
    let run = source_run(db, sid, &c["sourcePoint"], "merge-back")?.unwrap();
    if !matches!(run["status"].as_str(), Some("completed" | "waiting")) {
        return Err(format!(
            "Merge-back source run {} is {}; only provider-finished runs are supported.",
            run["id"].as_str().unwrap_or(""),
            run["status"].as_str().unwrap_or("")
        ));
    }
    if run["status"] == "waiting"
        && list(db, "providerTurns", sid)?.iter().any(|t| {
            t["runAttemptId"] == run["activeAttemptId"]
                && matches!(
                    t["status"].as_str(),
                    Some("running" | "waiting" | "starting" | "pending")
                )
        })
    {
        return Err("Merge-back requires a provider-finished source run; this run is waiting for a live provider request.".into());
    }
    let fork = list(db, "contextTransfers", sid)?
        .into_iter()
        .rev()
        .find(|t| t["type"] == "fork" && t["sourceThreadId"] == tid && t["targetThreadId"] == sid)
        .ok_or_else(|| format!("No fork transfer exists between {tid} and {sid}."))?;
    let at = c["createdAt"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| iso(now_ms()));
    let id = random_id()?;
    for mut previous in transfers.into_iter().filter(|t| {
        t["type"] == "merge_back" && t["status"] == "pending" && t["sourceThreadId"] == sid
    }) {
        previous["status"] = json!("superseded");
        previous["error"] = json!(format!("Superseded by merge-back transfer {id}."));
        previous["updatedAt"] = json!(at);
        save_transfer(db, &target, previous)?;
    }
    let pt = get(
        db,
        "providerThreads",
        run["providerThreadId"].as_str().unwrap_or(""),
    )
    .ok();
    let transfer = json!({"id":id,"type":"merge_back","sourceThreadId":sid,"targetThreadId":tid,"sourcePoint":canonical(db,sid,&run),"basePoint":fork["sourcePoint"],"sourceProviderInstanceId":run["providerInstanceId"],"targetProviderInstanceId":target["modelSelection"]["instanceId"],"targetRunId":null,"status":"pending","resolution":null,"createdBy":c["createdBy"].as_str().unwrap_or("user"),"error":if pt.is_some(){Value::Null}else{json!("Source merge-back run has no provider thread.")},"createdAt":at,"updatedAt":at,"consumedAt":null});
    put(db, "contextTransfers", &id, tid, &transfer)?;
    append(db, tid, "context-transfer.created", transfer, &target)
}
fn save_transfer(db: &Connection, thread: &Value, transfer: Value) -> Result<i64> {
    let id = transfer["id"].as_str().ok_or("transfer id missing")?;
    let tid = thread["id"].as_str().ok_or("thread id missing")?.to_owned();
    put(db, "contextTransfers", id, &tid, &transfer)?;
    append(db, &tid, "context-transfer.updated", transfer, thread)
}
pub(crate) fn own_items(db: &Connection, tid: &str, through: i64) -> Result<Vec<Value>> {
    let mut q=db.prepare("SELECT i.body FROM records i LEFT JOIN records r ON r.kind='runs' AND r.id=json_extract(i.body,'$.runId') WHERE i.kind='turnItems' AND i.thread_id=?1 AND (json_extract(i.body,'$.runId') IS NULL OR (json_extract(r.body,'$.ordinal')<=?2 AND coalesce(json_extract(r.body,'$.contextDiscarded'),0)=0 AND json_extract(r.body,'$.status')!='rolled_back' AND NOT(json_extract(r.body,'$.status')='cancelled' AND json_extract(i.body,'$.type')='user_message' AND coalesce(json_extract(i.body,'$.inputIntent'),'')='queued_turn'))) ORDER BY r.rowid,json_extract(i.body,'$.ordinal'),i.rowid").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![tid, through], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut out = vec![];
    for row in rows {
        let item: Value =
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        out.push(item)
    }
    let attempts = list(db, "attempts", tid)?;
    let interrupts = out
        .iter()
        .filter(|i| i["type"] == "run_interrupt_request")
        .map(|i| i["runId"].clone())
        .collect::<Vec<_>>();
    out.retain(|i| {
        !(i["type"] == "run_interrupt_result"
            && !interrupts.contains(&i["runId"])
            && attempts.iter().any(|a| {
                a["status"] == "superseded"
                    && a["runId"] == i["runId"]
                    && a["rootNodeId"] == i["nodeId"]
            }))
    });
    Ok(out)
}
pub(crate) fn fork_items(db: &Connection, tid: &str, through: i64) -> Result<Vec<Value>> {
    let mut segments = vec![own_items(db, tid, through)?];
    let mut thread = get(db, "thread", tid)?;
    let mut seen = std::collections::HashSet::from([tid.to_owned()]);
    while thread["forkedFrom"]["type"] == "run" && thread["lineage"]["sideChat"] != true {
        let Some(sid) = thread["forkedFrom"]["threadId"].as_str() else {
            break;
        };
        if !seen.insert(sid.to_owned()) {
            break;
        }
        let Ok(source) = get(db, "thread", sid) else {
            break;
        };
        let Ok(run) = get(
            db,
            "runs",
            thread["forkedFrom"]["runId"].as_str().unwrap_or(""),
        ) else {
            break;
        };
        segments.insert(0, own_items(db, sid, run["ordinal"].as_i64().unwrap_or(0))?);
        thread = source;
    }
    Ok(segments.into_iter().flatten().collect())
}
pub(crate) fn range(items: &[Value], runs: &[Value]) -> Value {
    let mut from = i64::MAX;
    let mut to = 1;
    for item in items {
        if let Some(run) = runs.iter().find(|r| r["id"] == item["runId"]) {
            let n = run["ordinal"].as_i64().unwrap_or(1);
            from = from.min(n);
            to = to.max(n)
        }
    }
    json!({"from":if from==i64::MAX{1}else{from},"to":to})
}
/// Resolve ordinary, nonqueued transfer consumption with the new run's admission.
pub(crate) fn admit(
    state: &crate::store::State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    queued: bool,
    at: &str,
) -> Result<()> {
    admit_inner(state, db, thread, run, queued, at, false)
}
pub(crate) fn portable_fork_fallback(
    state: &crate::store::State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    at: &str,
) -> Result<String> {
    admit_inner(state, db, thread, run, false, at, true)?;
    put(
        db,
        "runs",
        run["id"].as_str().unwrap_or(""),
        thread["id"].as_str().unwrap_or(""),
        run,
    )?;
    append(
        db,
        thread["id"].as_str().unwrap_or(""),
        "run.updated",
        run.clone(),
        thread,
    )?;
    run["contextHandoffId"]
        .as_str()
        .map(str::to_owned)
        .ok_or("Portable fork did not prepare context".into())
}
fn admit_inner(
    state: &crate::store::State,
    db: &Connection,
    thread: &mut Value,
    run: &mut Value,
    queued: bool,
    at: &str,
    force_portable: bool,
) -> Result<()> {
    let tid = thread["id"]
        .as_str()
        .ok_or("thread id required")?
        .to_owned();
    let transfers = list(db, "contextTransfers", &tid)?;
    let merges = transfers
        .iter()
        .filter(|t| {
            t["type"] == "merge_back" && t["status"] == "pending" && t["targetThreadId"] == tid
        })
        .collect::<Vec<_>>();
    if queued && !merges.is_empty() {
        return Err(format!(
            "Thread {tid} has a pending merge-back transfer; queued merge-back consumption is not implemented yet."
        ));
    }
    if queued {
        return Ok(());
    }
    if merges
        .iter()
        .map(|t| t["sourceThreadId"].as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1
    {
        return Err(format!(
            "Thread {tid} has pending merge-back transfers from multiple forks."
        ));
    }
    let merge = merges
        .into_iter()
        .max_by_key(|t| t["updatedAt"].as_str().unwrap_or(""));
    let fork = transfers
        .iter()
        .find(|t| t["type"] == "fork" && t["status"] == "pending" && t["targetThreadId"] == tid);
    for transfer in [fork, merge].into_iter().flatten() {
        let sid = transfer["sourceThreadId"]
            .as_str()
            .ok_or("source thread missing")?;
        get(db, "thread", sid)?;
        let source_run = get(
            db,
            "runs",
            transfer["sourcePoint"]["runId"].as_str().unwrap_or(""),
        )
        .ok();
        let is_merge = transfer["type"] == "merge_back";
        let sidechat = thread["lineage"]["sideChat"] == true;
        if source_run.is_none() && !sidechat {
            return Err(format!(
                "Pending transfer {} has no resolvable source run.",
                transfer["id"].as_str().unwrap_or("")
            ));
        }
        let source_pt = source_run.as_ref().and_then(|r| {
            get(
                db,
                "providerThreads",
                r["providerThreadId"].as_str().unwrap_or(""),
            )
            .ok()
        });
        if is_merge && source_pt.is_none() {
            return Err(format!(
                "Pending merge-back transfer {} has no resolvable source provider thread.",
                transfer["id"].as_str().unwrap_or("")
            ));
        }
        let mut updated = transfer.clone();
        updated["targetProviderInstanceId"] = run["providerInstanceId"].clone();
        updated["targetRunId"] = run["id"].clone();
        updated["error"] = Value::Null;
        updated["updatedAt"] = json!(at);
        let source_runs = list(db, "runs", sid)?;
        let at_tip = source_run.as_ref().is_some_and(|r| {
            source_runs
                .iter()
                .max_by_key(|r| r["ordinal"].as_i64().unwrap_or(0))
                .is_some_and(|last| last["id"] == r["id"] && last["status"] == "completed")
        });
        let point = &transfer["sourcePoint"];
        let anchored = point["providerThreadRef"]["driver"] == "codex"
            && point["providerTurnRef"]["strength"] == "strong";
        let target_driver = crate::provider_input::driver(state, &run["providerInstanceId"])?;
        let native = !force_portable
            && point["providerThreadRef"]["driver"] == target_driver
            && !is_merge
            && source_run.as_ref().is_some_and(|r| {
                r["providerInstanceId"] == run["providerInstanceId"]
                    && matches!(r["status"].as_str(), Some("completed" | "waiting"))
            })
            && point["providerThreadRef"]["strength"] == "strong"
            && (anchored || at_tip);
        if native {
            thread["nativeForkSource"] = point["providerThreadRef"]["nativeId"].clone();
            thread["nativeForkLastTurnId"] = if anchored {
                point["providerTurnRef"]["nativeId"].clone()
            } else {
                Value::Null
            };
            thread["nativeForkTransferId"] = transfer["id"].clone();
            save_transfer(db, thread, updated)?;
            if sidechat {
                side_chat_handoff(db, thread, run, transfer, source_run.as_ref(), at)?;
            }
            continue;
        }
        if !is_merge {
            thread["nativeForkSource"] = Value::Null;
            thread["nativeForkLastTurnId"] = Value::Null;
        }
        if is_merge && thread["providerHandoff"].is_string() {
            parent_handoff(db, thread, run, at)?;
        }
        let through = source_run
            .as_ref()
            .and_then(|r| r["ordinal"].as_i64())
            .unwrap_or(i64::MAX);
        if sidechat && source_run.is_none() {
            let id = side_chat_handoff(db, thread, run, transfer, None, at)?;
            updated["status"] = json!("resolved_portable");
            updated["resolution"] = json!({"strategy":"portable_context","contextHandoffId":id});
            save_transfer(db, thread, updated)?;
            thread["providerHandoff"] = Value::Null;
            thread["providerHandoffAttachments"] = json!([]);
            continue;
        }
        let items = if is_merge {
            own_items(db, sid, through)?
        } else {
            fork_items(db, sid, through)?
        };
        let runs = list(db, "runs", sid)?;
        let coverage_range = range(&items, &runs);
        let messages = items
            .iter()
            .filter_map(history::message)
            .collect::<Vec<_>>();
        let coverage = history::coverage(
            sid,
            coverage_range["from"].as_i64().unwrap_or(1),
            coverage_range["to"].as_i64().unwrap_or(1),
            &items,
        );
        let selected = history::select(&messages, &coverage, 0, history::cap());
        let id = random_id()?;
        let strategy = if is_merge {
            "fork_delta_summary"
        } else {
            "full_thread_summary"
        };
        let context = selected["context"].as_str().unwrap();
        let selected_messages = selected["messages"].as_array().unwrap();
        let summary = if is_merge {
            history::delta_summary(sid, &tid, &coverage_range, &items)
        } else {
            history::render(selected_messages, context)
        };
        let handoff = json!({"id":id,"transferId":transfer["id"],"threadId":tid,"targetRunId":run["id"],"fromProviderThreadIds":source_pt.as_ref().map(|p|vec![p["id"].clone()]).unwrap_or_default(),"toProviderThreadId":run["providerThreadId"],"coveredRunOrdinals":coverage_range,"strategy":strategy,"status":"ready","summaryMessageId":null,"summaryText":summary,"history":{"messages":selected["messages"],"coverage":coverage,"omittedItems":selected["omittedItems"],"omittedItemIds":selected["omittedItemIds"]},"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at});
        let mut pt = get(
            db,
            "providerThreads",
            run["providerThreadId"].as_str().unwrap_or(""),
        )?;
        pt["handoffIds"]
            .as_array_mut()
            .ok_or("provider handoffIds missing")?
            .push(json!(id));
        put(
            db,
            "providerThreads",
            pt["id"].as_str().unwrap_or(""),
            &tid,
            &pt,
        )?;
        append(db, &tid, "provider-thread.updated", pt, thread)?;
        put(db, "contextHandoffs", &id, &tid, &handoff)?;
        append(db, &tid, "context-handoff.updated", handoff.clone(), thread)?;
        run["contextHandoffId"] = json!(id);
        thread["providerHandoff"] = Value::Null;
        thread["providerHandoffAttachments"] = json!([]);
        updated["status"] = json!(if is_merge {
            "consumed"
        } else {
            "resolved_portable"
        });
        updated["resolution"] = json!({"strategy":if is_merge{"fork_delta_context"}else{"portable_context"},"contextHandoffId":id});
        if is_merge {
            updated["consumedAt"] = json!(at)
        }
        save_transfer(db, thread, updated)?;
        let item_id = format!("{}-handoff-{id}", run["id"].as_str().unwrap_or(""));
        let ordinal=db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",[&tid],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?;
        let item = json!({"id":item_id,"threadId":tid,"runId":run["id"],"nodeId":run["rootNodeId"],"providerThreadId":run["providerThreadId"],"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"completed","title":if is_merge{"Merge-back context"}else{"Fork context"},"startedAt":at,"completedAt":at,"updatedAt":at,"type":"handoff","contextHandoffId":id,"fromProviderThreadIds":handoff["fromProviderThreadIds"],"toProviderThreadId":run["providerThreadId"],"fromProviderInstanceIds":[transfer["sourceProviderInstanceId"]],"toProviderInstanceId":run["providerInstanceId"],"fromModelSelections":source_run.as_ref().map(|r|vec![r["modelSelection"].clone()]).unwrap_or_default(),"toModel":run["modelSelection"]["model"],"strategy":strategy,"summary":summary});
        crate::projection::save_item(db, &tid, thread, item)?;
        if sidechat {
            side_chat_handoff(db, thread, run, transfer, source_run.as_ref(), at)?;
        }
    }
    Ok(())
}

fn side_chat_handoff(
    db: &Connection,
    thread: &Value,
    run: &mut Value,
    transfer: &Value,
    source_run: Option<&Value>,
    at: &str,
) -> Result<String> {
    if let Some(existing) = list(db, "contextHandoffs", thread["id"].as_str().unwrap_or(""))?
        .into_iter()
        .find(|h| {
            h["targetRunId"] == run["id"]
                && h["strategy"] == "manual_context"
                && h["transferId"] == transfer["id"]
        })
    {
        return Ok(existing["id"]
            .as_str()
            .ok_or("Side chat context id missing")?
            .to_owned());
    }
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let sid = transfer["sourceThreadId"]
        .as_str()
        .ok_or("source missing")?;
    let runs = list(db, "runs", sid)?;
    let mut items = if source_run.is_none() {
        fork_items(db, sid, i64::MAX)?
    } else {
        own_items(db, sid, i64::MAX)?
    };
    items.retain(|i| match runs.iter().find(|r| r["id"] == i["runId"]) {
        Some(r) => {
            r["status"] != "queued"
                && source_run.is_none_or(|start| r["ordinal"].as_i64() > start["ordinal"].as_i64())
        }
        None => source_run.is_none(),
    });
    let id = random_id()?;
    let handoff = json!({"id":id,"transferId":transfer["id"],"threadId":tid,"targetRunId":run["id"],"fromProviderThreadIds":[],"toProviderThreadId":run["providerThreadId"],"coveredRunOrdinals":range(&items,&runs),"strategy":"manual_context","status":"ready","summaryMessageId":null,"summaryText":history::side_chat(sid,&items),"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at});
    put(db, "contextHandoffs", &id, tid, &handoff)?;
    append(db, tid, "context-handoff.updated", handoff, thread)?;
    let mut pt = get(
        db,
        "providerThreads",
        run["providerThreadId"].as_str().unwrap_or(""),
    )?;
    pt["handoffIds"]
        .as_array_mut()
        .ok_or("provider handoffIds missing")?
        .push(json!(id));
    put(
        db,
        "providerThreads",
        pt["id"].as_str().unwrap_or(""),
        tid,
        &pt,
    )?;
    append(db, tid, "provider-thread.updated", pt, thread)?;
    if run["contextHandoffId"].is_null() {
        run["contextHandoffId"] = json!(id)
    }
    Ok(id)
}
/// Switching providers while consuming a fork delta also needs the parent's full context.
fn parent_handoff(db: &Connection, thread: &Value, run: &Value, at: &str) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let runs = list(db, "runs", tid)?;
    let Some(source_run) = runs
        .iter()
        .filter(|r| r["status"] != "queued")
        .max_by_key(|r| r["ordinal"].as_i64().unwrap_or(0))
    else {
        return Ok(());
    };
    let items = fork_items(db, tid, source_run["ordinal"].as_i64().unwrap_or(0))?;
    let range = range(&items, &runs);
    let messages = items
        .iter()
        .filter_map(history::message)
        .collect::<Vec<_>>();
    let coverage = history::coverage(
        tid,
        range["from"].as_i64().unwrap_or(1),
        range["to"].as_i64().unwrap_or(1),
        &items,
    );
    let selected = history::select(&messages, &coverage, 0, history::cap());
    let id = random_id()?;
    let transfer_id = random_id()?;
    let h = json!({"id":id,"transferId":transfer_id,"threadId":tid,"targetRunId":run["id"],"fromProviderThreadIds":[source_run["providerThreadId"]],"toProviderThreadId":run["providerThreadId"],"coveredRunOrdinals":range,"strategy":"full_thread_summary","status":"ready","summaryMessageId":null,"summaryText":history::render(selected["messages"].as_array().unwrap(),selected["context"].as_str().unwrap()),"history":{"messages":selected["messages"],"coverage":coverage,"omittedItems":selected["omittedItems"],"omittedItemIds":selected["omittedItemIds"]},"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at});
    put(db, "contextHandoffs", &id, tid, &h)?;
    append(db, tid, "context-handoff.updated", h, thread)?;
    let transfer = json!({"id":transfer_id,"type":"provider_handoff","sourceThreadId":tid,"targetThreadId":tid,"sourcePoint":canonical(db,tid,source_run),"basePoint":null,"sourceProviderInstanceId":source_run["providerInstanceId"],"targetProviderInstanceId":run["providerInstanceId"],"targetRunId":run["id"],"status":"consumed","resolution":{"strategy":"portable_context","contextHandoffId":id},"createdBy":"user","error":null,"createdAt":at,"updatedAt":at,"consumedAt":at});
    put(db, "contextTransfers", &transfer_id, tid, &transfer)?;
    append(db, tid, "context-transfer.created", transfer, thread)?;
    let mut pt = get(
        db,
        "providerThreads",
        run["providerThreadId"].as_str().unwrap_or(""),
    )?;
    pt["handoffIds"]
        .as_array_mut()
        .ok_or("provider handoffIds missing")?
        .push(json!(id));
    put(
        db,
        "providerThreads",
        pt["id"].as_str().unwrap_or(""),
        tid,
        &pt,
    )?;
    append(db, tid, "provider-thread.updated", pt, thread)?;
    Ok(())
}
/// Only persisted handoffs enter adapter input; callers cannot supply this field.
pub(crate) fn input(db: &Connection, thread: &Value, run: &Value) -> Result<Value> {
    let tid = thread["id"].as_str().unwrap_or("");
    let mut all = list(db, "contextHandoffs", tid)?;
    let runs = list(db, "runs", tid)?;
    let Some(ptid) = run["providerThreadId"].as_str() else {
        // Legacy active-recovery runs predate app-owned provider rows.
        return Ok(json!({"handoffs":[],"deliveredItemIds":[]}));
    };
    let native = get(db, "providerThreads", ptid)?["nativeThreadRef"]["nativeId"].clone();
    let recovery = crate::native_recovery::handoff_id(db, run)?;
    if recovery.is_none() {
        if let Some(retry) = retry_handoff(db, thread, run, &all, &native)? {
            all.push(retry);
        }
    }
    let mut handoffs = all
        .iter()
        .filter(|h| {
            let manual = thread["lineage"]["sideChat"] == true
                && h["strategy"] == "manual_context"
                && h["transferId"].is_string();
            let prior = runs.iter().find(|r| r["id"] == h["targetRunId"]);
            let eligible = h["targetRunId"] == run["id"]
                || ((manual || h["toProviderThreadId"] == run["providerThreadId"])
                    && prior.is_some_and(|r| {
                        matches!(
                            r["status"].as_str(),
                            Some("failed" | "interrupted" | "cancelled")
                        ) || (r["status"] == "completed"
                            && h.get("delivery").is_none()
                            && get(db, "messages", r["userMessageId"].as_str().unwrap_or(""))
                                .is_ok_and(|m| crate::handoff_delivery::maintenance(&m["text"])))
                    }));
            h["status"] == "ready"
                && eligible
                && (!native.is_string()
                    || h["delivery"]["nativeThreadId"] != native
                    || !matches!(
                        h["delivery"]["status"].as_str(),
                        Some("inline" | "injected")
                    ))
        })
        .collect::<Vec<_>>();
    handoffs.sort_by_key(|h| {
        if h["targetRunId"] != run["id"] {
            0
        } else if recovery.as_ref().is_some_and(|id| h["id"] == id.as_str()) {
            1
        } else {
            2
        }
    });
    let delivered = all
        .iter()
        .filter(|h| {
            h["toProviderThreadId"] == run["providerThreadId"]
                && h["delivery"]["nativeThreadId"] == native
                && native.is_string()
                && matches!(
                    h["delivery"]["status"].as_str(),
                    Some("inline" | "injected")
                )
        })
        .flat_map(|h| h["delivery"]["itemIds"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    Ok(json!({"handoffs":handoffs,"deliveredItemIds":delivered}))
}
fn retry_handoff(
    db: &Connection,
    thread: &Value,
    run: &Value,
    handoffs: &[Value],
    native: &Value,
) -> Result<Option<Value>> {
    let tid = thread["id"].as_str().unwrap_or("");
    let turns = list(db, "providerTurns", tid)?;
    let runs = list(db, "runs", tid)?;
    let missed = runs
        .iter()
        .filter(|r| {
            r["providerThreadId"] == run["providerThreadId"]
                && r["ordinal"].as_i64() < run["ordinal"].as_i64()
                && matches!(r["status"].as_str(), Some("failed" | "interrupted"))
                && !turns
                    .iter()
                    .any(|t| t["runAttemptId"] == r["activeAttemptId"])
        })
        .collect::<Vec<_>>();
    if missed.is_empty() {
        return Ok(None);
    }
    let covered = handoffs
        .iter()
        .filter(|h| {
            h["toProviderThreadId"] == run["providerThreadId"]
                && h["delivery"]["nativeThreadId"] == *native
                && matches!(
                    h["delivery"]["status"].as_str(),
                    Some("inline" | "injected")
                )
        })
        .flat_map(|h| [&h["delivery"]["itemIds"], &h["delivery"]["omittedItemIds"]])
        .flat_map(|a| a.as_array().into_iter().flatten())
        .collect::<std::collections::HashSet<_>>();
    let items = own_items(db, tid, run["ordinal"].as_i64().unwrap_or(1) - 1)?
        .into_iter()
        .filter(|i| {
            missed.iter().any(|r| r["id"] == i["runId"])
                && !covered.contains(&i["id"])
                && history::message(i).is_some()
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return Ok(None);
    }
    let from = missed
        .iter()
        .filter_map(|r| r["ordinal"].as_i64())
        .min()
        .unwrap_or(1);
    let to = missed
        .iter()
        .filter_map(|r| r["ordinal"].as_i64())
        .max()
        .unwrap_or(1);
    let coverage = history::coverage(tid, from, to, &items);
    let messages = items
        .iter()
        .filter_map(history::message)
        .collect::<Vec<_>>();
    let selected = history::select(&messages, &coverage, 0, history::cap());
    let at = iso(now_ms());
    Ok(Some(
        json!({"id":random_id()?,"transferId":null,"threadId":tid,"targetRunId":run["id"],"fromProviderThreadIds":[run["providerThreadId"]],"toProviderThreadId":run["providerThreadId"],"coveredRunOrdinals":{"from":from,"to":to},"strategy":"delta_since_target_last_seen","status":"ready","summaryMessageId":null,"summaryText":history::render(selected["messages"].as_array().unwrap(),selected["context"].as_str().unwrap()),"history":{"messages":selected["messages"],"coverage":coverage,"omittedItems":selected["omittedItems"],"omittedItemIds":selected["omittedItemIds"]},"createdByProviderInstanceId":null,"createdAt":at,"updatedAt":at,"_o3Ephemeral":true}),
    ))
}
/// Called after the adapter's actual native binding, in the output transaction.
pub(crate) fn bound(db: &Connection, thread: &mut Value, run: &Value, at: &str) -> Result<()> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?.to_owned();
    let Some(id) = thread["nativeForkTransferId"].as_str() else {
        return Ok(());
    };
    let mut transfer = get(db, "contextTransfers", id)?;
    if transfer["status"] != "pending" || transfer["targetRunId"] != run["id"] {
        return Ok(());
    }
    let pt = get(
        db,
        "providerThreads",
        run["providerThreadId"].as_str().unwrap_or(""),
    )?;
    let reference = &pt["nativeThreadRef"];
    if reference["strength"] != "strong"
        || reference["nativeId"] == transfer["sourcePoint"]["providerThreadRef"]["nativeId"]
    {
        return Err("Native fork did not produce a distinct strong thread ref".into());
    }
    transfer["status"] = json!("consumed");
    transfer["resolution"] = json!({"strategy":"native_fork","providerThreadRef":reference});
    transfer["consumedAt"] = json!(at);
    transfer["updatedAt"] = json!(at);
    transfer["error"] = Value::Null;
    let mut pt = pt.clone();
    let source_run = get(
        db,
        "runs",
        transfer["sourcePoint"]["runId"].as_str().unwrap_or(""),
    )?;
    pt["forkedFrom"] = json!({"providerThreadId":source_run["providerThreadId"]});
    if let Some(turn) = list(
        db,
        "providerTurns",
        transfer["sourceThreadId"].as_str().unwrap_or(""),
    )?
    .into_iter()
    .find(|t| {
        t["nativeTurnRef"] == transfer["sourcePoint"]["providerTurnRef"]
            && t["nativeTurnRef"].is_object()
    }) {
        pt["forkedFrom"]["providerTurnId"] = turn["id"].clone();
    }
    put(
        db,
        "providerThreads",
        pt["id"].as_str().unwrap_or(""),
        &tid,
        &pt,
    )?;
    append(db, &tid, "provider-thread.updated", pt, thread)?;
    save_transfer(db, thread, transfer)?;
    thread["nativeForkSource"] = Value::Null;
    thread["nativeForkLastTurnId"] = Value::Null;
    thread["nativeForkTransferId"] = Value::Null;
    put(db, "thread", &tid, &tid, thread)?;
    Ok(())
}
/// Commit before acknowledging any provider-side history mutation.
pub(crate) fn delivery(
    s: &crate::store::State,
    tid: &str,
    rid: &str,
    expected: Option<&str>,
    data: Value,
) -> Result<()> {
    let mut db = s.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if !crate::run_restart::matches(&tx, rid, expected) {
        return Err("Stale handoff delivery attempt".into());
    }
    let thread = get(&tx, "thread", tid)?;
    let run = get(&tx, "runs", rid)?;
    if !thread["deletedAt"].is_null()
        || matches!(
            run["status"].as_str(),
            Some("completed" | "cancelled" | "failed" | "interrupted" | "rolled_back")
        )
    {
        return Err("Handoff delivery run is no longer active".into());
    }
    let id = data["handoffId"].as_str().ok_or("handoff id missing")?;
    use rusqlite::OptionalExtension;
    let body: Option<String> = tx
        .query_row(
            "SELECT body FROM records WHERE kind='contextHandoffs' AND id=?",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let mut h = match body {
        Some(body) => serde_json::from_str(&body).map_err(|e| e.to_string())?,
        None => data
            .get("handoff")
            .cloned()
            .filter(|h| {
                h["id"] == id
                    && h["targetRunId"] == rid
                    && h["status"] == "ready"
                    && h["transferId"].is_null()
            })
            .ok_or("Retry handoff snapshot missing")?,
    };
    if h["threadId"] != tid || h["toProviderThreadId"] != run["providerThreadId"] {
        return Err("Handoff delivery owner mismatch".into());
    }
    let native = data["nativeThreadId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("native delivery binding missing")?;
    let status = data["status"].as_str().ok_or("delivery status missing")?;
    match status {
        "pending" if h["delivery"]["status"]=="pending" && h["delivery"]["nativeThreadId"]==native=>return Err("Context handoff delivery is uncertain; create a fresh conversation before replaying history.".into()),
        "pending" if h["delivery"]["nativeThreadId"]==native && matches!(h["delivery"]["status"].as_str(),Some("injected"|"inline"))=>return Err("Context handoff was already delivered".into()),
        "pending"=>{},
        "injected"|"inline" if h["delivery"]["status"]=="pending" && h["delivery"]["nativeThreadId"]==native=>{},
        "unsent" if h["delivery"]["status"]=="pending" && h["delivery"]["nativeThreadId"]==native=>{},
        _=>return Err("Invalid context delivery transition".into()),
    }
    let at = iso(now_ms());
    h["delivery"] = json!({"nativeThreadId":native,"status":status,"itemIds":data["itemIds"],"omittedItemIds":data["omittedItemIds"]});
    if status == "unsent" {
        h.as_object_mut().unwrap().remove("delivery");
    }
    h["updatedAt"] = json!(at);
    put(&tx, "contextHandoffs", id, tid, &h)?;
    append(&tx, tid, "context-handoff.updated", h.clone(), &thread)?;
    if matches!(status, "inline" | "injected") {
        if let Some(id) = h["transferId"].as_str() {
            let mut transfer = get(&tx, "contextTransfers", id)?;
            if transfer["status"] == "resolved_portable" {
                transfer["status"] = json!("consumed");
                transfer["consumedAt"] = json!(at);
                transfer["updatedAt"] = json!(at);
                save_transfer(&tx, &thread, transfer)?;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop(db);
    s.notify();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_fork_sources_upgrade_idempotently_without_inventing_native_anchors() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        let old = json!({"id":"legacy","sourceThreadId":"source","targetThreadId":"target","status":"pending","sourcePoint":{"type":"run","runId":"boundary"}});
        put(&db, "contextTransfers", "legacy", "target", &old).unwrap();
        initialize(&db).unwrap();
        let upgraded = get(&db, "contextTransfers", "legacy").unwrap();
        assert_eq!(
            upgraded["sourcePoint"],
            json!({"threadId":"source","runId":"boundary"})
        );
        assert_eq!(upgraded["status"], "pending");
        initialize(&db).unwrap();
        assert_eq!(get(&db, "contextTransfers", "legacy").unwrap(), upgraded);
    }
    #[test]
    fn stale_and_terminal_delivery_receipts_cannot_mutate_persisted_handoffs() {
        let dir = std::env::temp_dir().join(format!("delivery-fence-{}", random_id().unwrap()));
        let s = crate::store::State::open(&dir).unwrap();
        {
            let db = s.db.lock().unwrap();
            put(&db, "thread", "t", "t", &json!({"id":"t","deletedAt":null})).unwrap();
            put(
                &db,
                "runs",
                "r",
                "t",
                &json!({"id":"r","threadId":"t","status":"running","activeAttemptId":"new"}),
            )
            .unwrap();
            put(
                &db,
                "contextHandoffs",
                "h",
                "t",
                &json!({"id":"h","threadId":"t","delivery":{"status":"pending"}}),
            )
            .unwrap();
        }
        assert!(delivery(
            &s,
            "t",
            "r",
            Some("old"),
            json!({"handoffId":"h","status":"injected"})
        )
        .unwrap_err()
        .contains("Stale"));
        {
            let db = s.db.lock().unwrap();
            let mut r = get(&db, "runs", "r").unwrap();
            r["status"] = json!("failed");
            put(&db, "runs", "r", "t", &r).unwrap();
        }
        assert!(delivery(
            &s,
            "t",
            "r",
            Some("new"),
            json!({"handoffId":"h","status":"injected"})
        )
        .unwrap_err()
        .contains("no longer active"));
        assert_eq!(
            get(&s.db.lock().unwrap(), "contextHandoffs", "h").unwrap()["delivery"]["status"],
            "pending"
        );
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
