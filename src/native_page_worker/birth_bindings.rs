//! Source-issued installation evidence for one exact canonical Page. This is
//! not an initial-state observation or a complete present Status certificate.
use super::Fault;
use serde_json::{json, Value};

const EVENTS: [&str; 10] = [
    "request",
    "load",
    "framenavigated",
    "requestfailed",
    "dialog",
    "filechooser",
    "download",
    "popup",
    "close",
    "crash",
];

#[derive(Clone)]
pub(crate) struct ProducerBindings {
    target: String,
}
impl ProducerBindings {
    pub(super) fn handshake(
        value: &Value,
        nonce: &str,
        target: Option<&str>,
    ) -> Result<Option<Self>, Fault> {
        if *value == json!({"kind":"ready","protocol":1,"nonce":nonce}) {
            return Ok(None);
        }
        let target = target.ok_or(Fault::Protocol)?;
        let expected = json!({"kind":"ready","protocol":1,"nonce":nonce,"producerBindings":{
            "kind":"canonical-page-event-bindings","targetId":target,"events":EVENTS
        }});
        if *value != expected {
            return Err(Fault::Protocol);
        }
        Ok(Some(Self {
            target: target.to_owned(),
        }))
    }
    pub(crate) fn matches_target(&self, target: &str) -> bool {
        self.target == target
    }
}
