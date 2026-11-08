//! Source host request metadata. Tool classification and caller identity are
//! outside action input; decoding an input value never creates a write permit.
use super::{JavaScriptJson, NativeOperation};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Request {
    request_id: String,
    thread_id: String,
    #[serde(default)]
    tab_id: Option<String>,
    tab_id_explicit: bool,
    agent_session_id: String,
    operation: String,
    input: Box<serde_json::value::RawValue>,
    timeout_ms: u64,
}
#[derive(Clone)]
pub struct CallbackRequest {
    pub request_id: String,
    pub thread_id: String,
    pub tab_id: Option<String>,
    pub tab_id_explicit: bool,
    pub agent_session_id: String,
    pub operation: String,
    pub input: JavaScriptJson,
    pub timeout_ms: u64,
    pub class: NativeOperation,
    pub metadata_status: bool,
}
impl CallbackRequest {
    pub(super) fn decode(
        raw: &JavaScriptJson,
        original_tool: NativeOperation,
        update_current_tab: bool,
        thread: &str,
        actual_agent_session: &str,
    ) -> Result<Self, ()> {
        let request: Request = serde_json::from_str(raw.json()).map_err(|_| ())?;
        if request.thread_id != thread
            || request.agent_session_id != actual_agent_session
            || request.request_id.is_empty()
            || request.timeout_ms == 0
        {
            return Err(());
        }
        let class = if update_current_tab {
            if !original_tool.permits_page_operation(&request.operation) {
                return Err(());
            }
            original_tool
        } else {
            // This is the source handler's status enrichment, not an arbitrary
            // observer selector or an extension of the original tool receipt.
            if request.operation != "status"
                || request.timeout_ms != 500
                || !serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                    request.input.get(),
                )
                .is_ok_and(|input| input.is_empty())
                || matches!(
                    original_tool,
                    NativeOperation::Status
                        | NativeOperation::Open
                        | NativeOperation::Navigate
                        | NativeOperation::Snapshot
                )
            {
                return Err(());
            }
            NativeOperation::Status
        };
        let valid_timeout = if !update_current_tab {
            request.timeout_ms == 500
        } else {
            match (original_tool, request.operation.as_str()) {
                (NativeOperation::Open, "open") => request.timeout_ms == 660_000,
                (NativeOperation::RecordingStop, "recordingStop") => request.timeout_ms == 120_000,
                (NativeOperation::Navigate, _)
                | (NativeOperation::Resize, _)
                | (NativeOperation::WaitFor, _) => request.timeout_ms <= 60_000,
                (
                    NativeOperation::Act,
                    "click" | "type" | "hover" | "select" | "drag" | "upload",
                ) => request.timeout_ms <= 60_000,
                _ => request.timeout_ms == 15_000,
            }
        };
        if !valid_timeout {
            return Err(());
        }
        Ok(Self {
            request_id: request.request_id,
            thread_id: request.thread_id,
            tab_id: request.tab_id,
            tab_id_explicit: request.tab_id_explicit,
            agent_session_id: request.agent_session_id,
            operation: request.operation,
            input: JavaScriptJson(request.input),
            timeout_ms: request.timeout_ms,
            class,
            metadata_status: !update_current_tab,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(operation: &str, timeout: u64, input: &str) -> JavaScriptJson {
        JavaScriptJson::parse(format!(r#"{{"requestId":"source-1","threadId":"thread","tabId":"tab","tabIdExplicit":false,"agentSessionId":"env\u0000published-session\u0000thread","operation":"{operation}","input":{input},"timeoutMs":{timeout}}}"#)).unwrap()
    }
    #[test]
    fn open_source_deadline_is_not_the_legacy_worker_ingress_budget() {
        let raw = request("open", 660_000, "{}");
        let decoded = CallbackRequest::decode(
            &raw,
            NativeOperation::Open,
            true,
            "thread",
            "env\0published-session\0thread",
        )
        .unwrap();
        assert_eq!(decoded.timeout_ms, 660_000);
        assert_eq!(decoded.class, NativeOperation::Open);
        assert!(CallbackRequest::decode(
            &request("open", 120_000, "{}"),
            NativeOperation::Open,
            true,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
        assert!(CallbackRequest::decode(
            &request("click", 660_000, "{}"),
            NativeOperation::Act,
            true,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
    }
    #[test]
    fn original_operation_class_and_published_session_cannot_be_relabelled() {
        let raw = request(
            "evaluate",
            15_000,
            r#"{"expression":"\ud800","caller":"model","ownerScope":{}}"#,
        );
        let decoded = CallbackRequest::decode(
            &raw,
            NativeOperation::Evaluate,
            true,
            "thread",
            "env\0published-session\0thread",
        )
        .unwrap();
        assert!(decoded.input.json().contains("\\ud800"));
        assert!(CallbackRequest::decode(
            &raw,
            NativeOperation::Snapshot,
            true,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
        assert!(CallbackRequest::decode(
            &raw,
            NativeOperation::Evaluate,
            true,
            "thread",
            "placeholder"
        )
        .is_err());
    }
    #[test]
    fn metadata_status_is_only_the_original_fixed_read_producer() {
        let raw = request("status", 500, "{}");
        let decoded = CallbackRequest::decode(
            &raw,
            NativeOperation::Act,
            false,
            "thread",
            "env\0published-session\0thread",
        )
        .unwrap();
        assert_eq!(decoded.class, NativeOperation::Status);
        assert!(decoded.metadata_status);
        assert!(CallbackRequest::decode(
            &raw,
            NativeOperation::Status,
            false,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
        assert!(CallbackRequest::decode(
            &request("status", 501, "{}"),
            NativeOperation::Act,
            false,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
        assert!(CallbackRequest::decode(
            &request("status", 500, r#"{"write":true}"#),
            NativeOperation::Act,
            false,
            "thread",
            "env\0published-session\0thread"
        )
        .is_err());
    }
}
