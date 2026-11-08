//! Private continuation messages on the owned toolkit process pipes. These
//! methods are not MCP tools and never grant browser-global CDP access.
use super::{Fault, JavaScriptJson};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ServiceMethod {
    CallerScope,
    Projection,
    ReserveCallback,
    RunCallback,
    MarkUsed,
    AbandonCallback,
    EndInvocation,
}
pub struct ServiceRequest {
    pub id: u64,
    pub caller: String,
    pub method: ServiceMethod,
    pub input: JavaScriptJson,
}
pub struct ServiceReply {
    pub failed: bool,
    pub payload: JavaScriptJson,
    pub callback_receipt: Option<String>,
}
pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<ServiceReply, Fault>> + Send>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    kind: String,
    nonce: String,
    id: u64,
    caller: String,
    method: ServiceMethod,
    input: Box<serde_json::value::RawValue>,
}
pub(super) fn decode(bytes: &[u8]) -> Result<(String, ServiceRequest), Fault> {
    let packet: Request = serde_json::from_slice(bytes).map_err(|_| Fault::Protocol)?;
    if packet.kind != "service"
        || packet.id == 0
        || packet.id > 9_007_199_254_740_991
        || packet.caller.len() != 96
        || !packet.caller.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Fault::Protocol);
    }
    Ok((
        packet.nonce,
        ServiceRequest {
            id: packet.id,
            caller: packet.caller,
            method: packet.method,
            input: JavaScriptJson(packet.input),
        },
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Reply<'a> {
    pub kind: &'static str,
    pub nonce: &'a str,
    pub id: u64,
    pub ok: bool,
    pub payload: &'a JavaScriptJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_receipt: Option<&'a str>,
}
pub(super) fn encode(nonce: &str, id: u64, reply: &ServiceReply) -> Result<Vec<u8>, Fault> {
    if reply.callback_receipt.as_ref().is_some_and(|receipt| {
        receipt.len() != 96 || !receipt.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(Fault::Protocol);
    }
    let encoded = serde_json::to_vec(&Reply {
        kind: "service-result",
        nonce,
        id,
        ok: !reply.failed,
        payload: &reply.payload,
        callback_receipt: reply.callback_receipt.as_deref(),
    })
    .map_err(|_| Fault::Protocol)?;
    if encoded.len() > super::MAX_FRAME_BYTES {
        return Err(Fault::TooLarge);
    }
    Ok(encoded)
}
