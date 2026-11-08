//! Pure state transitions from the pinned native preview manager; no browser or filesystem effects.
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Lookup { thread_id: String, tab_id: String },
    ControlRequired { tab_id: String },
    InvalidUrl(Value),
}
impl Error {
    pub fn value(&self) -> Value {
        match self {
            Self::Lookup { thread_id, tab_id } => {
                json!({"_tag":"PreviewSessionLookupError","threadId":thread_id,"tabId":tab_id,"message":format!("Unknown preview session: thread={thread_id}, tab={tab_id}")})
            }
            Self::ControlRequired { tab_id } => {
                json!({"_tag":"PreviewControlRequiredError","tabId":tab_id,"message":"Take control of the server browser and use its viewer controls."})
            }
            Self::InvalidUrl(value) => value.clone(),
        }
    }
}
#[derive(Clone)]
struct Session {
    key: String,
    thread: String,
    tab: String,
    snapshot: Value,
}
/// Callers serialize access and deliver drained, already revisioned events in order.
pub struct Kernel {
    epoch: String,
    revision: u64,
    sessions: Vec<Session>,
    events: Vec<Value>,
}
fn key(thread: &str, tab: &str) -> String {
    format!("{thread}\0{tab}")
}
fn string<'a>(input: &'a Value, name: &str) -> &'a str {
    input[name].as_str().unwrap_or("")
}
fn copy(source: &Value, destination: &mut Value, fields: &[&str]) {
    for field in fields {
        if let Some(value) = source.get(field) {
            destination[*field] = value.clone();
        }
    }
}
impl Kernel {
    pub fn new(server_epoch: String) -> Self {
        Self {
            epoch: server_epoch,
            revision: 0,
            sessions: Vec::new(),
            events: Vec::new(),
        }
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn get(&self, thread: &str, tab: &str) -> Option<&Value> {
        self.sessions
            .iter()
            .find(|session| session.key == key(thread, tab))
            .map(|session| &session.snapshot)
    }
    pub fn all(&self) -> Vec<Value> {
        self.sessions
            .iter()
            .map(|session| session.snapshot.clone())
            .collect()
    }
    pub fn drain_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
    fn publish(&mut self, mut event: Value) {
        self.revision += 1;
        event["revision"] = json!(self.revision);
        event["serverEpoch"] = json!(self.epoch);
        self.events.push(event);
    }
    fn index(&self, input: &Value) -> Result<usize, Error> {
        let thread = string(input, "threadId");
        let tab = string(input, "tabId");
        self.sessions
            .iter()
            .position(|session| session.key == key(thread, tab))
            .ok_or_else(|| Error::Lookup {
                thread_id: thread.into(),
                tab_id: tab.into(),
            })
    }
    /// The caller allocates the base tab ID before invoking this method, even for invalid URLs.
    /// A callback panic leaves both state and event sequence unchanged.
    pub fn open(
        &mut self,
        input: &Value,
        base_tab_id: &str,
        mut clock: impl FnMut() -> String,
        before_publish: impl FnOnce(&Value),
    ) -> Result<Value, Error> {
        let thread = string(input, "threadId");
        let server = input["runtime"] == "server";
        let tab = if server {
            format!("{base_tab_id}_{}", self.epoch)
        } else {
            base_tab_id.into()
        };
        let at = clock();
        let navigation = match input["url"].as_str().filter(|url| !url.is_empty()) {
            Some(raw) => {
                json!({"_tag":"Loading","url":crate::preview::normalize_url(raw).map_err(Error::InvalidUrl)?,"title":""})
            }
            None => json!({"_tag":"Idle"}),
        };
        let mut snapshot = json!({"threadId":thread,"tabId":tab,"navStatus":navigation,"canGoBack":false,"canGoForward":false,"viewport":input.get("viewport").filter(|value|!value.is_null()).cloned().unwrap_or_else(||json!({"_tag":"fill"})),"updatedAt":at});
        copy(input, &mut snapshot, &["profileId", "runtime", "reveal"]);
        if server {
            copy(input, &mut snapshot, &["automationOwner"]);
        }
        before_publish(&snapshot);
        let session = Session {
            key: key(thread, &tab),
            thread: thread.into(),
            tab: tab.clone(),
            snapshot: snapshot.clone(),
        };
        if let Some(index) = self
            .sessions
            .iter()
            .position(|entry| entry.key == session.key)
        {
            self.sessions[index] = session;
        } else {
            self.sessions.push(session);
        }
        self.publish(json!({"type":"opened","threadId":thread,"tabId":tab,"createdAt":at,"snapshot":snapshot}));
        Ok(snapshot)
    }
    pub fn list(&self, thread: &str) -> Value {
        let mut sessions = self
            .sessions
            .iter()
            .filter(|session| session.thread == thread)
            .map(|session| session.snapshot.clone())
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| string(left, "updatedAt").cmp(string(right, "updatedAt")));
        json!({"sessions":sessions,"serverEpoch":self.epoch,"revision":self.revision})
    }
    pub fn report_status(
        &mut self,
        input: &Value,
        mut clock: impl FnMut() -> String,
    ) -> Result<(), Error> {
        let index = self.index(input)?;
        if self.sessions[index].snapshot["runtime"] == "server" && input["serverControlled"] != true
        {
            return Err(Error::ControlRequired {
                tab_id: string(input, "tabId").into(),
            });
        }
        let mut snapshot = self.sessions[index].snapshot.clone();
        let at = clock();
        copy(
            input,
            &mut snapshot,
            &["navStatus", "canGoBack", "canGoForward"],
        );
        if snapshot["viewport"].is_null() {
            snapshot["viewport"] = json!({"_tag":"fill"});
        }
        snapshot["updatedAt"] = json!(at);
        let session = &self.sessions[index];
        let mut event = if input["navStatus"]["_tag"] == "LoadFailed" {
            let mut event = json!({"type":"failed","threadId":session.thread,"tabId":session.tab,"createdAt":at});
            copy(
                &input["navStatus"],
                &mut event,
                &["url", "title", "code", "description", "download"],
            );
            event
        } else {
            json!({"type":"navigated","threadId":session.thread,"tabId":session.tab,"createdAt":at,"snapshot":snapshot})
        };
        self.sessions[index].snapshot = snapshot;
        self.publish(std::mem::take(&mut event));
        Ok(())
    }
    fn commit_snapshot(
        &mut self,
        index: usize,
        snapshot: Value,
        kind: &str,
        request: Option<Value>,
    ) {
        let session = &self.sessions[index];
        let mut event = json!({"type":kind,"threadId":session.thread,"tabId":session.tab,"createdAt":snapshot["updatedAt"],"snapshot":snapshot});
        if let Some(request) = request {
            event["request"] = request;
        }
        self.sessions[index].snapshot = snapshot;
        self.publish(event);
    }
    pub fn mark_headless(
        &mut self,
        input: &Value,
        mut clock: impl FnMut() -> String,
    ) -> Result<(), Error> {
        let index = self.index(input)?;
        if self.sessions[index].snapshot["headless"] == true {
            return Ok(());
        }
        let mut snapshot = self.sessions[index].snapshot.clone();
        snapshot["headless"] = json!(true);
        snapshot["updatedAt"] = json!(clock());
        self.commit_snapshot(index, snapshot, "resized", None);
        Ok(())
    }
    pub fn request_reveal(
        &mut self,
        input: &Value,
        mut clock: impl FnMut() -> String,
        uuid: impl FnOnce() -> String,
    ) -> Result<(), Error> {
        let index = self.index(input)?;
        let mut snapshot = self.sessions[index].snapshot.clone();
        snapshot["reveal"] = json!(true);
        snapshot["revealRequest"] = json!({"id":uuid(),"force":input["force"]});
        snapshot["updatedAt"] = json!(clock());
        self.commit_snapshot(index, snapshot, "navigated", None);
        Ok(())
    }
    pub fn resize(
        &mut self,
        input: &Value,
        mut clock: impl FnMut() -> String,
    ) -> Result<Value, Error> {
        let index = self.index(input)?;
        let mut snapshot = self.sessions[index].snapshot.clone();
        snapshot["viewport"] = input["viewport"].clone();
        snapshot["updatedAt"] = json!(clock());
        self.commit_snapshot(index, snapshot.clone(), "resized", None);
        Ok(snapshot)
    }
    pub fn adjust(
        &mut self,
        input: &Value,
        mut clock: impl FnMut() -> String,
    ) -> Result<Value, Error> {
        let index = self.index(input)?;
        let mut snapshot = self.sessions[index].snapshot.clone();
        copy(input, &mut snapshot, &["colorScheme", "zoomFactor"]);
        snapshot["updatedAt"] = json!(clock());
        let mut request = json!({});
        if input["hardReload"] == true {
            request["hardReload"] = json!(true);
        }
        if input
            .get("clear")
            .is_some_and(|value| !value.is_null() && value != false && value != "")
        {
            request["clear"] = input["clear"].clone();
        }
        let request = if request.as_object().unwrap().is_empty() {
            None
        } else {
            Some(request)
        };
        self.commit_snapshot(index, snapshot.clone(), "resized", request);
        Ok(snapshot)
    }
    pub fn close(&mut self, input: &Value, mut clock: impl FnMut() -> String) {
        let at = clock();
        let thread = string(input, "threadId");
        let tab = string(input, "tabId");
        let targets = self
            .sessions
            .iter()
            .filter(|session| {
                if tab.is_empty() {
                    session.thread == thread
                } else {
                    session.key == key(thread, tab)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        for target in targets {
            self.sessions.retain(|session| session.key != target.key);
            self.publish(
                json!({"type":"closed","threadId":target.thread,"tabId":target.tab,"createdAt":at}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base36(mut number: u64) -> String {
        let mut result = Vec::new();
        loop {
            result.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(number % 36) as usize] as char);
            number /= 36;
            if number == 0 {
                break;
            }
        }
        result.into_iter().rev().collect()
    }
    #[test]
    fn whole_native_manager_traces_match_every_result_event_callback_and_list() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/preview-manager-4288.json"))
                .unwrap();
        assert_eq!(fixture["traces"].as_array().unwrap().len(), 43);
        let mut observations = 0;
        for (trace_index, trace) in fixture["traces"].as_array().unwrap().iter().enumerate() {
            let mut kernel = Kernel::new(string(trace, "serverEpoch").into());
            let mut sequence = trace["initialSequence"].as_u64().unwrap();
            let mut uuid = 1_u64;
            let mut aliases = serde_json::Map::new();
            for (step_index, step) in trace["steps"].as_array().unwrap().iter().enumerate() {
                let operation = &step["input"];
                let mut input = operation["input"].clone();
                fn resolve(value: &mut Value, aliases: &serde_json::Map<String, Value>) {
                    match value {
                        Value::String(text) if text.starts_with('$') => {
                            *value = aliases.get(&text[1..]).cloned().unwrap_or(json!("missing"));
                        }
                        Value::Array(values) => {
                            for value in values {
                                resolve(value, aliases)
                            }
                        }
                        Value::Object(values) => {
                            for value in values.values_mut() {
                                resolve(value, aliases)
                            }
                        }
                        _ => {}
                    }
                }
                resolve(&mut input, &aliases);
                let at = string(step, "at");
                let clock = || at.to_string();
                let mut before = Vec::new();
                let result = match string(operation, "method") {
                    "open" => {
                        sequence += 1;
                        kernel.open(
                            &input,
                            &format!("tab_{}", base36(sequence)),
                            clock,
                            |snapshot| before.push(snapshot.clone()),
                        )
                    }
                    "reportStatus" => kernel.report_status(&input, clock).map(|()| Value::Null),
                    "markHeadless" => kernel.mark_headless(&input, clock).map(|()| Value::Null),
                    "requestReveal" => kernel
                        .request_reveal(&input, clock, || {
                            uuid += 1;
                            format!("00000000-0000-4000-8000-{uuid:012}")
                        })
                        .map(|()| Value::Null),
                    "resize" => kernel.resize(&input, clock),
                    "adjust" => kernel.adjust(&input, clock),
                    "close" => {
                        kernel.close(&input, clock);
                        Ok(Value::Null)
                    }
                    _ => panic!("unknown native operation"),
                };
                let mut actual = json!({"events":kernel.drain_events(),"before":before,"lists":{"thread":kernel.list("thread"),"other":kernel.list("other")}});
                match result {
                    Ok(result) => {
                        if let Some(alias) = operation["as"].as_str() {
                            aliases.insert(alias.into(), result["tabId"].clone());
                        }
                        actual["result"] = result;
                    }
                    Err(error) => actual["error"] = error.value(),
                }
                assert_eq!(
                    actual, step["expected"],
                    "trace {trace_index}, step {step_index}"
                );
                observations += 1;
            }
        }
        assert!(observations > 500);
    }
    #[test]
    fn ordered_map_collision_callbacks_and_lazy_inputs_follow_native_commit_boundaries() {
        crate::test_sandbox::assert_isolated();
        let mut kernel = Kernel::new("epoch".into());
        let clock = || "2026-11-06T00:00:00.000Z".into();
        kernel
            .open(&json!({"threadId":"a\u{0}b"}), "c", clock, |_| {})
            .unwrap();
        kernel
            .open(&json!({"threadId":"a"}), "b\u{0}c", clock, |_| {})
            .unwrap();
        assert_eq!(kernel.all().len(), 1);
        assert_eq!(kernel.list("a")["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(kernel.get("a\0b", "c").unwrap()["threadId"], "a");
        let revision = kernel.revision();
        let all = kernel.all();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            kernel.open(&json!({"threadId":"new"}), "new", clock, |_| {
                panic!("synthetic callback failure")
            })
        }));
        assert!(result.is_err());
        assert_eq!(kernel.revision(), revision);
        assert_eq!(kernel.all(), all);
        let missing = json!({"threadId":"missing","tabId":"missing","force":true});
        assert!(kernel
            .request_reveal(
                &missing,
                || panic!("missing lookup must not read clock"),
                || panic!("missing lookup must not allocate UUID")
            )
            .is_err());
        kernel
            .mark_headless(&json!({"threadId":"a","tabId":"b\u{0}c"}), clock)
            .unwrap();
        kernel
            .mark_headless(&json!({"threadId":"a","tabId":"b\u{0}c"}), || {
                panic!("sticky headless must not read clock")
            })
            .unwrap();
    }
}
