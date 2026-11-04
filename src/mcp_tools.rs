//! MCP readers adapted from T3's MIT toolkit contracts; see docs/MCP.md for reference and gaps.
use crate::{
    events,
    store::{now_ms, State},
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
type Reply = Result<Value, Value>;
pub(crate) fn failure(code: &str, message: &str) -> Value {
    json!({"_tag":"OrchestratorMcpFailure","code":code,"message":message})
}
fn unavailable(_: impl std::fmt::Display) -> Value {
    failure("orchestration_error", "Unable to read server state.")
}
fn string() -> Value {
    json!({"type":"string","minLength":1})
}
fn integer(min: usize, max: usize) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}
pub(crate) fn definition(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> Value {
    let mut properties = properties;
    properties["environmentId"] = json!({"type":"string","minLength":1});
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required},"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
}
pub(crate) fn catalog() -> Vec<Value> {
    let mut tools=vec![
        definition("t3_environment_read","Read this server's identity and selected environment preferences.",json!({}),&[]),
        definition("t3_project_list","List registered projects with live offset pagination.",json!({"cursor":integer(0,usize::MAX),"limit":integer(1,100)}),&[]),
        definition("t3_project_read","Read a registered project, workspace and saved scripts.",json!({"projectId":string()}),&["projectId"]),
        definition("t3_thread_configuration","Read a thread's provider/model selection and modes. Outside a thread, pass threadId.",json!({"threadId":string()}),&[]),
        definition("t3_thread_search","Search active thread titles and content. Omit projectId to search every project. Results are bounded, without exhaustive-result guarantee.",json!({"query":{"type":"string","minLength":2,"maxLength":200},"limit":integer(1,50),"projectId":string(),"projectIds":{"type":"array","items":string()},"titlesOnly":{"type":"boolean"}}),&["query"]),
        definition("t3_thread_list","List project threads newest first, optionally filtering durable status, title, settled or snoozed state. Outside a thread, pass projectId.",json!({"projectId":string(),"allProjects":{"type":"boolean"},"statuses":{"type":"array","maxItems":10,"items":{"type":"string","enum":["idle","queued","preparing","starting","running","waiting","completed","failed","interrupted","cancelled","rolled_back"]}},"titleContains":{"type":"string","minLength":1,"maxLength":256},"settled":{"type":"boolean"},"snoozed":{"type":"boolean"},"includeSubagents":{"type":"boolean"},"cursor":integer(0,usize::MAX),"limit":integer(1,100)}),&[]),
        definition("t3_thread_read","Read durable thread state and paginated messages or activity. Use itemId and textOffset to read more of a truncated item.",json!({"threadId":string(),"itemId":string(),"textOffset":integer(0,usize::MAX),"view":{"type":"string","enum":["messages","activity"]},"afterPosition":integer(0,usize::MAX),"limit":integer(1,100),"runLimit":integer(1,50),"maxCharsPerItem":integer(1,50000)}),&["threadId"]),
    ];
    tools.extend(crate::mcp_mutations::catalog());
    tools
}
pub(crate) fn prepare_arguments(name: &str, mut args: Value) -> Value {
    if name == "t3_thread_read" && args.is_object() {
        for (key, max) in [
            ("limit", 100),
            ("runLimit", 50),
            ("maxCharsPerItem", 50_000),
        ] {
            if uint(&args[key]).is_some_and(|n| n > max) {
                args[key] = json!(max);
            }
        }
    }
    args
}
pub(crate) fn validate(name: &str, p: &Value) -> Result<(), String> {
    let tool = catalog()
        .into_iter()
        .find(|t| t["name"] == name)
        .ok_or("Unknown tool")?;
    validate_schema(&tool["inputSchema"], p).map_err(|e| format!("Invalid tool arguments: {e}"))
}
fn validate_schema(schema: &Value, p: &Value) -> Result<(), String> {
    let fail = || Err("value does not match input schema".to_owned());
    if let Some(choices) = schema["anyOf"].as_array() {
        return if choices.iter().any(|s| validate_schema(s, p).is_ok()) {
            Ok(())
        } else {
            fail()
        };
    }

    match schema["type"].as_str() {
        Some("object") => {
            let Some(object) = p.as_object() else {
                return fail();
            };
            for key in schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !object.contains_key(key) {
                    return Err(format!("{key} is required"));
                }
            }
            for (key, value) in object {
                if let Some(s) = schema["properties"].get(key) {
                    validate_schema(s, value)?;
                }
            }
        }
        Some("string") => {
            let Some(text) = p.as_str() else {
                return fail();
            };
            let n = text.encode_utf16().count() as u64;
            if schema["minLength"]
                .as_u64()
                .is_some_and(|min| n < min || text.trim().is_empty())
                || schema["maxLength"].as_u64().is_some_and(|max| n > max)
            {
                return fail();
            }
        }
        Some("integer") => {
            let Some(n) = uint(p) else { return fail() };
            if schema["minimum"].as_u64().is_some_and(|m| n < m)
                || schema["maximum"].as_u64().is_some_and(|m| n > m)
            {
                return fail();
            }
        }
        Some("null") => {
            if !p.is_null() {
                return fail();
            }
        }
        Some("number") => {
            if !p.is_number() {
                return fail();
            }
        }
        Some("boolean") => {
            if !p.is_boolean() {
                return fail();
            }
        }
        Some("array") => {
            let Some(a) = p.as_array() else { return fail() };
            if schema["maxItems"]
                .as_u64()
                .is_some_and(|n| a.len() as u64 > n)
            {
                return fail();
            }
            for v in a {
                validate_schema(&schema["items"], v)?;
            }
        }
        _ => {}
    }
    if schema["enum"].as_array().is_some_and(|a| !a.contains(p)) {
        return fail();
    }
    Ok(())
}
fn uint(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n < (u64::MAX as f64))
            .map(|n| n as u64)
    })
}
pub(crate) fn number(p: &Value, key: &str, default: usize) -> usize {
    uint(&p[key]).map(|n| n as usize).unwrap_or(default)
}

