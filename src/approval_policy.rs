//! Durable consent is scoped to one thread, provider and exact native operation.
use serde_json::{json, Value};
pub(crate) fn key(frame: &Value, claude: bool) -> Option<Value> {
    if claude {
        let r = &frame["request"];
        let tool = r["tool_name"].as_str()?;
        if frame["type"] != "permission"
            || matches!(tool, "AskUserQuestion" | "ExitPlanMode" | "EnterPlanMode")
        {
            return None;
        }
        Some(json!({"driver":"claudeAgent","tool":tool,"input":r.get("input")?}))
    } else {
        let method = frame["method"].as_str()?;
        if !matches!(
            method,
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
        ) {
            return None;
        }
        let mut params = frame["params"].as_object()?.clone();
        for k in ["threadId", "turnId", "itemId", "o3RequestId"] {
            params.remove(k);
        }
        // Never turn a generic approval with no operation into a wildcard grant.
        if !params.contains_key("command") && !params.contains_key("changes") {
            return None;
        }
        Some(json!({"driver":"codex","method":method,"params":params}))
    }
}
