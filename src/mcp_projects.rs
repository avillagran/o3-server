//! Project and workspace MCP contracts, adapted from nightly's MIT toolkits.
use crate::{
    events,
    mcp_access::{command_error, Authority},
    mcp_tools::{definition, failure},
    store::{random_id, State},
};
use serde_json::{json, Value};
use std::sync::Arc;
type Reply = Result<Value, Value>;
fn string() -> Value {
    json!({"type":"string","minLength":1})
}
fn nullable(schema: Value) -> Value {
    json!({"anyOf":[schema,{"type":"null"}]})
}
fn write(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut tool = definition(name, description, properties, required);
    tool["annotations"] = json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":name=="t3_project_clone"});
    tool
}
pub(crate) fn catalog() -> Vec<Value> {
    let model = json!({"type":"object","properties":{"instanceId":string(),"model":string(),"options":{"type":"array","items":{"type":"object","properties":{"id":string(),"value":{"anyOf":[string(),{"type":"boolean"}]}},"required":["id","value"]}}},"required":["instanceId","model"]});
    let script = json!({"type":"object","properties":{"id":string(),"name":string(),"command":string(),"icon":{"type":"string","enum":["play","test","lint","configure","build","debug"]},"runOnWorktreeCreate":{"type":"boolean"},"runOnSettle":{"type":"boolean"},"async":{"type":"boolean"},"previewUrl":string(),"autoOpenPreview":{"type":"boolean"}},"required":["id","name","command","icon","runOnWorktreeCreate"]});
    let colors = json!({"type":"string","enum":["gray","red","orange","amber","yellow","lime","green","emerald","teal","cyan","sky","blue","indigo","violet","purple","fuchsia","pink","rose"]});
    let icon = json!({"anyOf":[{"type":"null"},{"type":"object","properties":{"kind":{"type":"string","enum":["lucide"]},"name":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[a-z0-9]+(?:-[a-z0-9]+)*$"},"color":colors},"required":["kind","name","color"]},{"type":"object","properties":{"kind":{"type":"string","enum":["emoji"]},"emoji":{"type":"string","minLength":1,"maxLength":32}},"required":["kind","emoji"]},{"type":"object","properties":{"kind":{"type":"string","enum":["monogram"]},"text":{"type":"string","minLength":1,"maxLength":32,"pattern":"^[\\p{L}\\p{N}][\\p{L}\\p{N}\\p{M}\\x{200c}\\x{200d}]*$"},"color":colors},"required":["kind","text","color"]}]});
    vec![
        write("t3_project_create","Register a folder, or omit workspaceRoot to create a named git project with README, icon and initial commit. Clone first with t3_project_clone if needed.",json!({"title":string(),"workspaceRoot":string(),"createWorkspaceRootIfMissing":{"type":"boolean"},"defaultModelSelection":nullable(model.clone()),"scripts":{"type":"array","items":script.clone()}}),&["title"]),
        write("t3_project_update","Update a project's settings; omitted fields are unchanged.",json!({"projectId":string(),"title":string(),"workspaceRoot":string(),"defaultModelSelection":nullable(model),"autoPull":{"type":"boolean"},"projectIcon":icon,"faviconPath":nullable(string()),"defaultThreadEnvMode":nullable(json!({"type":"string","enum":["local","worktree"]})),"scripts":{"type":"array","items":script},"pinned":{"type":"boolean"},"archived":{"type":"boolean"}}),&["projectId"]),
        write("t3_project_delete","Delete a project; nonempty projects require force=true. Its folder remains on disk.",json!({"projectId":string(),"force":{"type":"boolean"}}),&["projectId"]),
        write("t3_project_clone","Clone into a new or empty folder. Register afterwards with t3_project_create.",json!({"provider":string(),"repository":string(),"remoteUrl":string(),"destinationPath":string(),"protocol":{"type":"string","enum":["auto","ssh","https"]}}),&["destinationPath"]),
        definition("t3_worktree_list","List refs and checkout paths for a thread's workspace; omit threadId for the calling thread. Detached worktrees are omitted.",json!({"threadId":string(),"query":{"type":"string","minLength":1,"maxLength":256},"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":200},"refKind":{"type":"string","enum":["local","remote","all"]},"includeMatchingRemoteRefs":{"type":"boolean"}}),&[]),
        definition("t3_worktree_status","Read the calling thread's worktree path, branch, project root and startFromOrigin default.",json!({}),&[]),
    ]
}
pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "t3_project_create"
            | "t3_project_update"
            | "t3_project_delete"
            | "t3_project_clone"
            | "t3_worktree_list"
            | "t3_worktree_status"
    )
}
fn error(e: Value) -> Value {
    if e["_tag"] == "OrchestratorMcpFailure" {
        return e;
    }
    command_error(
        e["detail"]
            .as_str()
            .or(e["message"].as_str())
            .unwrap_or("The project operation failed.")
            .into(),
    )
}
fn normalize(schema: &Value, value: Value) -> Value {
    if let Some(choices) = schema["anyOf"].as_array() {
        if let Some(choice) = choices
            .iter()
            .find(|s| crate::mcp_tools::validate_schema(s, &value).is_ok())
        {
            return normalize(choice, value);
        }
    }
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter_map(|(k, v)| schema["properties"].get(&k).map(|s| (k, normalize(s, v))))
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|v| normalize(&schema["items"], v))
                .collect(),
        ),
        Value::String(s) => json!(s.trim()),
        other => other,
    }
}
pub(crate) async fn call(s: Arc<State>, authority: Authority, name: &str, p: Value) -> Reply {
    let schema = catalog()
        .into_iter()
        .find(|t| t["name"] == name)
        .ok_or_else(|| failure("invalid_request", "Unknown project tool."))?;
    let p = normalize(&schema["inputSchema"], p);
    if name.starts_with("t3_worktree_") {
        return worktree(&s, &authority, name, &p).await;
    }
    authority.environment(&s)?;
    if name == "t3_project_clone" {
        return crate::repositories::clone(&s, &p, Some(&authority))
            .await
            .map_err(error);
    }
    if name == "t3_project_create" && p.get("workspaceRoot").is_none() {
        if [
            "scripts",
            "createWorkspaceRootIfMissing",
            "defaultModelSelection",
        ]
        .iter()
        .any(|k| p.get(k).is_some())
        {
            return Err(failure("invalid_request","A project started from its title takes only a title; set scripts or defaultModelSelection afterwards with t3_project_update."));
        }
        let created =
            crate::misc::create_new_scoped(&s, &json!({"name":p["title"]}), Some(&authority))
                .await
                .map_err(error)?;
        let mut project = events::get(
            &s.reader.lock().unwrap(),
            "project",
            created["projectId"].as_str().unwrap_or(""),
        )
        .map_err(command_error)?;
        if let Some(e) = created.get("commitError") {
            project["commitError"] = e.clone();
        }
        return Ok(project);
    }
    let mut command = p;
    command["type"] = json!(name.trim_start_matches("t3_").replace('_', "."));
    let id = format!("mcp:{}", random_id().map_err(command_error)?);
    command["commandId"] = json!(id);
    if name == "t3_project_create" {
        command["projectId"] = json!(id);
        command
            .as_object_mut()
            .unwrap()
            .remove("defaultModelSelection"); // ProjectService.create records defaults only on update.
    }
    crate::projects::mutate_resolved_scoped(&s, &command, Some(&authority))
        .await
        .map_err(|e| {
            let structured = command_error(e.clone());
            if structured["code"] != "orchestration_error" {
                structured
            } else {
                failure("invalid_request", &e)
            }
        })
}
async fn worktree(s: &State, a: &Authority, name: &str, p: &Value) -> Reply {
    let tid = if name == "t3_worktree_status" {
        a.native_thread()
    } else {
        p["threadId"].as_str().or(a.native_thread())
    };
    let Some(tid) = tid else {
        return Err(if name == "t3_worktree_status" {
            json!({"_tag":"WorktreeMcpFailure","code":"thread_credential_required","message":"Worktree status needs an agent running inside a T3 thread."})
        } else {
            failure(
                "target_required",
                "Pass threadId: this MCP client is not running inside a T3 thread.",
            )
        });
    };
    let (t, project) = {
        let db = s.reader.lock().unwrap();
        a.session(&db, false)?;
        let t = events::get(&db, "thread", tid)
            .ok()
            .filter(|t| t["deletedAt"].is_null())
            .ok_or_else(|| failure("thread_not_found", "The thread was not found."))?;
        let project = events::get(&db, "project", t["projectId"].as_str().unwrap_or(""))
            .ok()
            .filter(|p| p["deletedAt"].is_null())
            .ok_or_else(|| if name=="t3_worktree_status"{json!({"_tag":"WorktreeMcpFailure","code":"project_not_found","message":"The calling thread's project was not found."})}else{failure("invalid_request", "The project was not found.")})?;
        (t, project)
    };
    if name == "t3_worktree_status" {
        let settings = crate::settings::read(s).map_err(command_error)?;
        let project_id = t["projectId"].as_str().unwrap_or("");
        let start = settings["projectSettingsOverrides"][project_id]
            .get("newWorktreesStartFromOrigin")
            .unwrap_or(&settings["newWorktreesStartFromOrigin"]);
        return Ok(
            json!({"attached":t["worktreePath"].is_string(),"worktreePath":t["worktreePath"],"branch":t["branch"],"projectWorkspaceRoot":project["workspaceRoot"],"defaultStartFromOrigin":start==true}),
        );
    }
    let mut refs = p.clone();
    refs["cwd"] = t["worktreePath"]
        .as_str()
        .filter(|path| std::path::Path::new(path).exists())
        .map(|s| json!(s))
        .unwrap_or(project["workspaceRoot"].clone());
    crate::vcs::list_refs(refs).await.map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_commit_rechecks_authority_before_receipt_replay() {
        let root =
            std::env::temp_dir().join(format!("o3-project-authority-{}", random_id().unwrap()));
        let state = State::open(&root).unwrap();
        events::mutate_project(&state,&json!({"type":"project.create","commandId":"create","projectId":"p","title":"Before","workspaceRoot":root})).unwrap();
        let token = state
            .issue_mcp_session("full-access", 60_000, "transaction")
            .unwrap();
        let claims = state.mcp_session(&token).unwrap();
        let a = Authority::from_claims(&claims).unwrap();
        let command = json!({"type":"project.update","commandId":"update","projectId":"p","title":"Accepted"});
        events::mutate_project_scoped(&state, &command, Some(&a)).unwrap();
        let plan = a.clone().narrowed("full-access", "plan");
        assert!(events::mutate_project_scoped(
            &state,
            &json!({"type":"project.update","commandId":"plan","projectId":"p","title":"Denied"}),
            Some(&plan)
        )
        .unwrap_err()
        .contains("capability_denied"));

        // Skip the outer HTTP/preflight layer to exercise transaction admission itself.
        state.db.lock().unwrap().execute("UPDATE auth_sessions SET client=json_set(client,'$.runtimeModeCeiling','approval-required') WHERE session_id=?",[&a.session_id]).unwrap();
        let error = events::mutate_project_scoped(&state, &command, Some(&a)).unwrap_err();
        assert!(error.contains("capability_denied"));
        assert_eq!(
            events::get(&state.db.lock().unwrap(), "project", "p").unwrap()["title"],
            "Accepted"
        );
        state.revoke_session(&a.session_id).unwrap();
        let error = events::mutate_project_scoped(
            &state,
            &json!({"type":"project.delete","commandId":"delete","projectId":"p"}),
            Some(&a),
        )
        .unwrap_err();
        assert!(error.contains("capability_denied"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
