//! Narrow, captured-session exact Runtime.evaluate transport. No MCP driver or
//! browser-global command is selected by this owner-only API.
use super::*;
use crate::native_page_worker::{cdp::Packet, JavaScriptJson};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Closed,
    Deadline,
    Protocol,
    Busy,
}
pub(super) trait Observer: Send + Sync {
    fn completed(&self, packet: &Packet);
    fn cancelled(&self, outcome_unknown: bool);
}
pub(super) struct Request {
    pub method: String,
    pub observer: Option<std::sync::Arc<dyn Observer>>,
    pub params: JavaScriptJson,
    pub session: Option<String>,
    pub expires: tokio::time::Instant,
    pub reply: oneshot::Sender<Result<Packet, Fault>>,
    pub permit: write::Permit,
}
#[derive(Serialize)]
pub(super) struct Command<'a> {
    pub id: u64,
    pub method: &'a str,
    pub params: &'a JavaScriptJson,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session: Option<&'a str>,
}
pub(super) fn parameters(bytes: &[u8]) -> Result<JavaScriptJson, Fault> {
    if bytes.is_empty() || bytes.len() >= 4 * 1024 * 1024 {
        return Err(Fault::Protocol);
    }
    let fields: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).map_err(|_| Fault::Protocol)?;
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "expression" | "awaitPromise" | "returnByValue"
        )
    }) || !fields
        .get("expression")
        .is_some_and(|value| value.get().starts_with('"'))
    {
        return Err(Fault::Protocol);
    }
    for key in ["awaitPromise", "returnByValue"] {
        if let Some(value) = fields.get(key) {
            serde_json::from_str::<bool>(value.get()).map_err(|_| Fault::Protocol)?;
        }
    }
    JavaScriptJson::parse(
        std::str::from_utf8(bytes)
            .map_err(|_| Fault::Protocol)?
            .to_owned(),
    )
    .map_err(|_| Fault::Protocol)
}
/// Fixed Runtime.evaluate on a captured real page, authorized by that page's
/// actual callback admission. This is an owner library API, not an MCP route.
#[derive(Clone)]
pub struct CapturedEvaluation {
    actor: mpsc::WeakSender<Request>,
    session: String,
    done: tokio::sync::watch::Receiver<bool>,
    control: crate::preview_control::runtime::Control,
    wake: std::sync::Arc<tokio::sync::Notify>,
}
pub(super) struct CallbackAuthority {
    pub(super) control: crate::preview_control::runtime::Control,
    pub(super) context: crate::preview_control::runtime::RunContext,
}
impl write::Authority for CallbackAuthority {
    fn valid(&self) -> bool {
        self.control.owns_context(&self.context)
    }
    fn begin(&self) -> bool {
        self.control.authorize_context_write(&self.context)
    }
}
impl CapturedEvaluation {
    /// The real captured scheduler, never a caller-provided controller epoch.
    pub fn control(&self) -> crate::preview_control::runtime::Control {
        self.control.clone()
    }
    pub async fn evaluate(
        &self,
        context: &crate::preview_control::runtime::RunContext,
        params: &[u8],
        budget: Duration,
    ) -> Result<Packet, Value> {
        if !self.control.owns_context(context) {
            return Err(
                json!({"message":"Browser callback admission is unavailable","reason":"scope"}),
            );
        }
        self.evaluate_inner(context, params, budget)
            .await
            .map_err(fault_value)
    }
    async fn evaluate_inner(
        &self,
        context: &crate::preview_control::runtime::RunContext,
        params: &[u8],
        budget: Duration,
    ) -> Result<Packet, Fault> {
        if *self.done.borrow() {
            return Err(Fault::Closed);
        }
        if budget.is_zero() || budget > Duration::from_secs(121) {
            return Err(Fault::Deadline);
        }
        let params = parameters(params)?;
        let expires = tokio::time::Instant::now() + budget;
        let authority = std::sync::Arc::new(CallbackAuthority {
            control: self.control.clone(),
            context: context.clone(),
        });
        let permit =
            write::Permit::leased(expires, self.done.clone(), authority, self.wake.clone());
        let _cancel = permit.hold();
        let actor = self.actor.upgrade().ok_or(Fault::Closed)?;
        let (reply, result) = oneshot::channel();
        tokio::time::timeout_at(
            expires,
            actor.send(Request {
                method: "Runtime.evaluate".into(),
                observer: None,
                params,
                session: Some(self.session.clone()),
                expires,
                reply,
                permit,
            }),
        )
        .await
        .map_err(|_| Fault::Deadline)?
        .map_err(|_| Fault::Closed)?;
        tokio::time::timeout_at(expires, result)
            .await
            .map_err(|_| Fault::Deadline)?
            .map_err(|_| Fault::Closed)?
    }
}
fn fault_value(fault: Fault) -> Value {
    json!({"message":"Exact browser evaluation did not complete","reason":match fault{Fault::Closed=>"closed",Fault::Deadline=>"deadline",Fault::Protocol=>"protocol",Fault::Busy=>"busy"}})
}
impl Page {
    pub(super) async fn evaluate_exact(
        &self,
        params: &[u8],
        budget: Duration,
    ) -> Result<Packet, Fault> {
        self.live().map_err(|_| Fault::Closed)?;
        if budget.is_zero() || budget > Duration::from_secs(121) {
            return Err(Fault::Deadline);
        }
        let params = parameters(params)?;
        let session = self.tab.session.clone().ok_or(Fault::Closed)?;
        let expires = tokio::time::Instant::now() + budget;
        let permit = write::Permit::new(
            expires,
            self.tab.done.clone(),
            self.control().subscribe(),
            self.tab.exact_wake.clone(),
        );
        let _cancel = permit.hold();
        let (reply, result) = oneshot::channel();
        tokio::time::timeout_at(
            expires,
            self.tab.exact_tx.send(Request {
                method: "Runtime.evaluate".into(),
                observer: None,
                params,
                session: Some(session),
                expires,
                reply,
                permit,
            }),
        )
        .await
        .map_err(|_| Fault::Deadline)?
        .map_err(|_| Fault::Closed)?;
        let result = tokio::time::timeout_at(expires, result)
            .await
            .map_err(|_| Fault::Deadline)?
            .map_err(|_| Fault::Closed)??;
        if tokio::time::Instant::now() >= expires {
            return Err(Fault::Deadline);
        }
        self.live().map_err(|_| Fault::Closed)?;
        Ok(result)
    }
}
impl Browsers {
    /// Trusted owner-library entry point; authenticated MCP admission remains
    /// outside this API. It cannot select another target/context or root method.
    pub async fn evaluate_exact(
        &self,
        thread: &str,
        tab: &str,
        params: &[u8],
        budget: Duration,
    ) -> Result<Packet, Value> {
        self.page(thread, tab)?.evaluate_exact(params, budget).await.map_err(|fault| json!({"message":"Exact browser evaluation did not complete","reason":match fault { Fault::Closed=>"closed",Fault::Deadline=>"deadline",Fault::Protocol=>"protocol",Fault::Busy=>"busy" }}))
    }
}

