//! Native occupancy and handoff allowance from nightly4165 handoffBudget.ts (MIT).
use crate::{
    context_transfer::{list, own_items},
    events::get,
    handoff_history as history,
    provider::RunInput,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;
type Result<T> = std::result::Result<T, String>;
pub(crate) fn attachment_allowance(attachments: &Value) -> u64 {
    attachments
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| if a["type"] == "image" { 8192 } else { 4096 })
        .sum()
}
pub(crate) fn usage_for_handoff(
    same_native: bool,
    same_selection: bool,
    reuse: bool,
    previous: &Value,
    known: Option<u64>,
) -> Value {
    if !same_native || previous.is_null() {
        return Value::Null;
    }
    if same_selection {
        return previous.clone();
    }
    let mut usage = json!({"usedTokens":previous["usedTokens"]});
    let reported = previous["maxTokens"].as_u64().filter(|v| *v > 0);
    if let Some(max) = if reuse { reported } else { known.or(reported) } {
        usage["maxTokens"] = json!(max)
    }
    usage
}
pub(crate) fn allowance(
    cap: usize,
    text: &str,
    attachments: &Value,
    usage: &Value,
    estimate: u64,
    known: Option<u64>,
) -> usize {
    let window = known
        .or_else(|| usage["maxTokens"].as_u64())
        .unwrap_or(128000)
        .min(usage["maxTokens"].as_u64().unwrap_or(u64::MAX))
        .min(usage["autoCompactThreshold"].as_u64().unwrap_or(u64::MAX));
    let current = serde_json::to_vec(text)
        .map_or(u64::MAX, |v| v.len() as u64)
        .saturating_add(attachment_allowance(attachments));
    let native = usage["usedTokens"].as_u64().unwrap_or(estimate);
    cap.min(64000).min(
        window.saturating_sub(
            native
                .saturating_add(current)
                .saturating_add(16000.max(window.div_ceil(4))),
        ) as usize,
    )
}
fn selections_equal(a: &Value, b: &Value) -> bool {
    fn options(v: &Value) -> Vec<(String, Value)> {
        let mut opts = v["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| Some((o["id"].as_str()?.to_owned(), o["value"].clone())))
            .collect::<Vec<_>>();
        opts.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                a.1.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| a.1.to_string())
                    .cmp(
                        &b.1.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| b.1.to_string()),
                    )
            })
        });
        opts
    }
    a["instanceId"] == b["instanceId"] && a["model"] == b["model"] && options(a) == options(b)
}
fn without_effort(selection: &Value) -> Value {
    let mut v = selection.clone();
    if let Some(options) = v["options"].as_array_mut() {
        options.retain(|o| o["id"] != "reasoningEffort")
    }
    v
}
pub(crate) fn context(db: &Connection, thread: &Value, run: &Value) -> Result<Value> {
    let tid = thread["id"].as_str().ok_or("thread id missing")?;
    let Some(ptid) = run["providerThreadId"].as_str() else {
        // Legacy imported/restart runs acquire their first app row at native binding.
        return Ok(
            json!({"nativeThreadId":null,"contextUsage":null,"nativeContextEstimate":0,"uncertainDelivery":false,"modelContextWindow":null}),
        );
    };
    let pt = get(db, "providerThreads", ptid)?;
    let native = &pt["nativeThreadRef"]["nativeId"];
    let attempts = list(db, "attempts", tid)?;
    let runs = list(db, "runs", tid)?;
    let turns = list(db, "providerTurns", tid)?;
    let latest = turns
        .iter()
        .filter(|t| {
            t["providerThreadId"] == ptid
                && t["tokenUsage"].is_object()
                && attempts.iter().any(|a| {
                    a["id"] == t["runAttemptId"]
                        && a["nativeThreadId"] == *native
                        && native.is_string()
                        && a["providerThreadId"] == ptid
                        && a["rootNodeId"] == t["nodeId"]
                })
        })
        .max_by_key(|t| t["tokenUsage"]["updatedAt"].as_str().unwrap_or(""));
    let previous_run = latest
        .and_then(|t| attempts.iter().find(|a| a["id"] == t["runAttemptId"]))
        .and_then(|a| runs.iter().find(|r| r["id"] == a["runId"]))
        .or_else(|| {
            runs.iter()
                .filter(|r| {
                    r["providerThreadId"] == ptid
                        && !r["startedAt"].is_null()
                        && r["ordinal"].as_i64() < run["ordinal"].as_i64()
                })
                .max_by_key(|r| r["ordinal"].as_i64())
        });
    let mut previous = pt["contextUsage"].clone();
    if let Some(t) = latest {
        if previous.is_null() {
            previous = json!({})
        }
        previous["usedTokens"] = t["tokenUsage"]["usedTokens"].clone();
        if t["tokenUsage"]["maxTokens"].as_u64().is_some_and(|n| n > 0) {
            previous["maxTokens"] = t["tokenUsage"]["maxTokens"].clone();
        }
    }
    let same =
        previous_run.is_none_or(|r| selections_equal(&r["modelSelection"], &run["modelSelection"]));
    let reuse = same
        || (pt["driver"] == "codex"
            && previous_run.is_some_and(|r| {
                selections_equal(
                    &without_effort(&r["modelSelection"]),
                    &without_effort(&run["modelSelection"]),
                )
            }));
    let known = if pt["driver"] == "claudeAgent" {
        crate::claude_models::context_window(&run["modelSelection"])
    } else {
        None
    };
    let usage = usage_for_handoff(native.is_string(), same, reuse, &previous, known);
    let all_handoffs = list(db, "contextHandoffs", tid)?;
    let uncertain = all_handoffs.iter().any(|h| {
        h["toProviderThreadId"] == ptid
            && h["delivery"]["nativeThreadId"] == *native
            && native.is_string()
            && h["delivery"]["status"] == "pending"
    });
    let settled = all_handoffs
        .into_iter()
        .filter(|h| {
            h["toProviderThreadId"] == ptid
                && h["delivery"]["nativeThreadId"] == *native
                && native.is_string()
                && matches!(
                    h["delivery"]["status"].as_str(),
                    Some("inline" | "injected")
                )
        })
        .collect::<Vec<_>>();
    let delivered = settled
        .iter()
        .flat_map(|h| h["delivery"]["itemIds"].as_array().into_iter().flatten())
        .cloned()
        .collect::<HashSet<_>>();
    let covered = settled
        .iter()
        .flat_map(|h| [&h["delivery"]["itemIds"], &h["delivery"]["omittedItemIds"]])
        .flat_map(|a| a.as_array().into_iter().flatten())
        .cloned()
        .collect::<HashSet<_>>();
    let accepted = attempts
        .iter()
        .filter(|a| {
            a["providerThreadId"] == ptid && turns.iter().any(|t| t["runAttemptId"] == a["id"])
        })
        .collect::<Vec<_>>();
    let native_runs = accepted
        .iter()
        .filter(|a| a["nativeThreadId"] == *native && native.is_string())
        .map(|a| a["runId"].clone())
        .collect::<HashSet<_>>();
    let legacy_runs = accepted
        .iter()
        .filter(|a| a.get("nativeThreadId").is_none())
        .map(|a| a["runId"].clone())
        .collect::<HashSet<_>>();
    let recovered = runs
        .iter()
        .filter(|r| {
            r["providerThreadId"] == ptid
                && settled.iter().any(|h| {
                    h["strategy"] == "full_thread_summary"
                        && h["fromProviderThreadIds"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!(ptid)))
                        && r["ordinal"].as_i64() >= h["coveredRunOrdinals"]["from"].as_i64()
                        && r["ordinal"].as_i64() <= h["coveredRunOrdinals"]["to"].as_i64()
                })
        })
        .map(|r| r["id"].clone())
        .collect::<HashSet<_>>();
    let mut estimate = 0u64;
    if native.is_string() {
        for item in own_items(db, tid, i64::MAX)? {
            if item["runId"] == run["id"] {
                continue;
            }
            let accepted = item["providerThreadId"] == ptid
                && (native_runs.contains(&item["runId"])
                    || (legacy_runs.contains(&item["runId"])
                        && !covered.contains(&item["id"])
                        && !recovered.contains(&item["runId"])));
            if !accepted && !delivered.contains(&item["id"]) {
                continue;
            }
            if let Some(message) = history::message(&item) {
                estimate =
                    estimate.saturating_add(message["text"].as_str().unwrap_or("").len() as u64)
            }
            if item["type"] == "user_message" && accepted {
                estimate = estimate.saturating_add(attachment_allowance(&item["attachments"]));
            }
        }
    }
    Ok(
        json!({"nativeThreadId":native,"contextUsage":usage,"nativeContextEstimate":estimate,"uncertainDelivery":uncertain,"modelContextWindow":known.or_else(||previous["maxTokens"].as_u64().filter(|_|!usage.is_null()||reuse))}),
    )
}
pub(crate) fn for_input(input: &RunInput, current: &[Value]) -> usize {
    let meta = &input.model_selection["o3HandoffBudget"];
    let same = input
        .native_session_id
        .as_deref()
        .is_some_and(|id| meta["nativeThreadId"] == id);
    let usage = if same {
        &meta["contextUsage"]
    } else {
        &Value::Null
    };
    let text = current
        .first()
        .and_then(|v| v["text"].as_str())
        .unwrap_or(&input.text);
    allowance(
        history::cap(),
        text,
        &input.attachments,
        usage,
        if same {
            meta["nativeContextEstimate"].as_u64().unwrap_or(0)
        } else {
            0
        },
        meta["modelContextWindow"].as_u64(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_native_identity_and_wrong_root_telemetry_cannot_revive_usage() {
        let db = Connection::open_in_memory().unwrap();
        crate::events::initialize(&db).unwrap();
        let selection = json!({"instanceId":"codex","model":"test"});
        let thread = json!({"id":"t"});
        let current =
            json!({"id":"current","ordinal":2,"providerThreadId":"pt","modelSelection":selection});
        for (kind, id, v) in [
            (
                "providerThreads",
                "pt",
                json!({"id":"pt","driver":"codex","nativeThreadRef":{"nativeId":"new"},"contextUsage":null}),
            ),
            (
                "runs",
                "old",
                json!({"id":"old","ordinal":1,"status":"completed","providerThreadId":"pt","modelSelection":selection,"startedAt":"2026-11-04T00:00:00.000Z"}),
            ),
            (
                "attempts",
                "a",
                json!({"id":"a","runId":"old","providerThreadId":"pt","nativeThreadId":"old","rootNodeId":"root"}),
            ),
            (
                "providerTurns",
                "turn",
                json!({"id":"turn","providerThreadId":"pt","runAttemptId":"a","nodeId":"root","tokenUsage":{"usedTokens":127999,"maxTokens":128000,"updatedAt":"2026-11-04T00:00:00.000Z"}}),
            ),
        ] {
            crate::events::put(&db, kind, id, "t", &v).unwrap();
        }
        assert!(context(&db, &thread, &current).unwrap()["contextUsage"].is_null());
        db.execute("UPDATE records SET body=json_set(body,'$.nativeThreadId','new','$.rootNodeId','different') WHERE kind='attempts'",[]).unwrap();
        assert!(context(&db, &thread, &current).unwrap()["contextUsage"].is_null());
        db.execute(
            "UPDATE records SET body=json_set(body,'$.rootNodeId','root') WHERE kind='attempts'",
            [],
        )
        .unwrap();
        assert_eq!(
            context(&db, &thread, &current).unwrap()["contextUsage"]["usedTokens"],
            127999
        );
    }
    #[test]
    fn source_usage_and_budget_fixtures_match_windows_reserves_and_attachment_allowances() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/transfer/handoff-budget.json"
        ))
        .unwrap();
        for case in fixture["usageCases"].as_array().unwrap() {
            let i = &case["input"];
            assert_eq!(
                usage_for_handoff(
                    i["sameNativeThread"].as_bool().unwrap(),
                    i["sameSelection"].as_bool().unwrap(),
                    i["reuseTelemetry"].as_bool().unwrap(),
                    &i["previousUsage"],
                    i["knownModelWindow"].as_u64()
                ),
                case["expected"]
            );
        }
        for case in fixture["budgetCases"].as_array().unwrap() {
            let i = &case["input"];
            assert_eq!(
                json!(allowance(
                    i["tokenCap"].as_u64().unwrap() as usize,
                    i["userText"].as_str().unwrap(),
                    &i["attachments"],
                    &i["usage"],
                    i["nativeContextEstimate"].as_u64().unwrap(),
                    i["modelContextWindow"].as_u64()
                )),
                case["expected"]
            );
        }
    }
}
