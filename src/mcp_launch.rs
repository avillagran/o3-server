//! MCP project toolkit's top-level thread launch, using the shared durable service.
use crate::{
    events,
    mcp_access::{self, Authority},
    mcp_tools::{definition, failure},
    store::{now_ms, State},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
type Reply = Result<Value, Value>;
fn string() -> Value {
    json!({"type":"string","minLength":1})
}
pub(crate) fn normalize(mut p: Value) -> Value {
    fn trim(object: &mut Value, keys: &[&str]) {
        for key in keys {
            if let Some(value) = object.get_mut(*key) {
                if let Some(text) = value.as_str() {
                    *value = json!(text.trim());
                }
            }
        }
    }
    trim(
        &mut p,
        &[
            "projectId",
            "environmentId",
            "clientRequestId",
            "title",
            "runtimeMode",
            "interactionMode",
        ],
    );
    if let Some(provenance) = p.get_mut("delegatedFrom").filter(|v| v.is_object()) {
        trim(provenance, &["environmentId", "threadId"]);
        provenance
            .as_object_mut()
            .unwrap()
            .retain(|key, _| matches!(key.as_str(), "environmentId" | "threadId" | "title"));
    }
    if let Some(strategy) = p.get_mut("workspaceStrategy") {
        trim(strategy, &["type", "baseRef", "branch", "worktreePath"]);
        let kind = strategy["type"].as_str().unwrap_or("").to_owned();
        if let Some(fields) = strategy.as_object_mut() {
            fields.retain(|key, _| {
                key == "type"
                    || key == "branch"
                    || (kind == "worktree" && matches!(key.as_str(), "baseRef" | "startFromOrigin"))
                    || (kind == "existing_worktree" && key == "worktreePath")
            });
        }
    }
    if let Some(model) = p.get_mut("modelSelection").filter(|v| v.is_object()) {
        if model.get("instanceId").is_none() {
            if let Some(provider) = model.get("provider") {
                model["instanceId"] = provider.clone();
            }
        }
        trim(model, &["instanceId", "model"]);
        if let Some(options) = model["options"].as_object() {
            let values = options
                .iter()
                .filter_map(|(key, value)| {
                    let key = key.trim();
                    if key.is_empty() {
                        return None;
                    }
                    match value {
                        Value::String(text) if !text.trim().is_empty() => {
                            Some(json!({"id":key,"value":text.trim()}))
                        }
                        Value::Bool(value) => Some(json!({"id":key,"value":value})),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>();
            model["options"] = json!(values);
        }
        if let Some(options) = model.get_mut("options").and_then(Value::as_array_mut) {
            for option in options {
                trim(option, &["id", "value"]);
                if let Some(fields) = option.as_object_mut() {
                    fields.retain(|key, _| matches!(key.as_str(), "id" | "value"));
                }
            }
        }
        model
            .as_object_mut()
            .unwrap()
            .retain(|key, _| matches!(key.as_str(), "instanceId" | "model" | "options"));
    }
    if let Some(attachments) = p.get_mut("attachments").and_then(Value::as_array_mut) {
        for attachment in attachments {
            trim(attachment, &["id", "name", "mimeType"]);
            if attachment["type"] == "image" {
                attachment.as_object_mut().unwrap().remove("source");
            } else if attachment["source"].is_object() {
                let tag = attachment["source"]["_tag"].clone();
                attachment["source"] = json!({"_tag":tag});
            }
        }
    }
    p
}
pub(crate) fn catalog() -> Value {
    let attachment = crate::mcp_attachments::input_schema();
    let mut tool=definition("t3_thread_launch","Create one new top-level thread, bound to its workspace before its agent starts. Not delegation. Task in message. Omit projectId, model and modes to inherit the caller (outside a thread pass projectId or scratch). Modes cannot exceed the caller. After an error or lost reply retry the same clientRequestId; mention the result with its link. Default workspace is project root, not the caller's worktree.",json!({"projectId":string(),"scratch":{"type":"boolean"},"title":{"type":"string","minLength":1},"modelSelection":{"type":"object","properties":{"instanceId":string(),"model":string(),"options":{"type":"array","items":{"type":"object","properties":{"id":string(),"value":{"anyOf":[{"type":"string","minLength":1},{"type":"boolean"}]}},"required":["id","value"]}}},"required":["instanceId","model"]},"runtimeMode":{"type":"string","enum":["approval-required","auto-accept-edits","auto","full-access"]},"interactionMode":{"type":"string","enum":["default","plan"]},"workspaceStrategy":{"anyOf":[{"type":"object","properties":{"type":{"type":"string","enum":["root"]},"branch":string()},"required":["type"]},{"type":"object","properties":{"type":{"type":"string","enum":["worktree"]},"baseRef":string(),"branch":string(),"startFromOrigin":{"type":"boolean"}},"required":["type","baseRef"]},{"type":"object","properties":{"type":{"type":"string","enum":["existing_worktree"]},"worktreePath":string(),"branch":string()},"required":["type","worktreePath"]}]},"message":{"type":"string","maxLength":120000},"attachments":{"type":"array","maxItems":8,"items":attachment},"clientRequestId":{"type":"string","minLength":1,"maxLength":256},"delegatedFrom":{"type":"object","properties":{"environmentId":string(),"threadId":string(),"title":{"type":"string"}},"required":["environmentId","threadId","title"]}}),&["title"]);
    tool["annotations"] = json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true});
    tool
}
fn invalid(message: impl AsRef<str>) -> Value {
    failure("invalid_request", message.as_ref())
}
fn error(message: String) -> Value {
    mcp_access::command_error(message)
}
fn exhausted(provider: &Value, model: &str) -> bool {
    let windows: Vec<_> = provider["usageLimits"]["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| {
            w["usedPercent"].as_f64().unwrap_or(0.) >= 100.
                && w["resetsAt"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .is_none_or(|t| t.timestamp_millis() > now_ms())
        })
        .collect();
    match provider["driver"].as_str() {
        Some("cursor") => {
            let api = windows.iter().any(|w| w["id"] == "apiPercentUsed");
            api && (!["auto", "default", "grok", "composer"]
                .iter()
                .any(|p| model.starts_with(p))
                || windows.iter().any(|w| w["id"] == "autoPercentUsed"))
        }
        Some("opencode") => windows.iter().any(|w| {
            !w["id"].as_str().unwrap_or("").starts_with("go_") || model.starts_with("opencode-go/")
        }),
        Some("antigravity") => windows.iter().any(|w| {
            let family = ["gemini", "claude", "gpt"]
                .into_iter()
                .find(|p| model.starts_with(&format!("{p}-")));
            let pool = ["gemini", "claude", "gpt", "3p"]
                .into_iter()
                .find(|p| w["id"].as_str().unwrap_or("").starts_with(&format!("{p}-")));
            pool.is_none()
                || family.is_none()
                || pool == family
                || (pool == Some("3p") && matches!(family, Some("claude" | "gpt")))
        }),
        _ => !windows.is_empty(),
    }
}
fn default_model(settings: &Value, providers: &[Value], pid: &str) -> Option<Value> {
    let default = settings
        .get("defaultModelSelection")
        .filter(|v| v.is_object())?;
    let fallback = &settings["fallbackModelSelection"];
    let primary = providers
        .iter()
        .find(|p| p["instanceId"] == default["instanceId"]);
    let candidate = providers
        .iter()
        .find(|p| p["instanceId"] == fallback["instanceId"]);
    let enabled = settings["projectSettingsOverrides"][pid]["providerInstances"]
        [fallback["instanceId"].as_str().unwrap_or("")]["enabled"]
        .as_bool()
        .or(
            settings["providerInstances"][fallback["instanceId"].as_str().unwrap_or("")]["enabled"]
                .as_bool(),
        )
        .unwrap_or(true);
    if primary.is_some_and(|p| exhausted(p, default["model"].as_str().unwrap_or("")))
        && candidate.is_some_and(|p| {
            p["enabled"] == true
                && p["installed"] == true
                && p["auth"]["status"] != "unauthenticated"
                && p["availability"] != "unavailable"
                && !exhausted(p, fallback["model"].as_str().unwrap_or(""))
        })
        && enabled
    {
        Some(fallback.clone())
    } else {
        Some(default.clone())
    }
}
fn runtime(ceiling: &str, requested: Option<&str>, supported: &Value) -> Result<String, Value> {
    let mode = requested.unwrap_or(ceiling);
    if mcp_access::rank(mode) > mcp_access::rank(ceiling) {
        return Err(failure(
            "runtime_mode_escalation_denied",
            "The requested runtime mode is broader than this caller's.",
        ));
    }
    let modes: Vec<&str> = supported
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if modes.is_empty() || modes.contains(&mode) {
        return Ok(mode.into());
    }
    if requested.is_some() {
        return Err(invalid(format!(
            "Provider does not offer runtime mode {mode}. Use one of: {}.",
            modes.join(", ")
        )));
    }
    Ok(modes
        .into_iter()
        .filter(|m| mcp_access::rank(m) <= mcp_access::rank(mode))
        .max_by_key(|m| mcp_access::rank(m))
        .unwrap_or("approval-required")
        .into())
}
async fn git(root: &Path, args: &[&str]) -> Result<crate::proc::Out, Value> {
    crate::proc::git(root, args, crate::proc::Opts::READ)
        .await
        .map_err(|e| invalid(format!("Unable to inspect launch workspace: {e:?}")))
}
async fn preflight(project: &Value, strategy: &Value) -> Result<(), Value> {
    let root = Path::new(
        project["workspaceRoot"]
            .as_str()
            .ok_or_else(|| invalid("Project workspace is missing."))?,
    );
    if strategy["type"] == "existing_worktree" {
        let target = tokio::fs::canonicalize(strategy["worktreePath"].as_str().unwrap_or(""))
            .await
            .map_err(|_| invalid("worktreePath must be one of the project's git worktrees."))?;
        let out = git(root, &["worktree", "list", "--porcelain", "-z"]).await?;
        let mut matched = false;
        if out.ok() {
            for field in out.stdout.split(|b| *b == 0) {
                if let Some(path) = field.strip_prefix(b"worktree ") {
                    if let Ok(path) = std::str::from_utf8(path) {
                        if tokio::fs::canonicalize(path)
                            .await
                            .is_ok_and(|p| p == target)
                        {
                            matched = true;
                            break;
                        }
                    }
                }
            }
        }
        if !matched {
            return Err(invalid("worktreePath must be one of the project's git worktrees. t3_worktree_list shows them."));
        }
        let target_git = git(
            &target,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
        let root_git = git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
        if !target_git.ok() || !root_git.ok() || target_git.text().trim() != root_git.text().trim()
        {
            return Err(invalid(
                "worktreePath must be a live git worktree of this project.",
            ));
        }
    }
    if strategy["type"] == "worktree" {
        let base = strategy["baseRef"].as_str().unwrap_or("");
        if base.starts_with(':') {
            return Ok(());
        }
        let base = if base == "-" { "@{-1}" } else { base };
        if git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ],
        )
        .await?
        .ok()
        {
            return Ok(());
        }
        if strategy["startFromOrigin"] == true
            && crate::worktree_handoff::remote_target(root, base)
                .await
                .is_ok_and(|v| v.is_some())
        {
            return Ok(());
        }
        if git(
            root,
            &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
        )
        .await?
        .ok()
        {
            return Err(invalid(format!("Base ref {base:?} does not resolve to a commit in project {:?} ({}). No thread was created. Pass a branch, tag, or commit that exists there, or set startFromOrigin:true if it exists only on a remote.",project["title"].as_str().unwrap_or(""),root.display())));
        }
    }
    Ok(())
}
pub(crate) async fn call(s: Arc<State>, a: Authority, p: Value) -> Reply {
    let (caller, ceiling, interaction) = a.launch_context(&s.reader.lock().unwrap())?;
    let attachments = p.get("attachments").cloned().unwrap_or(json!([]));
    if attachments.as_array().into_iter().flatten().any(|v| {
        !crate::assets::parse_id(v["id"].as_str().unwrap_or(""))
            .is_some_and(|(segment, _)| segment == "pending")
    }) {
        return Err(invalid(
            "A new thread accepts only pending attachment uploads.",
        ));
    }
    let asked_runtime = p["runtimeMode"].as_str();
    let parent_runtime = runtime(&ceiling, asked_runtime, &Value::Null)?;
    let interaction = p["interactionMode"]
        .as_str()
        .unwrap_or(&interaction)
        .to_owned();
    if a.launch_context(&s.reader.lock().unwrap())?.2 == "plan" && interaction != "plan" {
        return Err(failure(
            "interaction_mode_escalation_denied",
            "The requested interaction mode is broader than this caller's.",
        ));
    }
    if p["scratch"] == true
        && (p.get("projectId").is_some() || p.get("workspaceStrategy").is_some())
    {
        return Err(invalid(
            "scratch:true picks its own project and folder; omit projectId and workspaceStrategy.",
        ));
    }
    let key = p["clientRequestId"].as_str().map(str::trim);
    let cid = if let Some(key) = key {
        format!(
            "mcp:launch:{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([a.request_namespace(), key]))
                    .map_err(|e| error(e.to_string()))?
            )
        )
    } else {
        format!("mcp:{}", crate::store::random_id().map_err(error)?)
    };
    let launched = events::get(&s.reader.lock().unwrap(), "thread", &cid)
        .ok()
        .filter(|t| t["deletedAt"].is_null());
    let pid = if p["scratch"] == true {
        if let Some(t) = &launched {
            t["projectId"].as_str().unwrap_or("").to_owned()
        } else {
            let state = s.clone();
            let auth = a.clone();
            tokio::task::spawn_blocking(move || {
                crate::managed_folders::ensure_scratch(&state, Some(&auth))
            })
            .await
            .map_err(|e| error(e.to_string()))?
            .map_err(error)?["projectId"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        }
    } else {
        p["projectId"]
            .as_str()
            .or_else(|| caller.as_ref().and_then(|t| t["projectId"].as_str()))
            .map(str::trim)
            .ok_or_else(|| {
                failure(
                    "target_required",
                    "Pass projectId or scratch:true: this client is not running inside a thread.",
                )
            })?
            .to_owned()
    };
    let project = events::get(&s.reader.lock().unwrap(), "project", &pid)
        .ok()
        .filter(|p| p["deletedAt"].is_null())
        .ok_or_else(|| invalid("The project was not found."))?;
    let strategy = p
        .get("workspaceStrategy")
        .cloned()
        .unwrap_or(json!({"type":"root"}));
    if launched.is_none() {
        preflight(&project, &strategy).await?;
    }
    let mut settings = crate::settings::read(&s).map_err(error)?;
    let stored = settings["projectSettingsOverrides"][&pid].clone();
    let project_model = stored.get("defaultModelSelection").or_else(|| {
        (settings["projectSettingsFolded"] != true)
            .then(|| {
                project
                    .get("defaultModelSelection")
                    .filter(|v| !v.is_null())
            })
            .flatten()
    });
    if let Some(model) = project_model {
        let instance = model["instanceId"].as_str().unwrap_or("");
        let enabled = stored["providerInstances"][instance]["enabled"]
            .as_bool()
            .or(settings["providerInstances"][instance]["enabled"].as_bool())
            .unwrap_or(true);
        if model.is_null() || enabled {
            settings["defaultModelSelection"] = model.clone();
        }
    }
    if let Some(mode) = stored.get("defaultRuntimeMode") {
        settings["defaultRuntimeMode"] = mode.clone();
    }
    let providers = s
        .provider
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.config())
        .unwrap_or_default();
    let selection=p.get("modelSelection").cloned().or_else(||launched.as_ref().map(|t|t["modelSelection"].clone())).or_else(||caller.as_ref().map(|t|t["modelSelection"].clone())).or_else(||default_model(&settings,&providers,&pid)).ok_or_else(||invalid("Pass modelSelection: the project has no default model. orchestrator_capabilities lists providers and models."))?;
    let mut mode = parent_runtime;
    if caller.is_none() && launched.is_none() && asked_runtime.is_none() {
        mode =
            runtime(&mode, settings["defaultRuntimeMode"].as_str(), &Value::Null).unwrap_or(mode);
    }
    if launched.is_none() {
        mode = runtime(
            &mode,
            asked_runtime,
            &providers
                .iter()
                .find(|v| v["instanceId"] == selection["instanceId"])
                .map(|v| v["supportedRuntimeModes"].clone())
                .unwrap_or_default(),
        )?;
    }
    let mut launch = json!({"commandId":cid,"threadId":cid,"projectId":pid,"title":p["title"].as_str().unwrap_or("").trim(),"modelSelection":selection,"runtimeMode":mode,"interactionMode":interaction,"workspaceStrategy":strategy,"creationSource":"mcp"});
    if let Some(provenance) = p.get("delegatedFrom") {
        launch["delegatedFrom"] = provenance.clone();
    }
    if p.get("message").is_some() || attachments.as_array().is_some_and(|v| !v.is_empty()) {
        launch["initialMessage"] = json!({"messageId":cid,"text":p["message"].as_str().unwrap_or(""),"attachments":attachments});
        if let Some(tid) = a.native_thread() {
            launch["initialMessage"]["senderThreadId"] = json!(tid);
        }
    }
    let state = s.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::thread_launch::launch_scoped(&state, &launch, Some(&a))
    })
    .await
    .map_err(|e| error(e.to_string()))?
    .map_err(error)?;
    let projection = &result["projection"];
    let thread = &projection["thread"];
    let run = projection["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["userMessageId"] == cid);
    Ok(
        json!({"threadId":thread["id"],"link":format!("<t3-thread://v2/{}/{}>",s.environment_id,cid),"projectId":thread["projectId"],"modelSelection":thread["modelSelection"],"runId":run.map(|r|r["id"].clone()).unwrap_or_default(),"status":run.map(|r|r["status"].clone()).unwrap_or_default()}),
    )
}
