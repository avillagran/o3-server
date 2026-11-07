//! Provider-owned MCP credentials. Registry entries are hashes and weak references, never bearer rows.
use crate::{
    agent_browser,
    mcp_tools::failure,
    store::{now_ms, random_id, State},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc, Mutex, Weak,
    },
};
const IDLE_MS: i64 = 24 * 60 * 60 * 1000;
#[derive(Default)]
pub(crate) struct Registry {
    endpoint: Mutex<Option<String>>,
    entries: Mutex<Entries>,
}
#[derive(Default)]
struct Entries {
    tokens: HashMap<[u8; 32], Weak<Context>>,
    threads: HashMap<String, Weak<Context>>,
}
type Dispose = Arc<
    dyn Fn(bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;
/// Opaque lifetime owner held by the actual CLI process/session, not by HTTP requests.
pub struct Context {
    scope: Arc<Scope>,
    config: Value,
    browser: Option<agent_browser::Broker>,
    dispose: Mutex<Option<Dispose>>,
}
pub(crate) struct Scope {
    pub id: String,
    pub thread: String,
    pub instance: String,
    generation: String,
    state: Weak<State>,
    revoked: AtomicBool,
    touched: AtomicI64,
    pub browser: Option<agent_browser::Access>,
    process_check: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
}
impl Drop for Context {
    fn drop(&mut self) {
        self.revoke();
    }
}
impl Context {
    pub(crate) fn set_disposer(&self, dispose: Dispose) {
        *self.dispose.lock().unwrap() = Some(dispose);
    }
    pub(crate) fn revoke(&self) {
        self.scope.revoke();
    }
    pub(crate) fn set_process_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>) {
        *self.scope.process_check.lock().unwrap() = Some(check);
    }
    pub(crate) async fn refresh(
        &self,
        cwd: &std::path::Path,
        options: &Value,
    ) -> Result<Arc<Self>, String> {
        let state = self.scope.state.upgrade().ok_or("MCP owner closed")?;
        state
            .native_mcp
            .obtain(
                &state,
                &self.scope.thread,
                &self.scope.instance,
                cwd,
                options,
            )
            .await
    }

    pub(crate) fn config(&self) -> Value {
        self.config.clone()
    }
    pub(crate) fn thread_key(&self) -> String {
        format!("{}:{}", self.scope.thread, self.scope.generation)
    }
    pub(crate) fn id(&self) -> &str {
        &self.scope.id
    }
}
impl Registry {
    pub(crate) fn configured(&self) -> bool {
        self.endpoint.lock().unwrap().is_some()
    }
    pub(crate) fn listen(&self, address: std::net::SocketAddr) {
        let address = if address.ip().is_unspecified() {
            std::net::SocketAddr::new(
                if address.is_ipv4() {
                    std::net::Ipv4Addr::LOCALHOST.into()
                } else {
                    std::net::Ipv6Addr::LOCALHOST.into()
                },
                address.port(),
            )
        } else {
            address
        };
        *self.endpoint.lock().unwrap() = Some(format!("http://{address}/mcp"));
    }
    pub(crate) fn resolve(&self, token: &str) -> Option<Arc<Scope>> {
        if token.is_empty() || token.len() > 256 || token.contains('.') {
            return None;
        }
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let context = { self.entries.lock().unwrap().tokens.get(&hash).cloned() }?.upgrade()?;
        if !context.scope.valid() {
            return None;
        }
        context.scope.touched.store(now_ms(), Ordering::Relaxed);
        Some(context.scope.clone())
    }
    pub(crate) async fn dispose_thread(&self, thread: &str, revoke: bool) -> Result<bool, String> {
        let contexts: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .tokens
            .values()
            .filter_map(Weak::upgrade)
            .filter(|c| c.scope.thread == thread)
            .collect();
        let mut handled = false;
        for context in contexts {
            let dispose = context.dispose.lock().unwrap().clone();
            if let Some(dispose) = dispose {
                handled = true;
                dispose(revoke).await?;
            }
        }
        Ok(handled)
    }
    pub(crate) fn revoke_thread(&self, thread: &str) {
        let contexts: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .tokens
            .values()
            .filter_map(Weak::upgrade)
            .filter(|c| c.scope.thread == thread)
            .collect();
        for c in contexts {
            c.revoke();
        }
    }
    pub(crate) fn touch(&self, thread: &str) {
        let entries = self.entries.lock().unwrap();
        for context in entries.tokens.values().filter_map(Weak::upgrade) {
            if context.scope.thread == thread {
                context.scope.touched.store(now_ms(), Ordering::Relaxed);
            }
        }
    }
    pub(crate) async fn obtain(
        &self,
        state: &Arc<State>,
        thread: &str,
        instance: &str,
        cwd: &std::path::Path,
        options: &Value,
    ) -> Result<Arc<Context>, String> {
        let endpoint = self
            .endpoint
            .lock()
            .unwrap()
            .clone()
            .ok_or("Native MCP requires an active HTTP listener")?;
        let (generation, project) = {
            let db = state.db.lock().unwrap();
            let t = crate::events::get(&db, "thread", thread)?;
            (
                t["providerGeneration"].as_str().unwrap_or("").to_owned(),
                t["projectId"].as_str().unwrap_or("").to_owned(),
            )
        };
        let browser_enabled = agent_browser::enabled(state, &project);
        let settings = crate::settings::read(state)?;
        let key = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([
                    thread,
                    instance,
                    generation,
                    // A shared Codex process keeps its credential across workspace
                    // detach/resume. Exclusive CLI contexts still include cwd.
                    if instance == "codex"
                        || settings["providerInstances"]
                            [crate::provider_policy::configured_id(&settings, instance)]["driver"]
                            == "codex"
                    {
                        Value::Null
                    } else {
                        json!(cwd)
                    },
                    crate::selection_transition::option_key(options),
                    settings["providerInstances"]
                        [crate::provider_policy::configured_id(&settings, instance)]
                ]))
                .map_err(|e| e.to_string())?
            )
        );
        let old = {
            self.entries
                .lock()
                .unwrap()
                .threads
                .get(&key)
                .and_then(Weak::upgrade)
        };
        if let Some(context) = old.filter(|c| {
            !c.scope.revoked.load(Ordering::SeqCst) && c.browser.is_some() == browser_enabled
        }) {
            context.scope.touched.store(now_ms(), Ordering::Relaxed);
            if context.scope.valid() {
                return Ok(context);
            }
        }
        let browser = if browser_enabled {
            agent_browser::Broker::start(state.clone(), thread).await
        } else {
            None
        };
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let hash = Sha256::digest(token.as_bytes()).into();
        let id = format!("provider:{}", random_id()?);
        let scope = Arc::new(Scope {
            id,
            thread: thread.into(),
            instance: instance.into(),
            generation,
            state: Arc::downgrade(state),
            revoked: AtomicBool::new(false),
            touched: AtomicI64::new(now_ms()),
            browser: browser.as_ref().map(agent_browser::Broker::access),
            process_check: Mutex::new(None),
        });
        let context = Arc::new(Context {
            scope,
            dispose: Mutex::new(None),
            config: json!({"url":endpoint,"headers":{"Authorization":format!("Bearer {token}")}}),
            browser,
        });
        {
            let mut entries = self.entries.lock().unwrap();
            entries.tokens.retain(|_, v| v.strong_count() > 0);
            entries.threads.retain(|_, v| v.strong_count() > 0);
            if entries.tokens.len() >= 256 {
                return Err("Native MCP session capacity reached".into());
            }
            entries.tokens.insert(hash, Arc::downgrade(&context));
            entries.threads.insert(key, Arc::downgrade(&context));
        }
        Ok(context)
    }
}
impl Scope {
    pub(crate) fn revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }
    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        if let Some(state) = self.state.upgrade() {
            state.mcp.close_owner(&self.id);
        }
    }
    pub(crate) fn valid(&self) -> bool {
        if self.revoked.load(Ordering::SeqCst) {
            return false;
        }
        let check = self.process_check.lock().unwrap().clone();
        if check.is_some_and(|check| !check()) {
            self.revoke();
            return false;
        }

        !self.revoked.load(Ordering::SeqCst)
            && now_ms().saturating_sub(self.touched.load(Ordering::Relaxed)) < IDLE_MS
    }
    pub(crate) fn caller(&self, db: &rusqlite::Connection, write: bool) -> Result<Value, Value> {
        if !self.valid() {
            return Err(failure(
                "capability_denied",
                "The provider MCP credential is no longer valid.",
            ));
        }
        let t = crate::events::get(db, "thread", &self.thread)
            .ok()
            .filter(|t| t["deletedAt"].is_null())
            .ok_or_else(|| failure("thread_not_found", "The calling thread was not found."))?;
        if t["providerInstanceId"] != self.instance
            || t["providerGeneration"].as_str().unwrap_or("") != self.generation
        {
            return Err(failure(
                "parent_not_active",
                "The calling provider no longer owns this thread.",
            ));
        }
        if write {
            if !matches!(
                t["runtimeMode"].as_str(),
                Some("approval-required" | "auto-accept-edits" | "auto" | "full-access")
            ) || !matches!(t["interactionMode"].as_str(), Some("default" | "plan"))
            {
                return Err(failure(
                    "capability_denied",
                    "The calling thread has unsupported modes.",
                ));
            }

            let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind='runs' AND thread_id=?1 AND json_extract(body,'$.providerInstanceId')=?2 AND json_extract(body,'$.status') IN ('preparing','starting','running','waiting'))",rusqlite::params![self.thread,self.instance],|r|r.get(0)).map_err(|_|failure("orchestration_error","Unable to check provider liveness."))?;
            let background:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM records r WHERE kind IN ('subagents','backgroundTasks') AND thread_id=?1 AND (kind='backgroundTasks' OR NOT EXISTS(SELECT 1 FROM records b WHERE b.kind='backgroundTasks' AND b.thread_id=r.thread_id AND json_extract(b.body,'$.itemId')=r.id)) AND json_extract(body,'$.providerInstanceId')=?2 AND (kind='subagents' OR json_extract(body,'$.kind')='subagent') AND coalesce(json_extract(body,'$.status'),'pending') NOT IN ('completed','cancelled','failed','interrupted'))",rusqlite::params![self.thread,self.instance],|r|r.get(0)).map_err(|_|failure("orchestration_error","Unable to check provider background work."))?;
            if !t["archivedAt"].is_null() || !(active || background) {
                return Err(failure(
                    "parent_not_active",
                    "The calling provider no longer owns an active run or background subagent.",
                ));
            }
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{get, put},
        mcp_access::Authority,
    };
    fn fixture() -> (Arc<State>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("o3-native-mcp-{}", random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"enableAgentBrowserAccess":false}"#,
        )
        .unwrap();
        let state = Arc::new(State::open(&dir).unwrap());
        state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
        {
            let db = state.db.lock().unwrap();
            for id in ["parent", "target"] {
                put(&db, "thread", id, id, &json!({"id":id,"projectId":"p","providerInstanceId":"claudeAgent","runtimeMode":"full-access","interactionMode":"default","providerGeneration":"g"})).unwrap();
            }
            put(
                &db,
                "runs",
                "r",
                "parent",
                &json!({"id":"r","providerInstanceId":"claudeAgent","status":"running"}),
            )
            .unwrap();
        }
        (state, dir)
    }
    #[tokio::test]
    async fn credential_idle_refresh_and_owner_lifetime() {
        let (state, dir) = fixture();
        let context = state
            .native_mcp
            .obtain(&state, "parent", "claudeAgent", &dir, &Value::Null)
            .await
            .unwrap();
        let auth = context.config()["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .to_owned();
        let token = auth.strip_prefix("Bearer ").unwrap();
        let scope = state.native_mcp.resolve(token).unwrap();
        scope
            .touched
            .store(now_ms() - IDLE_MS - 1, Ordering::Relaxed);
        assert!(
            state.native_mcp.resolve(token).is_none(),
            "HTTP cannot revive expired credentials"
        );
        state.native_mcp.touch("parent");
        assert!(
            state.native_mcp.resolve(token).is_some(),
            "trusted provider activity can revive idle credentials"
        );
        let same = context.refresh(&dir, &Value::Null).await.unwrap();
        assert!(Arc::ptr_eq(&context, &same));
        drop(same);
        drop(context);
        assert!(
            !scope.valid(),
            "an in-flight request cannot extend the process owner lifetime"
        );
        assert!(state.native_mcp.resolve(token).is_none());
        drop(state);
        // Fixture trees are retained for private container teardown.
    }
    #[tokio::test]
    async fn current_modes_generation_and_background_ownership_are_authoritative() {
        let (state, dir) = fixture();
        let context = state
            .native_mcp
            .obtain(&state, "parent", "claudeAgent", &dir, &Value::Null)
            .await
            .unwrap();
        let authority = Authority::from_native(context.scope.clone());
        let db = state.db.lock().unwrap();
        let command = json!({"type":"thread.pin.set","threadId":"target","pinned":true});
        authority.check(&db, &command).unwrap();
        let mut parent = get(&db, "thread", "parent").unwrap();
        parent["runtimeMode"] = json!("approval-required");
        put(&db, "thread", "parent", "parent", &parent).unwrap();
        assert!(authority
            .check(&db, &command)
            .unwrap_err()
            .contains("runtime_mode_escalation_denied"));
        parent["runtimeMode"] = json!("full-access");
        parent["interactionMode"] = json!("plan");
        put(&db, "thread", "parent", "parent", &parent).unwrap();
        assert!(authority
            .check(&db, &command)
            .unwrap_err()
            .contains("interaction_mode_escalation_denied"));
        parent["interactionMode"] = json!("default");
        put(&db, "thread", "parent", "parent", &parent).unwrap();
        put(
            &db,
            "runs",
            "r",
            "parent",
            &json!({"id":"r","providerInstanceId":"claudeAgent","status":"completed"}),
        )
        .unwrap();
        assert_eq!(
            context.scope.caller(&db, true).unwrap_err()["code"],
            "parent_not_active"
        );
        context.scope.caller(&db, false).unwrap();
        for (kind, instance, allowed) in [
            ("command", "claudeAgent", false),
            ("subagent", "codex", false),
            ("subagent", "claudeAgent", true),
        ] {
            let owner = parent.clone();
            let task = format!("{kind}-{instance}");
            let run = format!("background-{task}");
            put(
                &db,
                "runs",
                &run,
                "parent",
                &json!({"id":run,"providerInstanceId":instance,"status":"completed"}),
            )
            .unwrap();
            let event_run = get(&db, "runs", &run).unwrap();
            crate::background::update(
                &db,
                "parent",
                &event_run,
                &owner,
                "2026-11-04T00:00:00Z",
                &task,
                kind,
                false,
                "running",
                Some("s"),
            )
            .unwrap();
            assert_eq!(context.scope.caller(&db, true).is_ok(), allowed);
            let event_run = get(&db, "runs", &run).unwrap();
            crate::background::update(
                &db,
                "parent",
                &event_run,
                &owner,
                "2026-11-04T00:00:01Z",
                &task,
                kind,
                true,
                "completed",
                Some("s"),
            )
            .unwrap();
        }
        // A completed canonical task must mask its stale compatibility record.
        put(
            &db,
            "subagents",
            "s",
            "parent",
            &json!({"id":"s","providerInstanceId":"claudeAgent","status":"running"}),
        )
        .unwrap();
        assert_eq!(
            context.scope.caller(&db, true).unwrap_err()["code"],
            "parent_not_active"
        );
        parent["providerGeneration"] = json!("new");
        put(&db, "thread", "parent", "parent", &parent).unwrap();
        assert_eq!(
            context.scope.caller(&db, false).unwrap_err()["code"],
            "parent_not_active"
        );
        drop(db);
        drop(context);
        drop(state);
        // Fixture trees are retained for private container teardown.
    }
    #[tokio::test]
    async fn process_exit_revokes_even_with_surviving_owner_references() {
        let (state, dir) = fixture();
        let context = state
            .native_mcp
            .obtain(&state, "parent", "claudeAgent", &dir, &Value::Null)
            .await
            .unwrap();
        let live = Arc::new(AtomicBool::new(true));
        let check = live.clone();
        context.set_process_check(Arc::new(move || check.load(Ordering::SeqCst)));
        assert!(context.scope.valid());
        live.store(false, Ordering::SeqCst);
        assert!(!context.scope.valid());
        live.store(true, Ordering::SeqCst);
        state.native_mcp.touch("parent");
        assert!(
            !context.scope.valid(),
            "revocation cannot be undone by activity"
        );
        let replacement = context.refresh(&dir, &Value::Null).await.unwrap();
        assert_ne!(replacement.id(), context.id());
        drop(replacement);
        drop(context);
        drop(state);
        // Fixture trees are retained for private container teardown.
    }
}
