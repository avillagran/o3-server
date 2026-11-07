//! Trusted MCP authority is passed separately from wire commands, and checked under the write lock.
use crate::{
    events,
    mcp_tools::failure,
    store::{now_ms, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
#[derive(Clone)]
pub(crate) struct Authority {
    pub session_id: String,
    expires_at: i64,
    requested_runtime: Option<String>,
    interaction_ceiling: String,
    native: Option<std::sync::Arc<crate::native_mcp::Scope>>,
}
impl Authority {
    pub fn from_claims(claims: &Value) -> Result<Self, Value> {
        Ok(Self {
            session_id: claims["sid"]
                .as_str()
                .ok_or_else(|| failure("capability_denied", "Missing MCP session."))?
                .to_owned(),
            expires_at: claims["exp"].as_i64().unwrap_or(0),
            requested_runtime: None,
            interaction_ceiling: "default".into(),
            native: None,
        })
    }
    pub(crate) fn from_native(scope: std::sync::Arc<crate::native_mcp::Scope>) -> Self {
        Self {
            session_id: scope.id.clone(),
            expires_at: i64::MAX,
            requested_runtime: None,
            interaction_ceiling: "default".into(),
            native: Some(scope),
        }
    }
    pub(crate) fn is_native(&self) -> bool {
        self.native.is_some()
    }
    pub(crate) fn native_revoked(&self) -> bool {
        self.native.as_ref().is_some_and(|s| s.revoked())
    }
    pub(crate) fn native_thread(&self) -> Option<&str> {
        self.native.as_ref().map(|s| s.thread.as_str())
    }
    pub(crate) fn request_namespace(&self) -> String {
        if self.is_native() {
            self.session_id.clone()
        } else {
            format!("client:{}", self.session_id)
        }
    }
    pub(crate) fn launch_context(
        &self,
        db: &Connection,
    ) -> Result<(Option<Value>, String, String), Value> {
        let runtime = self.session(db, true)?;
        let caller = self
            .native
            .as_ref()
            .map(|scope| scope.caller(db, true))
            .transpose()?;
        let interaction = caller
            .as_ref()
            .and_then(|t| t["interactionMode"].as_str())
            .unwrap_or(&self.interaction_ceiling)
            .to_owned();
        Ok((caller, runtime, interaction))
    }
    pub(crate) fn check_launch(&self, db: &Connection, p: &Value) -> Result<(), String> {
        let (_, ceiling, interaction) = self.launch_context(db).map_err(|e| e.to_string())?;
        if rank(p["runtimeMode"].as_str().unwrap_or(""))
            .is_none_or(|r| r > rank(&ceiling).unwrap_or(0))
        {
            return Err(failure(
                "runtime_mode_escalation_denied",
                "The launched thread would run above this caller's runtime ceiling.",
            )
            .to_string());
        }
        if !matches!(p["interactionMode"].as_str(), Some("plan" | "default"))
            || (interaction == "plan" && p["interactionMode"] != "plan")
        {
            return Err(failure(
                "interaction_mode_escalation_denied",
                "The launched thread would run above this caller's interaction ceiling.",
            )
            .to_string());
        }
        if let Some(tid) = p["threadId"]
            .as_str()
            .filter(|tid| events::get(db, "thread", tid).is_ok())
        {
            self.check(db, &json!({"type":"launch.replay","threadId":tid}))?;
        }
        Ok(())
    }
    pub(crate) fn browser(&self) -> Option<crate::agent_browser::Access> {
        self.native
            .as_ref()?
            .browser
            .clone()
            .filter(|b| b.available())
    }
    pub(crate) fn arguments(
        &self,
        state: &State,
        name: &str,
        mut p: Value,
    ) -> Result<Value, Value> {
        let db = state.reader.lock().unwrap();
        self.session(&db, false)?;
        if let Some(scope) = &self.native {
            let t = scope.caller(&db, false)?;
            if p.get("threadId").is_none()
                && (name.starts_with("t3_queue")
                    || name.starts_with("t3_pending_request")
                    || matches!(
                        name,
                        "t3_thread_organize"
                            | "t3_thread_configure"
                            | "t3_thread_configuration"
                            | "t3_thread_transfer"
                    ))
            {
                p["threadId"] = json!(scope.thread);
            }
            if p.get("projectId").is_none() && matches!(name, "t3_thread_list" | "t3_thread_search")
            {
                p["projectId"] = t["projectId"].clone();
            }
        }
        Ok(p)
    }
    pub(crate) fn browser_admission(&self, state: &State, write: bool) -> Result<(), Value> {
        let scope = self
            .native
            .as_ref()
            .ok_or_else(|| failure("capability_denied", "Native browser access is unavailable."))?;
        scope.caller(&state.reader.lock().unwrap(), write)?;
        Ok(())
    }
    pub(crate) fn environment(&self, state: &State) -> Result<(), Value> {
        self.check(
            &state.reader.lock().unwrap(),
            &json!({"type":"project.create"}),
        )
        .map_err(command_error)
    }
    pub fn narrowed(mut self, runtime: &str, interaction: &str) -> Self {
        self.requested_runtime = Some(runtime.to_owned());
        self.interaction_ceiling = interaction.to_owned();
        self
    }
    pub fn session(&self, db: &Connection, write: bool) -> Result<String, Value> {
        if let Some(scope) = &self.native {
            let t = scope.caller(db, write)?;
            return Ok(t["runtimeMode"]
                .as_str()
                .unwrap_or("approval-required")
                .to_owned());
        }
        if self.expires_at <= now_ms() {
            return Err(failure("capability_denied", "The MCP credential expired."));
        }
        let row: Option<(String,String,String,String)>=db.query_row("SELECT subject,method,scopes,client FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?",params![self.session_id,now_ms()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|failure("orchestration_error","Unable to verify MCP authority."))?;
        let Some((subject, method, scopes, client)) = row else {
            return Err(failure(
                "capability_denied",
                "The MCP credential is no longer valid.",
            ));
        };
        let has = |s: &str| scopes.split_whitespace().any(|v| v == s);
        if subject != "mcp-client"
            || method != "bearer-access-token"
            || !has("orchestration:read")
            || (write && !has("orchestration:operate"))
        {
            return Err(failure(
                "capability_denied",
                "This MCP credential cannot change the environment.",
            ));
        }
        let client: Value = serde_json::from_str(&client)
            .map_err(|_| failure("capability_denied", "Invalid MCP authority."))?;
        let ceiling = if !has("orchestration:operate") {
            "read-only"
        } else {
            client["runtimeModeCeiling"]
                .as_str()
                .unwrap_or("approval-required")
        };
        if write && rank(ceiling).is_none() {
            return Err(failure("capability_denied", "Invalid MCP runtime ceiling."));
        }
        if let Some(asked) = &self.requested_runtime {
            if rank(asked) < rank(ceiling) {
                return Ok(asked.clone());
            }
        }
        Ok(ceiling.to_owned())
    }
    pub fn check(&self, db: &Connection, command: &Value) -> Result<(), String> {
        self.check_inner(db, command).map_err(|e| e.to_string())
    }
    fn check_inner(&self, db: &Connection, c: &Value) -> Result<(), Value> {
        let ceiling = self.session(db, true)?;
        let interaction = if let Some(scope) = &self.native {
            scope.caller(db, true)?["interactionMode"]
                .as_str()
                .unwrap_or("plan")
                .to_owned()
        } else {
            self.interaction_ceiling.clone()
        };

        if c["type"]
            .as_str()
            .is_some_and(|s| s.starts_with("project."))
        {
            if ceiling != "full-access" || interaction != "default" {
                return Err(failure(
                    "capability_denied",
                    "Changing projects requires full-access mode.",
                ));
            }
            return Ok(());
        }
        if matches!(
            c["type"].as_str(),
            Some("thread.fork" | "thread.merge_back")
        ) {
            let mut source = c.clone();
            source["type"] = json!("transfer.authorize");
            source["threadId"] = c["sourceThreadId"].clone();
            self.check_inner(db, &source)?;
            if c["type"] == "thread.merge_back" {
                source["threadId"] = c["targetThreadId"].clone();
                self.check_inner(db, &source)?;
            }
            return Ok(());
        }
        let tid = c["threadId"].as_str().unwrap_or("");
        let t = events::get(db, "thread", tid)
            .ok()
            .filter(|t| t["deletedAt"].is_null())
            .ok_or_else(|| failure("thread_not_found", "The thread was not found."))?;
        let target_rank = rank(t["runtimeMode"].as_str().unwrap_or(""));
        if target_rank.is_none() || target_rank > rank(&ceiling) {
            return Err(failure(
                "runtime_mode_escalation_denied",
                "The target thread runs above this MCP client's runtime ceiling.",
            ));
        }
        if (interaction == "plan" && t["interactionMode"] != "plan")
            || !matches!(t["interactionMode"].as_str(), Some("default" | "plan"))
        {
            return Err(failure(
                "interaction_mode_escalation_denied",
                "The target thread has an unsupported interaction mode.",
            ));
        }
        if c["type"] == "message.dispatch" && !t["archivedAt"].is_null() {
            return Err(failure("invalid_request", "The target thread is archived."));
        }
        if c["type"] == "runtime-request.respond" {
            let request = events::get(db, "runtimeRequests", c["requestId"].as_str().unwrap_or(""))
                .map_err(|_| failure("invalid_request", "The pending question was not found."))?;
            let owner: String = db
                .query_row(
                    "SELECT thread_id FROM records WHERE kind='runtimeRequests' AND id=?",
                    [c["requestId"].as_str().unwrap_or("")],
                    |r| r.get(0),
                )
                .map_err(|_| failure("invalid_request", "The pending question was not found."))?;
            if owner != tid || request["kind"] != "user_input" {
                return Err(failure(
                    "capability_denied",
                    "This tool can answer user questions, but cannot approve permissions.",
                ));
            }
        }
        Ok(())
    }
}
pub(crate) fn rank(mode: &str) -> Option<u8> {
    match mode {
        "approval-required" => Some(0),
        "auto-accept-edits" => Some(1),
        "auto" => Some(2),
        "full-access" => Some(3),
        _ => None,
    }
}
/// Typed errors cross the existing command API without being confused with wire metadata.
pub(crate) fn command_error(message: String) -> Value {
    if let Some(disabled) = crate::provider_policy::disabled_error(&message) {
        return failure(
            "invalid_request",
            disabled["message"]
                .as_str()
                .unwrap_or("Provider instance disabled."),
        );
    }
    serde_json::from_str::<Value>(&message)
        .ok()
        .filter(|e| e["_tag"] == "OrchestratorMcpFailure")
        .unwrap_or_else(|| {
            failure(
                "orchestration_error",
                &message.chars().take(1000).collect::<String>(),
            )
        })
}
pub(crate) fn sent_reply(
    db: &Connection,
    c: &Value,
    rid: &str,
    delivery: &str,
) -> Result<Value, String> {
    let run = events::get(db, "runs", rid)?;
    let result = json!({"threadId":c["threadId"],"messageId":c["messageId"],"runId":rid,"status":run["status"],"delivery":delivery});
    db.execute(
        "UPDATE receipts SET result=? WHERE command_id=?",
        params![result.to_string(), c["commandId"].as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(result)
}

pub(crate) fn interrupt_target(
    db: &Connection,
    c: &Value,
) -> Result<(Value, Option<Value>), String> {
    let tid = c["threadId"].as_str().unwrap_or("");
    let supplied = c["runId"].as_str();
    let run = if let Some(id) = supplied {
        Some(events::get(db, "runs", id).map_err(|_| {
            failure("invalid_request", "The run was not found in this thread.").to_string()
        })?)
    } else {
        let body: Option<String>=db.query_row("SELECT body FROM records WHERE kind='runs' AND thread_id=? AND json_extract(body,'$.status') IN ('preparing','starting','running','waiting') ORDER BY rowid DESC LIMIT 1",[tid],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        body.map(|b| serde_json::from_str::<Value>(&b).map_err(|e| e.to_string()))
            .transpose()?
    };
    let Some(run) = run else {
        return Ok((
            c.clone(),
            Some(json!({"threadId":tid,"runId":null,"status":"no_active_run"})),
        ));
    };
    if run["threadId"] != tid {
        return Err(
            failure("invalid_request", "The run was not found in this thread.").to_string(),
        );
    }
    if matches!(
        run["status"].as_str(),
        Some("completed" | "failed" | "cancelled" | "interrupted" | "rolled_back")
    ) {
        return Ok((
            c.clone(),
            Some(json!({"threadId":tid,"runId":run["id"],"status":run["status"]})),
        ));
    }
    let mut resolved = c.clone();
    resolved["runId"] = run["id"].clone();
    Ok((resolved, None))
}

pub(crate) fn replay_reply(db: &Connection, c: &Value, mut result: Value) -> Result<Value, String> {
    if c["type"] == "run.interrupt" {
        if let Some(id) = result["runId"].as_str() {
            let run = events::get(db, "runs", id)?;
            if matches!(
                run["status"].as_str(),
                Some("completed" | "interrupted" | "failed" | "cancelled" | "rolled_back")
            ) {
                result["status"] = run["status"].clone();
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{random_id, State};
    use std::sync::Arc;
    #[test]
    fn stale_authority_is_rechecked_in_both_command_transactions_before_receipts() {
        let dir = std::env::temp_dir().join(format!("o3-mcp-authority-{}", random_id().unwrap()));
        let state = Arc::new(State::open(&dir).unwrap());
        events::dispatch(&state,&json!({"type":"thread.create","commandId":"create","threadId":"t","projectId":"p","title":"Test","modelSelection":{"instanceId":"scripted","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
        let token = state
            .issue_mcp_session("full-access", 60_000, "test")
            .unwrap();
        let claims = state.mcp_session(&token).unwrap();
        let authority = Authority::from_claims(&claims).unwrap();
        let pin = json!({"type":"thread.pin","commandId":"pin","threadId":"t"});
        events::dispatch_scoped(&state, &pin, Some(&authority)).unwrap();
        let before: i64 = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
            .unwrap();
        // Simulate a mode change after HTTP authentication but before dispatch admission.
        state.db.lock().unwrap().execute("UPDATE auth_sessions SET client=json_set(client,'$.runtimeModeCeiling','approval-required') WHERE session_id=?",[&authority.session_id]).unwrap();
        let error = events::dispatch_scoped(&state, &pin, Some(&authority)).unwrap_err();
        assert_eq!(
            command_error(error)["code"],
            "runtime_mode_escalation_denied"
        );
        // Queue commands use a separate dispatcher, but must make the same check.
        let cancel = json!({"type":"queued-run.cancel","commandId":"cancel","threadId":"t","runId":"unknown"});
        let error = events::dispatch_scoped(&state, &cancel, Some(&authority)).unwrap_err();
        assert_eq!(
            command_error(error)["code"],
            "runtime_mode_escalation_denied"
        );
        state.revoke_session(&authority.session_id).unwrap();
        let error = events::dispatch_scoped(&state, &pin, Some(&authority)).unwrap_err();
        assert_eq!(command_error(error)["code"], "capability_denied");
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM receipts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        drop(state);
        // Fixture trees are retained for private container teardown.
    }
}