impl crate::preview::Manager {
    /// Capture only the selected real tab/session before controlled work. The
    /// caller must already have authenticated its thread and tab scope.
    pub fn capture_evaluation(&self, thread: &str, tab: &str) -> Result<CapturedEvaluation, Value> {
        let page = self.browser.page(thread, tab)?;
        let session = page
            .tab
            .session
            .clone()
            .ok_or_else(|| json!({"message":"Browser session is unavailable"}))?;
        Ok(CapturedEvaluation {
            actor: page.tab.exact_tx.downgrade(),
            session,
            done: page.tab.done.clone(),
            control: page.control(),
            wake: page.tab.exact_wake.clone(),
        })
    }

    /// Owner-library exact evaluation; this does not register a public RPC or
    /// replace authenticated MCP/control admission and native result encoding.
    pub async fn evaluate_exact(
        &self,
        thread: &str,
        tab: &str,
        params: &[u8],
        budget: Duration,
    ) -> Result<Packet, Value> {
        self.browser
            .evaluate_exact(thread, tab, params, budget)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_transport_parameters_cannot_select_another_context_or_root_command() {
        crate::test_sandbox::assert_isolated();
        assert!(parameters(br#"{"expression":"'\ud800'","awaitPromise":true}"#).is_ok());
        for bytes in [
            br#"{"expression":"1","contextId":2}"#.as_slice(),
            br#"{"expression":"1","uniqueContextId":"foreign"}"#,
            br#"{"expression":"1","method":"Browser.close"}"#,
            br#"{"expression":true}"#,
            br#"{"expression":"1","returnByValue":null}"#,
        ] {
            assert!(parameters(bytes).is_err());
        }
    }
}