fn target<'a>(p: &'a Value, key: &str) -> Result<&'a str, Value> {
    p[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            failure(
                "invalid_request",
                &format!("{key} is required for an external MCP client."),
            )
        })
}
fn thread(db: &Connection, tid: &str) -> Reply {
    events::get(db, "thread", tid)
        .ok()
        .filter(|t| t["deletedAt"].is_null())
        .ok_or_else(|| failure("thread_not_found", "The thread was not found."))
}
fn rows(db: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Value>, Value> {
    let mut q = db.prepare(sql).map_err(unavailable)?;
    let rows = q
        .query_map(args, |r| r.get::<_, String>(0))
        .map_err(unavailable)?;
    rows.map(|r| {
        r.map_err(unavailable)
            .and_then(|s| serde_json::from_str(&s).map_err(unavailable))
    })
    .collect()
}
pub(crate) fn call(s: &State, name: &str, p: &Value) -> Reply {
    match name {
        "t3_environment_read" => {
            let mut d = crate::settings::descriptor(s);
            let v = crate::settings::read(s).map_err(unavailable)?;
            let mut prefs = json!({"defaultThreadEnvMode":null});
            for k in [
                "defaultThreadEnvMode",
                "newWorktreesStartFromOrigin",
                "enableProviderUpdateChecks",
                "sourceControlWritingStyle",
            ] {
                if let Some(v) = v.get(k) {
                    prefs[k] = v.clone();
                }
            }
            prefs["backgroundActivity"] = json!({"profile":v["backgroundActivity"]["profile"]});
            if let Some(text) = v["sourceControlWritingStyle"]["customInstructions"].as_str() {
                prefs["sourceControlWritingStyle"]["customInstructions"] =
                    json!(text.chars().take(4000).collect::<String>());
                prefs["sourceControlWritingStyle"]["truncated"] =
                    json!(text.chars().count() > 4000);
            }
            d.as_object_mut().unwrap().retain(|k, _| {
                ["environmentId", "label", "serverVersion", "platform"].contains(&k.as_str())
            });
            d["preferences"] = prefs;
            Ok(d)
        }
        "t3_project_list" => {
            let snapshot = crate::projects::snapshot(s).map_err(unavailable)?;
            let all = snapshot["projects"]
                .as_array()
                .ok_or_else(|| unavailable("projects"))?;
            let start = number(p, "cursor", 0).min(all.len());
            let end = start.saturating_add(number(p, "limit", 20)).min(all.len());
            Ok(
                json!({"projects":all[start..end],"nextCursor":if end<all.len(){json!(end)}else{Value::Null}}),
            )
        }
        "t3_project_read" => events::get(
            &s.reader.lock().unwrap(),
            "project",
            target(p, "projectId")?,
        )
        .ok()
        .filter(|p| p["deletedAt"].is_null())
        .ok_or_else(|| failure("invalid_request", "The project was not found.")),
        "t3_thread_search" => crate::threadq::search_threads(s, p).map_err(|e| {
            failure(
                "invalid_request",
                e["message"].as_str().unwrap_or("Search failed"),
            )
        }),
        "t3_thread_configuration" => {
            let t = thread(&s.reader.lock().unwrap(), target(p, "threadId")?)?;
            Ok(
                json!({"threadId":t["id"],"modelSelection":t["modelSelection"],"runtimeMode":t["runtimeMode"],"interactionMode":t["interactionMode"]}),
            )
        }
        "t3_thread_list" => thread_list(s, p),
        "t3_thread_read" => thread_read(s, p),
        _ => Err(failure("invalid_request", "Unknown tool")),
    }
}
fn list_item(t: &Value) -> Value {
    let settled = t["settledOverride"] == "settled";
    let woke_on_error = t["status"] == "failed"
        && (t["snoozedAt"].is_null()
            || t["latestRunCompletedAt"].as_str() > t["snoozedAt"].as_str());
    let snoozed = t["snoozedUntil"]
        .as_str()
        .is_some_and(|v| v > crate::store::iso(now_ms()).as_str())
        && t["pendingRuntimeRequest"].is_null()
        && !woke_on_error;
    let links=t["pullRequests"].as_array().into_iter().flatten().filter(|l|l["removedAt"].is_null()).map(|l|json!({"host":l["host"],"repository":l["repository"],"number":l["number"],"url":l["url"],"source":l["source"],"state":l["snapshot"]["state"]})).collect::<Vec<_>>();
    json!({"threadId":t["id"],"projectId":t["projectId"],"title":t["title"],"createdBy":t["createdBy"],"creationSource":t["creationSource"],"status":t["activityRunStatus"].as_str().or(t["status"].as_str()).unwrap_or("idle"),"latestRunId":t["latestRunId"],"providerInstanceId":t["modelSelection"]["instanceId"],"model":t["modelSelection"]["model"],"runtimeMode":t["runtimeMode"],"interactionMode":t["interactionMode"],"linkedPullRequest":t["linkedPullRequest"],"pullRequests":links,"settled":settled,"settledAt":if settled{t["settledAt"].clone()}else{Value::Null},"snoozed":snoozed,"snoozedUntil":if snoozed{t["snoozedUntil"].clone()}else{Value::Null},"parentThreadId":t["lineage"]["parentThreadId"],"relationshipToParent":t["lineage"]["relationshipToParent"],"itemCount":t["visibleItemCount"],"createdAt":t["createdAt"],"updatedAt":t["updatedAt"]})
}
fn thread_list(s: &State, p: &Value) -> Reply {
    let project = if p["allProjects"] == true {
        None
    } else {
        Some(target(p, "projectId")?)
    };
    let db = s.reader.lock().unwrap();
    let db = db.unchecked_transaction().map_err(unavailable)?;
    let cursor = number(p, "cursor", 0);
    let limit = number(p, "limit", 50);
    let mut total = 0;
    let mut page = Vec::new();
    let mut query=db.prepare("SELECT body FROM records WHERE kind='thread' AND (?1 IS NULL OR json_extract(body,'$.projectId')=?1) AND json_extract(body,'$.deletedAt') IS NULL AND json_extract(body,'$.archivedAt') IS NULL ORDER BY json_extract(body,'$.updatedAt') DESC,id DESC").map_err(unavailable)?;
    let mut records = query.query([project]).map_err(unavailable)?;
    while let Some(row) = records.next().map_err(unavailable)? {
        let t: Value = serde_json::from_str(
            row.get_ref(0)
                .map_err(unavailable)?
                .as_str()
                .map_err(unavailable)?,
        )
        .map_err(unavailable)?;
        if p["includeSubagents"] == false && t["lineage"]["relationshipToParent"] == "subagent" {
            continue;
        }
        if p["titleContains"].as_str().is_some_and(|q| {
            !t["title"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains(&q.trim().to_lowercase())
        }) {
            continue;
        }
        let mut shell = events::shell_thread(&db, &t).map_err(unavailable)?;
        shell["visibleItemCount"] = json!(db
            .query_row(
                "SELECT count(*) FROM records WHERE kind='visibleTurnItems' AND thread_id=?",
                [t["id"].as_str().unwrap_or("")],
                |r| r.get::<_, i64>(0)
            )
            .map_err(unavailable)?);
        let item = list_item(&shell);
        if p["statuses"]
            .as_array()
            .is_some_and(|a| !a.is_empty() && !a.contains(&item["status"]))
        {
            continue;
        }
        if p["settled"].is_boolean() && p["settled"] != item["settled"] {
            continue;
        }
        if p["snoozed"].is_boolean() && p["snoozed"] != item["snoozed"] {
            continue;
        }
        if total >= cursor && page.len() < limit {
            page.push(item);
        }
        total += 1;
    }
    let end = cursor.saturating_add(page.len());
    Ok(
        json!({"projectId":project,"currentThreadId":null,"threads":page,"nextCursor":if end<total{json!(end)}else{Value::Null},"total":total}),
    )
}

fn thread_read(s: &State, p: &Value) -> Reply {
    let tid = target(p, "threadId")?;
    let db = s.reader.lock().unwrap();
    let db = db.unchecked_transaction().map_err(unavailable)?;
    let t = thread(&db, tid)?;
    let shell = events::shell_thread(&db, &t).map_err(unavailable)?;
    let mut detail = list_item(&shell);
    for key in [
        "projectId",
        "activeRunId",
        "branch",
        "worktreePath",
        "titleRegeneration",
    ] {
        detail[key] = shell[key].clone();
    }
    detail["archived"] = json!(!t["archivedAt"].is_null());
    let count = |kind: &str| {
        db.query_row(
            "SELECT count(*) FROM records WHERE kind=? AND thread_id=?",
            params![kind, tid],
            |r| r.get::<_, i64>(0),
        )
        .map_err(unavailable)
    };
    detail["runCount"] = json!(count("runs")?);
    detail["itemCount"] = json!(count("visibleTurnItems")?);
    detail["pendingRequestCount"]=json!(db.query_row("SELECT count(*) FROM records WHERE kind='runtimeRequests' AND thread_id=? AND json_extract(body,'$.status')='pending'",[tid],|r|r.get::<_,i64>(0)).map_err(unavailable)?);
    let runs=rows(&db,"SELECT body FROM records WHERE kind='runs' AND thread_id=? ORDER BY json_extract(body,'$.ordinal') DESC LIMIT ?",params![tid,number(p,"runLimit",10)])?.into_iter().map(|r|json!({"runId":r["id"],"ordinal":r["ordinal"],"status":r["status"],"providerInstanceId":r["modelSelection"]["instanceId"],"model":r["modelSelection"]["model"],"requestedAt":r["requestedAt"],"startedAt":r["startedAt"],"completedAt":r["completedAt"]})).collect::<Vec<_>>();
    let limit = number(p, "limit", 50);
    let mut query=db.prepare("SELECT body FROM records WHERE kind='visibleTurnItems' AND thread_id=?1 AND json_extract(body,'$.position')>?2 AND (?3 OR json_extract(body,'$.item.type') IN ('user_message','assistant_message')) AND (?4 IS NULL OR json_extract(body,'$.sourceItemId')=?4) ORDER BY json_extract(body,'$.position'),rowid LIMIT ?5").map_err(unavailable)?;
    let mut page = query
        .query(params![
            tid,
            uint(&p["afterPosition"])
                .map(|n| n.min(i64::MAX as u64) as i64)
                .unwrap_or(-1),
            p["view"] == "activity",
            p["itemId"].as_str().map(str::trim),
            limit + 1
        ])
        .map_err(unavailable)?;
    let mut has_more = false;
    let mut items = Vec::new();
    let mut payload_bytes = 0;
    while let Some(raw) = page.next().map_err(unavailable)? {
        if items.len() == limit {
            has_more = true;
            break;
        }
        let row: Value = serde_json::from_str(
            raw.get_ref(0)
                .map_err(unavailable)?
                .as_str()
                .map_err(unavailable)?,
        )
        .map_err(unavailable)?;
        let item = &row["item"];
        let text = item_text(item);
        let offset = if p["itemId"].is_string() {
            number(p, "textOffset", 0)
        } else {
            0
        };
        let max = number(p, "maxCharsPerItem", 20_000);
        let sliced = text.as_deref().map(|t| text_page(t, offset, max));
        let truncated = sliced.as_ref().is_some_and(|(_, next)| next.is_some());
        let next_offset = sliced.as_ref().and_then(|(_, next)| *next);
        let message = if let Some(mid) = item["messageId"].as_str() {
            events::get(&db, "messages", mid).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let mut entry = json!({"position":row["position"],"visibility":row["visibility"],"sourceThreadId":row["sourceThreadId"],"itemId":row["sourceItemId"],"runId":item["runId"],"messageId":item["messageId"],"createdBy":message["createdBy"],"creationSource":message["creationSource"],"type":item["type"],"status":item["status"],"title":item["title"],"text":sliced.map(|(text,_)|Value::String(text)).unwrap_or(Value::Null),"textTruncated":truncated,"nextTextOffset":json!(next_offset),"updatedAt":item["updatedAt"]});
        if item["type"] == "subagent" {
            entry["subagentId"] = item["subagentId"].clone();
        }
        if let Some(sender) = message.get("senderThreadId") {
            entry["senderThreadId"] = sender.clone();
        }
        let size = entry.to_string().len();
        if !items.is_empty() && payload_bytes + size > 2 * 1024 * 1024 {
            has_more = true;
            break;
        }
        payload_bytes += size;
        items.push(entry);
    }
    Ok(
        json!({"thread":detail,"recentRuns":runs,"nextPosition":items.last().map(|i|i["position"].clone()),"items":items,"hasMore":has_more}),
    )
}
// Return complete Unicode scalar values, including when a UTF-16 page boundary hits
// a surrogate pair. The next cursor always points past the complete returned text.
pub(crate) fn text_page(text: &str, offset: usize, max: usize) -> (String, Option<usize>) {
    let mut units = 0;
    let mut start = None;
    let end = offset.saturating_add(max);
    for (byte, ch) in text.char_indices() {
        let after = units + ch.len_utf16();
        if start.is_none() && after > offset {
            start = Some(byte);
        }
        if let Some(start_byte) = start.filter(|_| after >= end) {
            let end_byte = byte + ch.len_utf8();
            return (
                text[start_byte..end_byte].to_owned(),
                (end_byte < text.len()).then_some(after),
            );
        }
        units = after;
    }
    (
        start.map(|s| text[s..].to_owned()).unwrap_or_default(),
        None,
    )
}

fn item_text(i: &Value) -> Option<String> {
    let string = |k: &str| i[k].as_str().map(str::to_owned);
    let join = |keys: &[&str]| {
        Some(
            keys.iter()
                .filter_map(|k| string(k))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };
    match i["type"].as_str()? {
        "user_message" | "assistant_message" | "reasoning" => string("text"),
        "notification" => join(&["summary", "detail"]),
        "proposed_plan" => string("markdown"),
        "todo_list" => {
            let mut lines = string("explanation").into_iter().collect::<Vec<_>>();
            lines.extend(i["steps"].as_array().into_iter().flatten().map(|step| {
                format!(
                    "[{}] {}",
                    step["status"].as_str().unwrap_or(""),
                    step["text"].as_str().unwrap_or("")
                )
            }));
            Some(lines.join("\n"))
        }
        "file_change" => {
            let mut lines = string("fileName").into_iter().collect::<Vec<_>>();
            if i.get("additions").is_some() || i.get("deletions").is_some() {
                lines.push(format!(
                    "+{} -{}",
                    i["additions"].as_u64().unwrap_or(0),
                    i["deletions"].as_u64().unwrap_or(0)
                ));
            }
            lines.extend(string("diffStr").or_else(|| string("newStr")));
            Some(lines.join("\n"))
        }
        "command_execution" => Some(format!(
            "$ {}{}",
            i["input"].as_str().unwrap_or(""),
            i["output"]
                .as_str()
                .map(|s| format!("\n{s}"))
                .unwrap_or_default()
        )),
        "tool" => join(&["input", "output"]),
        "user_input_request" => Some(i["questions"].to_string()),
        "run_interrupt_request" | "run_interrupt_result" | "system_notice" => string("message"),
        "approval_request" => string("prompt").or_else(|| string("requestKind")),
        "error" => i["failure"]["message"].as_str().map(str::to_owned),
        "compaction" => string("summary"),
        "subagent" => string("result")
            .or_else(|| string("progress"))
            .or_else(|| string("prompt")),
        "checkpoint" => Some(i["files"].to_string()),
        "dynamic_tool" => Some(
            json!({"toolName":i["toolName"],"input":i["input"],"output":i["output"]}).to_string(),
        ),
        "file_search" => Some(json!({"pattern":i["pattern"],"results":i["results"]}).to_string()),
        "web_search" => Some(json!({"patterns":i["patterns"],"results":i["results"]}).to_string()),
        "handoff" => string("summary").or_else(|| {
            Some(format!(
                "{} handoff to {}",
                i["strategy"].as_str().unwrap_or(""),
                i["toProviderInstanceId"].as_str().unwrap_or("")
            ))
        }),
        "fork" => Some(format!(
            "Forked to thread {}.",
            i["targetThreadId"].as_str().unwrap_or("")
        )),
        "thread_created" => Some(format!(
            "Created thread {} with {} ({}).",
            i["targetThreadId"].as_str().unwrap_or(""),
            i["targetProviderInstanceId"].as_str().unwrap_or(""),
            i["targetModel"].as_str().unwrap_or("")
        )),
        "secret_request" => Some(format!(
            "Asked the user for {}: {}.",
            i["label"].as_str().unwrap_or(""),
            i["secretStatus"].as_str().unwrap_or("")
        )),
        _ => None,
    }
}
