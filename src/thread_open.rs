//! Atomic read-state/open-history commands. Original V8 computation runs outside SQLite.
use crate::{
    events,
    speech_worker::{NodeRuntime, SealedFile},
    store::State,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::MutexGuard};
const NODE_SHA256: &str = "0adde40d1b7dfa57c8bc83cbc319758c3ec6ebba21363bb2fa752ea6b267ad1b";
pub(crate) struct NativeVisitRuntime {
    pub node: std::sync::Arc<NodeRuntime>,
    pub timezone: Option<std::ffi::OsString>,
}
/// Application-owner startup only. Installed packages place the pinned runtime
/// next to the server; a service may select the same admitted executable explicitly.
pub fn install(state: &State, args: &[String]) -> Result<(), String> {
    let flags = args
        .iter()
        .enumerate()
        .filter(|(_, v)| v.as_str() == "--native-search-node")
        .collect::<Vec<_>>();
    if flags.len() > 1 {
        return Err("Duplicate --native-search-node".into());
    }
    let explicit = flags
        .first()
        .map(|(i, _)| {
            args.get(i + 1)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or("--native-search-node requires an absolute path")
        })
        .transpose()?;
    let source = match explicit {
        Some(path) => path,
        None => std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("Missing server directory")?
            .join("runtime/node-26.13.0"),
    };
    if !source.exists() {
        return if flags.is_empty() {
            Ok(())
        } else {
            Err("Native search runtime missing".into())
        };
    }
    let size = std::fs::symlink_metadata(&source)
        .map_err(|e| e.to_string())?
        .len();
    if size > 512 * 1024 * 1024 {
        return Err("Native search runtime oversized".into());
    }
    let file = SealedFile::admit(&source, NODE_SHA256, size).map_err(|e| e.to_string())?;
    let node = NodeRuntime::capture_or_reuse(file, &source).map_err(|e| e.to_string())?;
    state
        .thread_open_node
        .set(NativeVisitRuntime {
            node,
            timezone: std::env::var_os("TZ"),
        })
        .map_err(|_| "Native search runtime already installed".to_owned())
}
// The upstream thread schema decodes an unreadable opens shape as null.
fn lenient_opens(value: &Value) -> Value {
    let finite = |v: &Value| v.as_f64().is_some_and(f64::is_finite);
    let valid = value.is_object()
        && finite(&value["openedAt"])
        && value["score"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0)
        && value["picks"].as_array().is_some_and(|picks| {
            picks.len() <= 3
                && picks.iter().all(|pick| {
                    pick["query"]
                        .as_str()
                        .is_some_and(|q| q.encode_utf16().count() <= 32)
                        && finite(&pick["at"])
                })
        });
    // Effect Struct decoding emits declared properties only, including nested picks.
    if valid {
        json!({"openedAt":value["openedAt"],"score":value["score"],"picks":value["picks"].as_array().unwrap().iter().map(|pick|json!({"query":pick["query"],"at":pick["at"]})).collect::<Vec<_>>()})
    } else {
        Value::Null
    }
}
pub(crate) struct Prepared<'a> {
    _gate: MutexGuard<'a, ()>,
    pub at: String,
    pub visited_at: String,
    pub moves_forward: bool,
    pub previous_visited: Value,
    pub previous: Value,
    pub generation: Value,
    pub opens: Value,
}
pub(crate) fn prepare<'a>(
    state: &'a State,
    command: &Value,
    authority: Option<&crate::mcp_access::Authority>,
) -> Result<Option<Prepared<'a>>, String> {
    if command["type"] != "thread.visit" {
        return Ok(None);
    }
    if let Some(opened) = command.get("opened") {
        if !opened.is_boolean() {
            return Err("Invalid opened".into());
        }
    }
    if let Some(query) = command.get("searchQuery") {
        if !query
            .as_str()
            .is_some_and(|q| q.encode_utf16().count() <= 200)
        {
            return Err("Invalid searchQuery".into());
        }
    }
    if !command["visitedAt"].is_string() {
        return Err("Invalid visitedAt: string required".into());
    }
    if serde_json::to_vec(command)
        .map_err(|e| e.to_string())?
        .len()
        > 8 * 1024 * 1024
    {
        return Err("Oversized visit command".into());
    }
    let gate = state
        .thread_open_gate
        .lock()
        .map_err(|_| "Thread visit owner poisoned")?;
    let (previous, generation, previous_visited) = {
        let db = state.db.lock().map_err(|_| "Database owner poisoned")?;
        if let Some(authority) = authority {
            authority.check(&db, command)?;
        }
        crate::managed_policy::check_command(state, &db, command)?;
        let receipt: Option<String> = db
            .query_row(
                "SELECT result FROM receipts WHERE command_id=?",
                [command["commandId"].as_str().ok_or("commandId required")?],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if receipt.is_some() {
            return Ok(None);
        }
        let thread = events::get(
            &db,
            "thread",
            command["threadId"].as_str().ok_or("threadId required")?,
        )
        .map_err(|error| format!("Thread visit projection unavailable: {error}"))?;
        if !thread["deletedAt"].is_null() {
            return Err("Thread is deleted".into());
        }
        (
            thread["opens"].clone(),
            thread["createdAt"].clone(),
            thread["lastVisitedAt"].clone(),
        )
    };
    let runtime = state
        .thread_open_node
        .get()
        .ok_or("native_search_runtime_unavailable")?;
    let mut input = json!({"previousVisitedAt":previous_visited,"visitedAt":command["visitedAt"],"opened":command["opened"]==true,"previousOpens":lenient_opens(&previous)});
    if let Some(query) = command.get("searchQuery") {
        input["searchQuery"] = query.clone();
    }
    let result = runtime
        .node
        .prepare_thread_visit(&state.dir, input, runtime.timezone.clone())
        .map_err(|e| e.to_string())?;
    match result["kind"].as_str() {
        Some("invalidPrevious") => {
            return Err("Thread visit projection timestamp is invalid".into())
        }
        Some("invalidTimestamp") => {
            return Err(format!(
                "Thread {} visit time {} is not a valid timestamp.",
                command["threadId"].as_str().unwrap_or(""),
                command["visitedAt"].as_str().unwrap_or("")
            ))
        }
        Some("prepared") => {}
        _ => return Err("Invalid native visit result".into()),
    }
    let at = result["occurredAt"]
        .as_str()
        .ok_or("Missing native visit clock")?
        .to_owned();
    let visited_at = result["visitedAt"]
        .as_str()
        .ok_or("Missing native visit timestamp")?
        .to_owned();
    let moves_forward = result["movesForward"]
        .as_bool()
        .ok_or("Missing native visit comparison")?;
    let opens = result["opens"].clone();
    Ok(Some(Prepared {
        _gate: gate,
        at,
        visited_at,
        moves_forward,
        previous_visited,
        previous,
        generation,
        opens,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_struct_admission_strips_extra_fields_and_rejects_invalid_picks() {
        let value =
            json!({"openedAt":1,"score":0,"picks":[{"query":"","at":2,"extra":true}],"extra":true});
        assert_eq!(
            lenient_opens(&value),
            json!({"openedAt":1,"score":0,"picks":[{"query":"","at":2}]})
        );
        let mut invalid = value;
        invalid["picks"][0]["query"] = json!("😀".repeat(17));
        assert_eq!(lenient_opens(&invalid), Value::Null);
    }
}
