//! Unary RPCs that live outside `rpc::dispatch`: terminals and server operations.
use crate::store::State;
use serde_json::Value;
use std::sync::Arc;

/// `None` when `tag` is not handled here.
pub async fn call(s: &Arc<State>, tag: &str, p: &Value) -> Option<Result<Value, Value>> {
    match tag {
        "terminal.open" | "terminal.write" | "terminal.resize" | "terminal.clear"
        | "terminal.restart" | "terminal.close" => Some(crate::terminal::call(s, tag, p).await),
        _ => crate::ops::call(s, tag, p).await,
    }
}
