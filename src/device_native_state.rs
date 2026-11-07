//! Device inventory reconciliation and boot confirmation from native DeviceService.
use crate::device_native_hub::{Discovery, Platform};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum DetailUpdate {
    Unchanged,
    Clear,
    Set(String),
}
#[derive(Clone, Debug)]
pub struct Refresh {
    pub state: Value,
    pub detail: DetailUpdate,
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}
fn next(mut state: Value) -> Value {
    state["revision"] = json!(state["revision"].as_u64().unwrap_or(0) + 1);
    state
}
pub fn session_key(session: &Value) -> String {
    ["hostId", "deviceId", "threadId", "openedAt"]
        .map(|key| session[key].as_str().unwrap_or("undefined"))
        .join("\0")
}
pub struct RefreshInput<'a> {
    pub host_id: &'a str,
    pub listing: &'a Discovery,
    pub host_summaries: &'a Value,
    pub sessions_before: &'a [Value],
    pub ends_devices: bool,
    pub enabled: bool,
    pub host_unchanged: bool,
}
pub fn apply_discovery(current: &Value, input: RefreshInput<'_>) -> Refresh {
    if !input.enabled || !input.host_unchanged {
        return Refresh {
            state: current.clone(),
            detail: DetailUpdate::Unchanged,
        };
    }
    let incomplete = input.listing.incomplete_platforms.as_deref().unwrap_or(&[]);
    let android_incomplete = input.listing.android_discovery_incomplete == Some(true);
    let detail = match input
        .listing
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
    {
        Some(detail) if !incomplete.is_empty() || android_incomplete => {
            DetailUpdate::Set(detail.into())
        }
        _ => DetailUpdate::Clear,
    };
    let is_incomplete = |device: &Value| {
        incomplete
            .iter()
            .any(|platform| device["platform"] == platform.label())
    };
    let mut state = current.clone();
    state["hosts"] = input.host_summaries.clone();
    if input.host_id == "local" {
        if let Some(detail) = &input.listing.detail {
            state["hostStatusDetail"] = json!(detail);
        } else {
            state.as_object_mut().unwrap().remove("hostStatusDetail");
        }
    }
    let mut devices = array(&current["devices"])
        .iter()
        .filter(|device| device["hostId"] != input.host_id || is_incomplete(device))
        .cloned()
        .collect::<Vec<_>>();
    for device in &input.listing.devices {
        if !incomplete.contains(&device.platform) {
            let mut device = serde_json::to_value(device).unwrap();
            device["hostId"] = json!(input.host_id);
            devices.push(device);
        }
    }
    state["devices"] = json!(devices);
    if input.ends_devices {
        let before = input
            .sessions_before
            .iter()
            .map(session_key)
            .collect::<std::collections::HashSet<_>>();
        state["sessions"] = json!(array(&current["sessions"])
            .iter()
            .filter(|session| session["hostId"] != input.host_id
                || !before.contains(&session_key(session))
                || input
                    .listing
                    .devices
                    .iter()
                    .any(|device| session["deviceId"] == device.id))
            .cloned()
            .collect::<Vec<_>>());
    }
    let mut status = json!({"status":"ready"});
    if let Some(detail) = input
        .listing
        .detail
        .as_ref()
        .filter(|detail| !detail.is_empty())
    {
        status["detail"] = json!(detail);
    }
    if android_incomplete {
        status["androidDiscoveryIncomplete"] = json!(true);
    }
    if !incomplete.is_empty() {
        status["incompletePlatforms"] = json!(incomplete);
    }
    state["hostStatuses"][input.host_id] = status;
    Refresh {
        state: next(state),
        detail,
    }
}
pub fn apply_status(
    current: &Value,
    host_id: &str,
    status: &Value,
    cached_detail: Option<&str>,
    host_exists: bool,
) -> Value {
    if !host_exists {
        return current.clone();
    }
    let previous = &current["hostStatuses"][host_id];
    let detail = status
        .get("detail")
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .or_else(|| {
            (status["status"] == "ready"
                && (previous["incompletePlatforms"].is_array()
                    || previous["androidDiscoveryIncomplete"] == true))
                .then_some(cached_detail)
                .flatten()
        });
    let mut entry = json!({});
    if previous["androidDiscoveryIncomplete"] == true {
        entry["androidDiscoveryIncomplete"] = json!(true);
    }
    if previous["incompletePlatforms"].is_array() {
        entry["incompletePlatforms"] = previous["incompletePlatforms"].clone();
    }
    for (key, value) in status.as_object().into_iter().flatten() {
        entry[key] = value.clone();
    }
    if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
        entry["detail"] = json!(detail);
    }
    let mut state = current.clone();
    if host_id == "local" {
        state["hostStatus"] = status["status"].clone();
        if let Some(detail) = detail {
            state["hostStatusDetail"] = json!(detail);
        } else {
            state.as_object_mut().unwrap().remove("hostStatusDetail");
        }
    }
    state["hostStatuses"][host_id] = entry;
    next(state)
}
pub fn begin_boot(current: &Value, device: &Value, thread_id: &str) -> Value {
    let mut booting = array(&current["bootingDevices"])
        .iter()
        .filter(|entry| entry["hostId"] != device["hostId"] || entry["id"] != device["id"])
        .cloned()
        .collect::<Vec<_>>();
    let mut entry = device.clone();
    entry["threadId"] = json!(thread_id);
    booting.push(entry);
    let mut state = current.clone();
    state["bootingDevices"] = json!(booting);
    next(state)
}
pub fn finish_boot(current: &Value, host_id: &str, original_id: &str) -> Value {
    let mut state = current.clone();
    state["bootingDevices"] = json!(array(&current["bootingDevices"])
        .iter()
        .filter(|entry| entry["hostId"] != host_id || entry["id"] != original_id)
        .cloned()
        .collect::<Vec<_>>());
    next(state)
}
pub fn confirm_boot(
    current: &Value,
    host_id: &str,
    platform: Platform,
    original_id: &str,
    returned_id: &str,
) -> Result<Value, Value> {
    let incomplete = array(&current["hostStatuses"][host_id]["incompletePlatforms"])
        .iter()
        .any(|value| value == platform.label());
    if !incomplete {
        if let Some(device) = [returned_id, original_id].into_iter().find_map(|id| {
            array(&current["devices"])
                .iter()
                .find(|device| device["hostId"] == host_id && device["id"] == id)
        }) {
            return Ok(device.clone());
        }
    }
    Err(
        json!({"_tag":"DeviceNotFoundError","hostId":host_id,"deviceId":returned_id,"message":format!("Device {returned_id} was not found on host {host_id}.")}),
    )
}

