//! Captured-target authority for the private native Page transport.
//! No model argument can construct a grant or select its browser connection.
use crate::native_page_worker::{
    cdp::{Packet, Payload},
    JavaScriptJson,
};
use serde::de::{MapAccess, Visitor};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SESSIONS: usize = 64;
const MAX_ATTACHMENTS: usize = 8;
const MAX_REQUESTS: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Denied {
    Closed,
    Scope,
    Protocol,
    Capacity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    None,
    Attach {
        target: String,
        parent: Option<String>,
    },
    Detach {
        session: String,
    },
    Window,
}
#[derive(Clone, Debug)]
pub(super) struct Reservation {
    pub ticket: u64,
    pub effect: Effect,
}
#[derive(Clone, Debug)]
struct Session {
    target: String,
    parent: Option<String>,
    usable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Receipt {
    Attached,
    Detached,
    NotWritten,
    Rejected,
    Unknown,
}
/// This state is minted only from the actual actor's captured Page. The caller
/// supplies neither an endpoint nor another thread's target/session identity.
pub(super) struct State {
    target: String,
    sessions: BTreeMap<String, Session>,
    windows: BTreeSet<u64>,
    attachments: BTreeMap<u64, (String, bool)>,
    requests: BTreeMap<u64, bool>,
    next: u64,
    closing: bool,
    unknown: bool,
    receipts: std::collections::VecDeque<Receipt>,
    receipt_totals: [u64; 5],
}
struct Fields(BTreeMap<String, Box<RawValue>>);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unambiguous CDP metadata")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.len() >= 64 || fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("ambiguous CDP metadata"));
                    }
                }
                Ok(Fields(fields))
            }
        }
        d.deserialize_map(Object)
    }
}
fn object(raw: &str) -> Result<Fields, Denied> {
    serde_json::from_str(raw).map_err(|_| Denied::Protocol)
}
fn field<T: serde::de::DeserializeOwned>(fields: &Fields, name: &str) -> Result<T, Denied> {
    let value = fields.0.get(name).ok_or(Denied::Protocol)?;
    serde_json::from_str(value.get()).map_err(|_| Denied::Protocol)
}
fn identity(fields: &Fields, name: &str) -> Result<String, Denied> {
    let value: String = field(fields, name)?;
    if value.is_empty() || value.len() > 1024 {
        return Err(Denied::Protocol);
    }
    Ok(value)
}
impl State {
    pub fn captured(target: String) -> Result<Self, Denied> {
        if target.is_empty() || target.len() > 1024 {
            return Err(Denied::Protocol);
        }
        Ok(Self {
            target,
            sessions: BTreeMap::new(),
            windows: BTreeSet::new(),
            attachments: BTreeMap::new(),
            requests: BTreeMap::new(),
            next: 0,
            closing: false,
            unknown: false,
            receipts: std::collections::VecDeque::new(),
            receipt_totals: [0; 5],
        })
    }
    fn known_target(&self, target: &str) -> bool {
        target == self.target
            || self
                .sessions
                .values()
                .any(|session| session.usable && session.target == target)
    }
    pub fn reserve(
        &mut self,
        method: &str,
        params: &JavaScriptJson,
        session: Option<&str>,
    ) -> Result<Reservation, Denied> {
        if self.closing && !(method == "Target.detachFromTarget" && session.is_none()) {
            return Err(Denied::Closed);
        }
        if session.is_some_and(|id| !self.sessions.get(id).is_some_and(|session| session.usable)) {
            return Err(Denied::Scope);
        }
        if self.requests.len() >= MAX_REQUESTS {
            return Err(Denied::Capacity);
        }
        let fields = object(params.json())?;
        if session.is_none() {
            let allowed: &[&str] = match method {
                "Browser.getVersion" => &[],
                "Target.getTargetInfo" | "Browser.getWindowForTarget" => &["targetId"],
                "Target.attachToTarget" => &["targetId", "flatten"],
                "Target.detachFromTarget" => &["sessionId"],
                "Browser.getWindowBounds" => &["windowId"],
                _ => return Err(Denied::Scope),
            };
            if fields
                .0
                .keys()
                .any(|name| !allowed.contains(&name.as_str()))
            {
                return Err(Denied::Scope);
            }
        }
        let effect = match (method, session) {
            ("Browser.getVersion", None) => Effect::None,
            ("Target.getTargetInfo", None) => {
                if !self.known_target(&identity(&fields, "targetId")?) {
                    return Err(Denied::Scope);
                }
                Effect::None
            }
            ("Target.attachToTarget", None) => {
                let target = identity(&fields, "targetId")?;
                if !self.known_target(&target) || !field::<bool>(&fields, "flatten")? {
                    return Err(Denied::Scope);
                }
                if self.attachments.len() >= MAX_ATTACHMENTS
                    || self.sessions.len() + self.attachments.len() >= MAX_SESSIONS
                {
                    return Err(Denied::Capacity);
                }
                let parent = if target == self.target {
                    None
                } else {
                    self.sessions
                        .iter()
                        .find(|(_, session)| session.usable && session.target == target)
                        .map(|(id, _)| id.clone())
                };
                Effect::Attach { target, parent }
            }
            ("Target.detachFromTarget", None) => {
                let session = identity(&fields, "sessionId")?;
                if !self.sessions.contains_key(&session) {
                    return Err(Denied::Scope);
                }
                Effect::Detach { session }
            }
            ("Target.setAutoAttach", Some(_)) => {
                if !field::<bool>(&fields, "flatten")? {
                    return Err(Denied::Scope);
                }
                Effect::None
            }
            ("Browser.getWindowForTarget", None) => {
                if identity(&fields, "targetId")? != self.target {
                    return Err(Denied::Scope);
                }
                Effect::Window
            }
            ("Browser.getWindowBounds", None) => {
                if !self.windows.contains(&field::<u64>(&fields, "windowId")?) {
                    return Err(Denied::Scope);
                }
                Effect::None
            }
            (_, Some(_)) if page_method(method) => Effect::None,
            _ => return Err(Denied::Scope),
        };
        self.next = self
            .next
            .checked_add(1)
            .filter(|id| *id <= 9_007_199_254_740_991)
            .ok_or(Denied::Capacity)?;
        if let Effect::Attach { target, .. } = &effect {
            self.attachments.insert(self.next, (target.clone(), false));
        }
        self.requests.insert(self.next, false);
        Ok(Reservation {
            ticket: self.next,
            effect,
        })
    }
    fn receipt(&mut self, receipt: Receipt) {
        let index = match receipt {
            Receipt::Attached => 0,
            Receipt::Detached => 1,
            Receipt::NotWritten => 2,
            Receipt::Rejected => 3,
            Receipt::Unknown => 4,
        };
        self.receipt_totals[index] = self.receipt_totals[index].saturating_add(1);
        if self.receipts.len() == 128 {
            self.receipts.pop_front();
        }
        self.receipts.push_back(receipt);
    }
    /// Separate owner cleanup capability; it remains valid after close and can
    /// detach a refused child session without granting its target to the worker.
    pub fn cleanup_detach(&mut self, session: &str) -> Result<Reservation, Denied> {
        if !self.sessions.contains_key(session) {
            return Err(Denied::Scope);
        }
        if self.requests.len() >= MAX_REQUESTS {
            return Err(Denied::Capacity);
        }
        self.next = self
            .next
            .checked_add(1)
            .filter(|id| *id <= 9_007_199_254_740_991)
            .ok_or(Denied::Capacity)?;
        self.requests.insert(self.next, false);
        Ok(Reservation {
            ticket: self.next,
            effect: Effect::Detach {
                session: session.into(),
            },
        })
    }
    pub fn started(&mut self, reservation: &Reservation) {
        if let Some(started) = self.requests.get_mut(&reservation.ticket) {
            *started = true;
        }
        if let Some((_, started)) = self.attachments.get_mut(&reservation.ticket) {
            *started = true;
        }
    }
    /// Losing a caller is not an unknown wire outcome. An already written
    /// attachment remains tracked until its reply or an explicit outcome fence.
    pub fn cancelled(&mut self, reservation: &Reservation) {
        if self.requests.get(&reservation.ticket) == Some(&false) {
            self.requests.remove(&reservation.ticket);
            self.attachments.remove(&reservation.ticket);
            self.receipt(Receipt::NotWritten);
        }
    }
    pub fn wire_unknown(&mut self, reservation: &Reservation) {
        if self.requests.get(&reservation.ticket) == Some(&true) {
            self.mark_unknown();
        } else {
            self.cancelled(reservation);
        }
    }
    /// Completion remains active after close. A late attachment is registered
    /// for owned detach, never forgotten because its original caller has gone.
    pub fn completed(&mut self, reservation: &Reservation, packet: &Packet) -> Result<(), Denied> {
        if !self.requests.contains_key(&reservation.ticket) {
            return self.malformed();
        }
        let result = self.completed_inner(reservation, packet);
        if result.is_ok() {
            self.requests.remove(&reservation.ticket);
        }
        if result.is_err() {
            self.mark_unknown();
        }
        result
    }
    fn completed_inner(
        &mut self,
        reservation: &Reservation,
        packet: &Packet,
    ) -> Result<(), Denied> {
        let Some(payload) = packet.payload() else {
            return self.malformed();
        };
        if matches!(payload, Payload::Error(_)) {
            self.attachments.remove(&reservation.ticket);
            if matches!(&reservation.effect,Effect::Detach{session} if self.sessions.contains_key(session))
            {
                self.mark_unknown();
            }
            self.receipt(Receipt::Rejected);
            return Ok(());
        }
        let Payload::Result(result) = payload else {
            return self.malformed();
        };
        match &reservation.effect {
            Effect::Attach { target, parent } => {
                let Some((expected, _)) = self.attachments.get(&reservation.ticket) else {
                    return self.malformed();
                };
                if expected != target {
                    return self.malformed();
                }
                let id = identity(&object(result.json())?, "sessionId")?;
                if self.sessions.contains_key(&id) || self.sessions.len() >= MAX_SESSIONS {
                    return self.malformed();
                }
                self.attachments.remove(&reservation.ticket);
                self.sessions.insert(
                    id,
                    Session {
                        target: target.clone(),
                        parent: parent.clone(),
                        usable: !self.closing
                            && parent.as_ref().is_none_or(|parent| {
                                self.sessions
                                    .get(parent)
                                    .is_some_and(|session| session.usable)
                            }),
                    },
                );
                self.receipt(Receipt::Attached);
            }
            Effect::Detach { session } => {
                self.remove_session(session);
                self.receipt(Receipt::Detached);
            }
            Effect::Window => {
                self.windows
                    .insert(field::<u64>(&object(result.json())?, "windowId")?);
            }
            Effect::None => {}
        }
        Ok(())
    }
    fn malformed<T>(&mut self) -> Result<T, Denied> {
        self.unknown = true;
        self.receipt(Receipt::Unknown);
        Err(Denied::Protocol)
    }
    fn remove_session(&mut self, id: &str) {
        // Parent detach is not a fabricated receipt for its descendants. Keep
        // each owned child until its own notification or detach acknowledgment.
        let mut missing = vec![id.to_owned()];
        while let Some(parent) = missing.pop() {
            for (child, session) in &mut self.sessions {
                if session.parent.as_deref() == Some(&parent) {
                    session.usable = false;
                    missing.push(child.clone());
                }
            }
        }
        self.sessions.remove(id);
    }
    /// Root events cannot enumerate other threads. Child targets are admitted
    /// only by an event carried on a previously owned parent session.
    pub fn event(&mut self, packet: &Packet) -> Result<bool, Denied> {
        let result = self.event_inner(packet);
        if result.is_err() {
            self.mark_unknown();
        }
        result
    }
    fn event_inner(&mut self, packet: &Packet) -> Result<bool, Denied> {
        let Some(method) = packet.method() else {
            return Ok(false);
        };
        if let Some(session) = packet.session() {
            if !self.sessions.contains_key(session) {
                if method != "Target.detachedFromTarget" {
                    return Ok(false);
                }
                let Some(Payload::Parameters(params)) = packet.payload() else {
                    return self.malformed();
                };
                let child = identity(&object(params.json())?, "sessionId")?;
                if !self
                    .sessions
                    .get(&child)
                    .is_some_and(|child| child.parent.as_deref() == Some(session))
                {
                    return Ok(false);
                }
            }
        } else if method != "Target.detachedFromTarget" {
            return Ok(false);
        }
        match method {
            "Target.attachedToTarget" => {
                let Some(parent) = packet.session() else {
                    return Ok(false);
                };
                let Some(Payload::Parameters(params)) = packet.payload() else {
                    return self.malformed();
                };
                let fields = object(params.json())?;
                let id = identity(&fields, "sessionId")?;
                let info = object(fields.0.get("targetInfo").ok_or(Denied::Protocol)?.get())?;
                let kind: String = field(&info, "type")?;
                let usable = matches!(kind.as_str(), "iframe" | "worker")
                    && self
                        .sessions
                        .get(parent)
                        .is_some_and(|session| session.usable);
                let target = identity(&info, "targetId")?;
                if self.sessions.contains_key(&id) || self.sessions.len() >= MAX_SESSIONS {
                    return self.malformed();
                }
                self.sessions.insert(
                    id,
                    Session {
                        target,
                        parent: Some(parent.into()),
                        usable,
                    },
                );
                self.receipt(Receipt::Attached);
                if !usable {
                    return Ok(false);
                }
            }
            "Target.detachedFromTarget" => {
                let Some(Payload::Parameters(params)) = packet.payload() else {
                    return self.malformed();
                };
                let id = identity(&object(params.json())?, "sessionId")?;
                if !self.sessions.contains_key(&id) {
                    return Ok(false);
                }
                self.remove_session(&id);
            }
            _ => {
                if packet
                    .session()
                    .is_some_and(|session| !self.sessions[session].usable)
                {
                    return Ok(false);
                }
            }
        }
        Ok(!self.closing)
    }
    pub fn close(&mut self) {
        self.closing = true;
    }
    pub fn detach_sessions(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }
    pub fn refused_sessions(&self) -> Vec<String> {
        self.sessions
            .iter()
            .filter(|(_, session)| !session.usable)
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub fn receipt_totals(&self) -> [u64; 5] {
        self.receipt_totals
    }
    pub fn attachments_pending(&self) -> usize {
        self.attachments.len()
    }
    pub fn mark_unknown(&mut self) {
        self.unknown = true;
        self.receipt(Receipt::Unknown);
    }
    pub fn clean(&self) -> bool {
        self.closing
            && !self.unknown
            && self.requests.is_empty()
            && self.attachments.is_empty()
            && self.sessions.is_empty()
    }
}
fn page_method(method: &str) -> bool {
    let Some((domain, _)) = method.split_once('.') else {
        return false;
    };
    matches!(
        domain,
        "Accessibility"
            | "Animation"
            | "DOM"
            | "DOMSnapshot"
            | "Emulation"
            | "Input"
            | "Inspector"
            | "LayerTree"
            | "Log"
            | "Network"
            | "Page"
            | "Performance"
            | "Runtime"
    ) && !matches!(
        method,
        "Network.getAllCookies"
            | "Network.getCookies"
            | "Network.setCookie"
            | "Network.setCookies"
            | "Network.deleteCookies"
            | "Network.clearBrowserCookies"
            | "Network.clearBrowserCache"
            | "Page.crash"
            | "Page.close"
            | "Page.setDownloadBehavior"
            | "DOM.setFileInputFiles"
    )
}
