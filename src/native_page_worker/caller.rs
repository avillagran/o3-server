//! Actual provider caller projection for the whole native toolkit. Scope input
//! is never read from tool parameters, and credential IDs are not tab identity.
use crate::{mcp_access::Authority, native_mcp::Context, store::State};
use serde_json::{json, Value};
use std::sync::{Arc, Weak};
fn denied(reason: &str) -> Value {
    json!({"code":"capability_denied","message":"The native preview caller is unavailable.","reason":reason})
}

/// Retains the actual MCP credential session, as the original registry does.
/// Runtime provider-session rows and provider-thread IDs are separate identities;
/// neither their publication nor their transitions mint this caller's session.
pub struct Caller {
    state: Weak<State>,
    authority: Authority,
    thread: String,
    instance: String,
    provider_session: String,
    issued_at: i64,
}
impl Caller {
    pub(crate) fn belongs_to(&self, state: &Arc<State>) -> bool {
        Weak::ptr_eq(&self.state, &Arc::downgrade(state))
    }
    pub(crate) fn thread_id(&self) -> &str {
        &self.thread
    }
    pub(crate) fn agent_session_key(&self) -> Result<String, Value> {
        let scope = self.scope()?;
        Ok(format!(
            "{}\0{}\0{}",
            scope["environmentId"]
                .as_str()
                .ok_or_else(|| denied("environment_identity"))?,
            self.provider_session,
            self.thread
        ))
    }
    pub(crate) fn preview_current(&self) -> Result<(), Value> {
        let scope = self.scope()?;
        if scope["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "preview"))
        {
            Ok(())
        } else {
            Err(denied("preview_unavailable"))
        }
    }
    pub fn capture(context: &Context) -> Result<Self, Value> {
        let (state, authority) = context.page_authority();
        let current = state.upgrade().ok_or_else(|| denied("owner_closed"))?;
        let db = current.reader.lock().unwrap();
        let thread = authority.native_caller(&db, false)?;
        let tid = thread["id"]
            .as_str()
            .ok_or_else(|| denied("thread_identity"))?;
        let instance = thread["providerInstanceId"]
            .as_str()
            .ok_or_else(|| denied("provider_identity"))?;
        let session = context.id();
        Ok(Self {
            state,
            authority,
            thread: tid.into(),
            instance: instance.into(),
            provider_session: session.into(),
            issued_at: context.issued_at(),
        })
    }
    fn state(&self) -> Result<Arc<State>, Value> {
        self.state.upgrade().ok_or_else(|| denied("owner_closed"))
    }
    fn checked(&self, db: &rusqlite::Connection) -> Result<Value, Value> {
        let thread = self.authority.native_caller(db, false)?;
        if thread["id"] != self.thread || thread["providerInstanceId"] != self.instance {
            return Err(denied("provider_binding_changed"));
        }
        Ok(thread)
    }
    /// Metadata supplied separately to the original McpInvocationContext. Its
    /// Set and undefined client field are reconstructed inside the native VM.
    pub fn scope(&self) -> Result<Value, Value> {
        let state = self.state()?;
        let db = state.reader.lock().unwrap();
        self.checked(&db)?;
        drop(db);
        let capabilities = if self.authority.browser().is_some() {
            vec!["preview"]
        } else {
            vec![]
        };
        Ok(
            json!({"environmentId":state.environment_id,"capabilities":capabilities,"issuedAt":self.issued_at,"requestNamespace":self.authority.request_namespace(),"thread":{"threadId":self.thread,"providerSessionId":self.provider_session,"providerInstanceId":self.instance}}),
        )
    }
    /// Complete authoritative records for the original ProjectionStore. This
    /// does not use the bounded UI history window or infer background ownership
    /// from legacy task summaries. The native full codec runs before projection.
    pub fn thread_projection(&self, id: &str) -> Result<Value, Value> {
        if id != self.thread {
            return Err(denied("foreign_thread"));
        }
        let state = self.state()?;
        let db = state.reader.lock().unwrap();
        let snapshot = db
            .unchecked_transaction()
            .map_err(|_| denied("projection_unavailable"))?;
        let thread = self.checked(&snapshot)?;
        let mut projection = json!({"updatedAt":thread["updatedAt"],"thread":thread});
        for kind in [
            "runs",
            "attempts",
            "nodes",
            "subagents",
            "providerSessions",
            "providerThreads",
            "providerTurns",
            "runtimeRequests",
            "messages",
            "plans",
            "turnItems",
            "checkpointScopes",
            "checkpoints",
            "contextHandoffs",
            "contextTransfers",
            "visibleTurnItems",
        ] {
            let mut statement = snapshot
                .prepare(
                    "SELECT body FROM records WHERE kind=?1 AND thread_id=?2 ORDER BY rowid ASC",
                )
                .map_err(|_| denied("projection_unavailable"))?;
            let rows = statement
                .query_map(rusqlite::params![kind, id], |row| row.get::<_, String>(0))
                .map_err(|_| denied("projection_unavailable"))?;
            let mut records = Vec::new();
            for row in rows {
                records.push(
                    serde_json::from_str::<Value>(
                        &row.map_err(|_| denied("projection_unavailable"))?,
                    )
                    .map_err(|_| denied("projection_unavailable"))?,
                );
            }
            projection[kind] = json!(records);
        }
        Ok(projection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events;
    #[tokio::test]
    async fn actual_credential_reads_complete_owned_projection_and_fences_rebinding() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/native-preview-caller-projection-4314.json"
        ))
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "native-caller-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"enableAgentBrowserAccess":false}"#,
        )
        .unwrap();
        let state = Arc::new(State::open(&root).unwrap());
        state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
        let mut extracted = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let input = &case["input"];
            {
                let db = state.db.lock().unwrap();
                db.execute("DELETE FROM records", []).unwrap();
                events::put(&db, "thread", "thread", "thread", &input["thread"]).unwrap();
                for (kind, values) in input.as_object().unwrap() {
                    if let Some(values) = values.as_array() {
                        for record in values {
                            events::put(
                                &db,
                                kind,
                                record["id"].as_str().unwrap(),
                                "thread",
                                record,
                            )
                            .unwrap();
                        }
                    }
                }
                events::put(
                    &db,
                    "thread",
                    "foreign",
                    "foreign",
                    &json!({"id":"foreign","providerInstanceId":"codex"}),
                )
                .unwrap();
            }
            let context = state
                .native_mcp
                .obtain(&state, "thread", "codex", &root, &Value::Null)
                .await
                .unwrap();
            let caller = Caller::capture(&context).unwrap();
            assert_eq!(
                caller.scope().unwrap()["thread"]["providerSessionId"],
                context.id()
            );
            assert_eq!(caller.scope().unwrap()["requestNamespace"], context.id());
            assert!(caller.thread_projection("foreign").is_err());
            let projection = caller.thread_projection("thread").unwrap();
            assert_eq!(
                &projection, input,
                "Authoritative records must not be replaced by shell summaries"
            );
            extracted.push(json!({"name":case["name"],"status":case["status"],"background":case["background"],"input":projection,"expectedShell":case["expectedShell"],"caller":case["caller"],"sourceScope":caller.scope().unwrap()}));
            // Runtime publication is not MCP credential identity. Missing,
            // placeholder and successor runtime bindings cannot retag it.
            for status in ["active", "prepared", "not_loaded"] {
                {
                    let db = state.db.lock().unwrap();
                    let mut binding =
                        events::get(&db, "providerThreads", "provider-thread").unwrap();
                    binding["id"] = json!("next-provider-thread");
                    binding["status"] = json!(status);
                    binding["providerSessionId"] = json!("runtime-successor-session");
                    events::put(
                        &db,
                        "providerThreads",
                        "next-provider-thread",
                        "thread",
                        &binding,
                    )
                    .unwrap();
                    let mut thread = events::get(&db, "thread", "thread").unwrap();
                    thread["activeProviderThreadId"] = json!("next-provider-thread");
                    events::put(&db, "thread", "thread", "thread", &thread).unwrap();
                }
                assert_eq!(
                    caller.scope().unwrap()["thread"]["providerSessionId"],
                    context.id()
                );
            }
            {
                let db = state.db.lock().unwrap();
                let mut thread = events::get(&db, "thread", "thread").unwrap();
                thread["activeProviderThreadId"] = Value::Null;
                events::put(&db, "thread", "thread", "thread", &thread).unwrap();
            }
            assert_eq!(
                caller.scope().unwrap()["thread"]["providerSessionId"],
                context.id()
            );
            for field in ["providerInstanceId", "providerGeneration"] {
                let previous = {
                    let db = state.db.lock().unwrap();
                    let mut thread = events::get(&db, "thread", "thread").unwrap();
                    let previous = thread[field].clone();
                    thread[field] = json!("foreign-owner");
                    events::put(&db, "thread", "thread", "thread", &thread).unwrap();
                    previous
                };
                assert!(caller.scope().is_err());
                {
                    let db = state.db.lock().unwrap();
                    let mut thread = events::get(&db, "thread", "thread").unwrap();
                    thread[field] = previous;
                    events::put(&db, "thread", "thread", "thread", &thread).unwrap();
                }
            }
            context.revoke();
            assert!(caller.scope().is_err());
            assert!(caller.thread_projection("thread").is_err());
            let replacement = state
                .native_mcp
                .obtain(&state, "thread", "codex", &root, &Value::Null)
                .await
                .unwrap();
            assert_ne!(replacement.id(), context.id());
            let replacement_caller = Caller::capture(&replacement).unwrap();
            assert_eq!(
                replacement_caller.scope().unwrap()["thread"]["providerSessionId"],
                replacement.id()
            );
            assert!(
                caller.scope().is_err(),
                "A replacement credential cannot retag the old caller"
            );
        }
        if let Ok(output) = std::env::var("O3_NATIVE_CALLER_PROJECTION_OUTPUT") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)
                .unwrap();
            file.write_all(
                serde_json::to_string(
                    &json!({"actualState":true,"actualProviderCredential":true,"cases":extracted}),
                )
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
        }
        // All fixture files are retained inside the private container.
    }
}