/// Each thread keeps its own view of a device; only the same thread/host/device view is replaced.
pub fn record_session(current: &Value, session: &Value) -> Value {
    let mut sessions = array(&current["sessions"])
        .iter()
        .filter(|existing| {
            !(existing["threadId"] == session["threadId"]
                && existing["hostId"] == session["hostId"]
                && existing["deviceId"] == session["deviceId"])
        })
        .cloned()
        .collect::<Vec<_>>();
    sessions.push(session.clone());
    let mut state = current.clone();
    state["sessions"] = json!(sessions);
    next(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_native_service_refresh_and_status() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-state-4288.json"
        ))
        .unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let listing: Discovery = serde_json::from_value(input["listing"].clone()).unwrap();
            let result = apply_discovery(
                &input["current"],
                RefreshInput {
                    host_id: input["hostId"].as_str().unwrap(),
                    listing: &listing,
                    host_summaries: &input["current"]["hosts"],
                    sessions_before: array(&input["sessionsBefore"]),
                    ends_devices: input["endsDevices"].as_bool().unwrap(),
                    enabled: input["enabled"].as_bool().unwrap(),
                    host_unchanged: input["hostUnchanged"].as_bool().unwrap(),
                },
            );
            assert_eq!(
                result.state, case["expected"],
                "native refresh case {index}"
            );
            let cache = match &result.detail {
                DetailUpdate::Set(detail) => Some(detail.as_str()),
                _ => None,
            };
            for transition in case["transitions"].as_array().unwrap() {
                assert_eq!(
                    apply_status(
                        &transition["current"],
                        input["hostId"].as_str().unwrap(),
                        &transition["status"],
                        cache,
                        true
                    ),
                    transition["expected"],
                    "native status case {index}"
                );
            }
        }
    }
    #[test]
    fn pinned_native_boot_publications_and_confirmation() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-state-4288.json"
        ))
        .unwrap();
        for (index, case) in fixture["boot"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let host = input["hostId"].as_str().unwrap();
            let device = &input["initialDevice"];
            let id = device["id"].as_str().unwrap();
            let platform: Platform = serde_json::from_value(device["platform"].clone()).unwrap();
            let events = case["events"].as_array().unwrap();
            let initial: Discovery = serde_json::from_value(json!({"devices":[device]})).unwrap();
            let refreshed = apply_discovery(
                &case["before"],
                RefreshInput {
                    host_id: host,
                    listing: &initial,
                    host_summaries: &case["before"]["hosts"],
                    sessions_before: array(&case["before"]["sessions"]),
                    ends_devices: false,
                    enabled: true,
                    host_unchanged: true,
                },
            )
            .state;
            assert_eq!(refreshed, events[0], "pre-boot discovery {index}");
            if let Some(slot) = events
                .iter()
                .position(|state| !array(&state["bootingDevices"]).is_empty())
            {
                let summary = array(&events[slot - 1]["devices"])
                    .iter()
                    .find(|device| device["id"] == id)
                    .unwrap();
                assert_eq!(
                    begin_boot(&events[slot - 1], summary, "thread-open"),
                    events[slot],
                    "boot entry {index}"
                );
                let finished = finish_boot(&events[slot], host, id);
                assert_eq!(finished, events[slot + 1], "boot cleanup {index}");
                if input["bootFailure"] == true {
                    assert_eq!(
                        finished, case["after"],
                        "failed boot retains no slot {index}"
                    );
                    continue;
                }
                let listing: Discovery =
                    serde_json::from_value(input["afterBoot"].clone()).unwrap();
                let refreshed = apply_discovery(
                    &finished,
                    RefreshInput {
                        host_id: host,
                        listing: &listing,
                        host_summaries: &finished["hosts"],
                        sessions_before: array(&finished["sessions"]),
                        ends_devices: false,
                        enabled: true,
                        host_unchanged: true,
                    },
                )
                .state;
                assert_eq!(refreshed, events[slot + 2], "post-boot discovery {index}");
                match confirm_boot(
                    &refreshed,
                    host,
                    platform,
                    id,
                    input["returnedId"].as_str().unwrap(),
                ) {
                    Err(error) => {
                        assert_eq!(
                            error, case["result"]["error"],
                            "confirmation refusal {index}"
                        );
                        assert_eq!(refreshed, case["after"]);
                    }
                    Ok(device) => {
                        assert_eq!(
                            device["id"], case["result"]["value"]["deviceId"],
                            "confirmed id {index}"
                        );
                        assert_eq!(
                            record_session(&refreshed, &case["result"]["value"]),
                            case["after"],
                            "opened view {index}"
                        );
                    }
                }
            } else {
                assert_eq!(
                    record_session(&refreshed, &case["result"]["value"]),
                    case["after"],
                    "no-boot view {index}"
                );
            }
        }
    }
}
