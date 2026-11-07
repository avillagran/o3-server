//! Exact application JSON for an owned CDP lane. This parser grants no authority
//! and does not replace the existing unrelated scalar CDP decoder.
use super::JavaScriptJson;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Parameters(JavaScriptJson),
    Result(JavaScriptJson),
    Error(JavaScriptJson),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    id: Option<u64>,
    method: Option<String>,
    session: Option<String>,
    payload: Option<Payload>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Size,
    Grammar,
    Duplicate,
    Envelope,
}
struct Fields(BTreeMap<String, Box<RawValue>>, bool);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an unambiguous CDP object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = BTreeMap::new();
                let mut duplicate = false;
                while let Some((name, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.insert(name, value).is_some() {
                        duplicate = true;
                    }
                }
                Ok(Fields(fields, duplicate))
            }
        }
        d.deserialize_map(Object)
    }
}
fn scalar<T: serde::de::DeserializeOwned>(raw: &RawValue) -> Result<T, Invalid> {
    serde_json::from_str(raw.get()).map_err(|_| Invalid::Envelope)
}
fn object(raw: Box<RawValue>) -> Result<JavaScriptJson, Invalid> {
    if !raw.get().trim_start().starts_with('{') {
        return Err(Invalid::Envelope);
    }
    Ok(JavaScriptJson(raw))
}
impl Packet {
    pub fn id(&self) -> Option<u64> {
        self.id
    }
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }
    pub fn payload(&self) -> Option<&Payload> {
        self.payload.as_ref()
    }
    /// The caller's admitted byte cap is checked before JSON parsing. Transport
    /// code must also enforce that cap before receiving/allocating a full frame.
    pub fn decode(bytes: &[u8], limit: usize) -> Result<Self, Invalid> {
        if bytes.is_empty() || bytes.len() > limit {
            return Err(Invalid::Size);
        }
        let Fields(mut fields, duplicate) =
            serde_json::from_slice(bytes).map_err(|_| Invalid::Grammar)?;
        if duplicate {
            return Err(Invalid::Duplicate);
        }
        if fields.keys().any(|name| {
            !matches!(
                name.as_str(),
                "id" | "method" | "sessionId" | "params" | "result" | "error"
            )
        }) {
            return Err(Invalid::Envelope);
        }
        let id = fields
            .remove("id")
            .map(|raw| scalar::<u64>(&raw))
            .transpose()?;
        if id.is_some_and(|id| id == 0 || id > 9_007_199_254_740_991) {
            return Err(Invalid::Envelope);
        }
        let method = fields
            .remove("method")
            .map(|raw| scalar::<String>(&raw))
            .transpose()?;
        if method.as_ref().is_some_and(|method| {
            method.is_empty()
                || method.len() > 256
                || !method.is_ascii()
                || method
                    .bytes()
                    .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        }) {
            return Err(Invalid::Envelope);
        }
        let session = fields
            .remove("sessionId")
            .map(|raw| scalar::<String>(&raw))
            .transpose()?;
        if session
            .as_ref()
            .is_some_and(|session| session.is_empty() || session.len() > 1024)
        {
            return Err(Invalid::Envelope);
        }
        let params = fields.remove("params");
        let result = fields.remove("result");
        let error = fields.remove("error");
        let payload = match (method.is_some(), id.is_some(), params, result, error) {
            (true, _, Some(params), None, None) => Some(Payload::Parameters(object(params)?)),
            (true, _, None, None, None) => None,
            (false, true, None, Some(result), None) => Some(Payload::Result(object(result)?)),
            (false, true, None, None, Some(error)) => Some(Payload::Error(object(error)?)),
            _ => return Err(Invalid::Envelope),
        };
        Ok(Self {
            id,
            method,
            session,
            payload,
        })
    }
}
impl Serialize for Packet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        if let Some(id) = self.id {
            map.serialize_entry("id", &id)?;
        }
        if let Some(method) = &self.method {
            map.serialize_entry("method", method)?;
        }
        if let Some(session) = &self.session {
            map.serialize_entry("sessionId", session)?;
        }
        if let Some(payload) = &self.payload {
            match payload {
                Payload::Parameters(value) => map.serialize_entry("params", value)?,
                Payload::Result(value) => map.serialize_entry("result", value)?,
                Payload::Error(value) => map.serialize_entry("error", value)?,
            }
        }
        map.end()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_units_survive_command_result_error_and_event_without_scalar_conversion() {
        crate::test_sandbox::assert_isolated();
        for raw in [
            br#"{"id":1,"method":"Runtime.evaluate","sessionId":"owned","params":{"expression":"'\ud800'"}}"#.as_slice(),
            br#"{"id":1,"sessionId":"owned","result":{"result":{"value":{"\ud800":"\udfff","pair":"\ud83d\ude00"}}}}"#,
            br#"{"id":1,"error":{"code":-32000,"message":"\ud800"}}"#,
            br#"{"method":"Runtime.consoleAPICalled","sessionId":"owned","params":{"args":[{"value":"\udfff"}]}}"#,
        ] {
            assert!(serde_json::from_slice::<serde_json::Value>(raw).is_err());
            let packet = Packet::decode(raw, 4096).unwrap();
            let bytes = serde_json::to_vec(&packet).unwrap();
            let repeated = Packet::decode(&bytes, 4096).unwrap();
            assert_eq!(packet, repeated);
            let opaque = match packet.payload.unwrap() { Payload::Parameters(v)|Payload::Result(v)|Payload::Error(v) => v };
            assert!(opaque.json().contains("ud800") || opaque.json().contains("udfff"));
        }
    }
    #[test]
    fn untrusted_control_fields_never_inherit_the_raw_payload_exception() {
        crate::test_sandbox::assert_isolated();
        for raw in [
            br#"{"id":1,"sessionId":"\ud800","result":{}}"#.as_slice(),
            br#"{"id":1,"method":"Runtime.\udfff","params":{}}"#,
            br#"{"id":"1","result":{}}"#,
            br#"{"id":0,"result":{}}"#,
            br#"{"id":9007199254740992,"result":{}}"#,
            br#"{"id":1,"result":{},"error":{}}"#,
            br#"{"id":1,"method":"Runtime.evaluate","result":{}}"#,
            br#"{"id":1,"result":{},"targetId":"foreign"}"#,
            br#"{"id":1,"result":null}"#,
            br#"{"id":1,"result":{"value":"\uZZZZ"}}"#,
        ] {
            assert!(Packet::decode(raw, 4096).is_err(), "{raw:?}");
        }
        assert_eq!(
            Packet::decode(br#"{"id":1,"id":2,"result":{}}"#, 4096),
            Err(Invalid::Duplicate)
        );
        assert_eq!(
            Packet::decode(br#"{"id":1,"result":{}}"#, 4),
            Err(Invalid::Size)
        );
    }
}
