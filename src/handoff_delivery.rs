//! Provider history is delivered once, with durable acknowledgement before mutation.
use crate::{
    handoff_history as history,
    provider::{ProviderEvent, RunInput},
};
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio::sync::{mpsc, oneshot};
struct Receipt {
    id: String,
    item_ids: Vec<Value>,
    omitted: Value,
    handoff: Option<Value>,
}
pub(crate) struct Delivery {
    receipts: Vec<Receipt>,
    context: String,
    messages: Vec<Value>,
}
fn manual_handoff(h: &Value) -> bool {
    h["strategy"] == "manual_context" && h["transferId"].is_string()
}
pub(crate) fn maintenance(text: &Value) -> bool {
    text.as_str().is_some_and(|s| {
        let s = s.trim().to_ascii_lowercase();
        s == "/compact" || s == "/goal" || s.starts_with("/goal ")
    })
}
pub(crate) fn maintenance_input(input: &RunInput) -> bool {
    input.attachments.as_array().is_some_and(|a| a.is_empty()) && maintenance(&json!(input.text))
}
fn same_pending(input: &RunInput, h: &Value) -> bool {
    input.native_session_id.as_deref().is_some_and(|native| {
        h["delivery"]["nativeThreadId"] == native && h["delivery"]["status"] == "pending"
    })
}
pub(crate) fn manual(input: &RunInput, current: &mut [Value]) -> Result<Vec<Delivery>, String> {
    let mut receipts = vec![];
    let mut contexts = vec![];
    for h in input.model_selection["o3TransferHandoffs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| manual_handoff(h))
    {
        if same_pending(input, h) {
            return Err("Context handoff delivery is uncertain; create a fresh conversation before replaying history.".into());
        }
        contexts.push(h["summaryText"].as_str().unwrap_or("").to_owned());
        receipts.push(Receipt {
            id: h["id"].as_str().ok_or("handoff id missing")?.into(),
            item_ids: vec![],
            omitted: json!([]),
            handoff: None,
        });
    }
    if receipts.is_empty() {
        return Ok(vec![]);
    }
    let context = contexts.join("\n\n");
    let user = current[0]["text"].as_str().unwrap_or("");
    current[0]["text"] = json!(format!("{context}\n\nUser message:\n{user}"));
    if current
        .iter()
        .filter_map(|v| v["text"].as_str())
        .map(str::len)
        .sum::<usize>()
        > 1024 * 1024
        || serde_json::to_vec(current)
            .map_err(|e| e.to_string())?
            .len()
            > 8 * 1024 * 1024
    {
        return Err("Side chat context exceeds native input budget".into());
    }
    Ok(vec![Delivery {
        receipts,
        context,
        messages: vec![],
    }])
}
pub(crate) fn prepare(input: &RunInput, current: &[Value]) -> Result<Vec<Delivery>, String> {
    if let Some(error) = input.model_selection["o3HandoffError"].as_str() {
        return Err(error.into());
    }
    let handoffs = input.model_selection["o3TransferHandoffs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| !manual_handoff(h))
        .cloned()
        .collect::<Vec<_>>();
    if handoffs.is_empty() {
        return Ok(vec![]);
    }
    if handoffs.iter().any(|h| same_pending(input, h)) {
        return Err("Context handoff delivery is uncertain; create a fresh conversation before replaying history.".into());
    }
    let text_bytes = current
        .iter()
        .map(|b| b["text"].as_str().map_or(0, str::len) + b["path"].as_str().map_or(0, str::len))
        .sum::<usize>();
    let frame_bytes = serde_json::to_vec(current)
        .map_err(|e| e.to_string())?
        .len();
    let room = (1024 * 1024usize)
        .saturating_sub(text_bytes + 4096)
        .min((8 * 1024 * 1024usize).saturating_sub(frame_bytes + 4096))
        .min(crate::handoff_budget::for_input(input, current));
    let mut coverage=handoffs.iter().map(|h|format!("Context handoff ({}):\n{}",h["strategy"].as_str().unwrap_or(""),h["history"]["coverage"].as_str().map(str::to_owned).unwrap_or_else(||format!("From thread {}, runs {}-{}. Recover history with t3_thread_read, view=activity; paginate with afterPosition, and use itemId/textOffset for long items.",h["threadId"].as_str().unwrap_or(""),h["coveredRunOrdinals"]["from"],h["coveredRunOrdinals"]["to"])))).collect::<Vec<_>>().join("\n");
    if history::cost(&[], &coverage) > 4000.min(room / 2) {
        coverage=format!("Context handoff. {} handoff records; detailed coverage references omitted. Recover history with t3_thread_read({{threadId:\"{}\",view:\"activity\",limit:20,maxCharsPerItem:4000}}); paginate with afterPosition=nextPosition. Follow fork/handoff source references in activity. For long items use itemId and textOffset=nextTextOffset until null.",handoffs.len(),handoffs[0]["threadId"].as_str().unwrap_or(""));
    }
    let old_context = handoffs
        .iter()
        .filter(|h| h.get("history").is_none())
        .filter_map(|h| h["summaryText"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !old_context.is_empty()
        && history::cost(&[], &format!("{coverage}\n{old_context}")) + 512 <= room
    {
        coverage.push('\n');
        coverage.push_str(&old_context)
    }
    let mut seen = input.model_selection["o3DeliveredHistoryItemIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let messages = handoffs
        .iter()
        .flat_map(|h| h["history"]["messages"].as_array().into_iter().flatten())
        .filter(|m| seen.insert(m["itemId"].as_str().unwrap_or("").to_owned()))
        .cloned()
        .collect::<Vec<_>>();
    let omitted = handoffs
        .iter()
        .map(|h| h["history"]["omittedItems"].as_u64().unwrap_or(0) as usize)
        .sum();
    let selected = history::select(&messages, &coverage, omitted, room);
    let messages = selected["messages"].as_array().unwrap().clone();
    let context = selected["context"].as_str().unwrap().to_owned();
    if history::cost(&messages, &context) > room {
        return Err("Current input leaves no room for historical context".into());
    }
    let selected_ids = messages
        .iter()
        .map(|m| m["itemId"].clone())
        .collect::<HashSet<_>>();
    let omitted_ids = selected["omittedItemIds"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut receipts = vec![];
    for h in handoffs {
        let mut omitted = h["history"]["omittedItemIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        omitted.extend(
            h["history"]["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| omitted_ids.contains(&m["itemId"]))
                .map(|m| m["itemId"].clone()),
        );
        let item_ids = h["history"]["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| selected_ids.contains(&m["itemId"]))
            .map(|m| m["itemId"].clone())
            .collect();
        receipts.push(Receipt {
            id: h["id"].as_str().ok_or("handoff id missing")?.into(),
            item_ids,
            omitted: json!(omitted),
            handoff: if h["_o3Ephemeral"] == true {
                let mut h = h.clone();
                h.as_object_mut().unwrap().remove("_o3Ephemeral");
                Some(h)
            } else {
                None
            },
        });
    }
    Ok(vec![Delivery {
        receipts,
        context,
        messages,
    }])
}
impl Delivery {
    pub fn inline(&self) -> String {
        history::render(&self.messages, &self.context)
    }
    pub fn items(&self) -> Value {
        history::response_items(&self.messages, &self.context)
    }
    pub async fn acknowledge(
        &self,
        output: &mpsc::Sender<ProviderEvent>,
        native: &str,
        status: &str,
    ) -> Result<(), String> {
        for receipt in &self.receipts {
            let (reply, ready) = oneshot::channel();
            output.send(ProviderEvent::ContextDelivery{data:json!({"handoffId":receipt.id,"nativeThreadId":native,"status":status,"itemIds":receipt.item_ids,"omittedItemIds":receipt.omitted,"handoff":if status=="pending"{receipt.handoff.clone()}else{None}}),reply}).await.map_err(|_|"Handoff supervisor closed")?;
            ready.await.map_err(|_| "Handoff acknowledgement lost")??;
        }
        Ok(())
    }
}

pub(crate) fn recovery_reason(
    input: &RunInput,
    current: &[Value],
) -> Option<crate::provider::NativeRecoveryReason> {
    if crate::native_recovery::uncertain(input) {
        return Some(crate::provider::NativeRecoveryReason::UncertainHistory);
    }
    let mut preview = current.to_vec();
    if input.native_session_id.is_some()
        && !maintenance_input(input)
        && manual(input, &mut preview)
            .and_then(|_| prepare(input, &preview))
            .is_err_and(|e| e == "Current input leaves no room for historical context")
    {
        return Some(crate::provider::NativeRecoveryReason::HandoffBudget);
    }
    None
}
