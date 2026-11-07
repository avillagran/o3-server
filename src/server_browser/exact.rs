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
pub(super) struct Request {
    pub params: JavaScriptJson,
    pub session: String,
    pub expires: tokio::time::Instant,
    pub reply: oneshot::Sender<Result<Packet, Fault>>,
    pub permit: write::Permit,
}
#[derive(Serialize)]
pub(super) struct Command<'a> {
    pub id: u64,
    pub method: &'static str,
    pub params: &'a JavaScriptJson,
    #[serde(rename = "sessionId")]
    pub session: &'a str,
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
                params,
                session,
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
