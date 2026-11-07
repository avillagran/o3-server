//! Pinned native loaded-report guards and payload redaction; no browser effects.
use serde_json::{json, Value};
fn hex(value: u16) -> Option<u16> {
    match value {
        48..=57 => Some(value - 48),
        65..=70 => Some(value - 65 + 10),
        97..=102 => Some(value - 97 + 10),
        _ => None,
    }
}
/// Match the native sanitizer in UTF-16 units, including escaped non-BMP characters.
pub(super) fn without_data_payload(raw: &str) -> String {
    let units = raw.encode_utf16().collect::<Vec<_>>();
    let mut input = units.as_slice();
    if input.first().is_some_and(|value| matches!(value, 34 | 39)) {
        input = &input[1..];
    }
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < input.len() {
        if input[index] == 92
            && index + 1 < input.len()
            && !matches!(input[index + 1], 10 | 13 | 0x2028 | 0x2029)
        {
            let code = input[index + 1];
            let width = match code {
                120 | 88 => 2,
                117 | 85 => 4,
                _ => 0,
            };
            if width > 0 && index + 2 + width <= input.len() {
                let value = input[index + 2..index + 2 + width]
                    .iter()
                    .try_fold(0_u16, |total, value| {
                        hex(*value).map(|value| total * 16 + value)
                    });
                if let Some(value) = value {
                    decoded.push(value);
                    index += 2 + width;
                    continue;
                }
            }
            decoded.push(match code {
                116 => 9,
                110 => 10,
                114 => 13,
                34 => 34,
                92 => 92,
                _ => 1,
            });
            index += 2;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    decoded.retain(|value| !matches!(value, 9 | 10 | 13));
    let first = decoded
        .iter()
        .position(|value| *value > 32)
        .unwrap_or(decoded.len());
    let value = &decoded[first..];
    if value.len() < 5
        || !value[..5]
            .iter()
            .zip("data:".encode_utf16())
            .all(|(left, right)| left.eq_ignore_ascii_case_u16(right))
    {
        return raw.into();
    }
    let end = value[5..]
        .iter()
        .position(|value| matches!(value, 44 | 59))
        .map_or(value.len(), |end| end + 5);
    let media = String::from_utf16_lossy(&value[5..end]);
    let media = crate::native_goal::trim(&media);
    let parts = media.split('/').collect::<Vec<_>>();
    let valid = parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 64
                && part
                    .bytes()
                    .all(|value| value.is_ascii_alphanumeric() || b"_.+-".contains(&value))
        });
    format!("data:{},…", if valid { media } else { "" })
}
trait AsciiUnit {
    fn eq_ignore_ascii_case_u16(&self, other: u16) -> bool;
}
impl AsciiUnit for u16 {
    fn eq_ignore_ascii_case_u16(&self, other: u16) -> bool {
        let lower = |value: u16| {
            if (65..=90).contains(&value) {
                value + 32
            } else {
                value
            }
        };
        lower(*self) == lower(other)
    }
}
fn slice_utf16(value: &str, limit: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
}
pub(super) struct Loaded<'a> {
    pub before_url: &'a str,
    pub after_url: &'a str,
    pub generation: u64,
    pub after_generation: u64,
    pub loading: bool,
    pub title: Option<&'a str>,
}
pub(super) fn loaded(input: Loaded<'_>) -> Option<Value> {
    if input.before_url == "about:blank"
        || input.before_url.starts_with("chrome-error://")
        || input.generation != input.after_generation
        || input.loading
        || input.before_url != input.after_url
    {
        return None;
    }
    Some(
        json!({"_tag":"Success","url":slice_utf16(&without_data_payload(input.before_url),2048),"title":slice_utf16(input.title.unwrap_or(""),512)}),
    )
}
pub(super) fn report(thread: &str, tab: &str, nav: Value, history: Option<&Value>) -> Value {
    let index = history
        .and_then(|history| history["currentIndex"].as_f64())
        .unwrap_or(0.0);
    let count = history
        .and_then(|history| history["entries"].as_array())
        .map_or(0, Vec::len);
    json!({"threadId":thread,"tabId":tab,"serverControlled":true,"navStatus":nav,"canGoBack":index>0.0,"canGoForward":index<(count as f64)-1.0})
}
/// An observation is committed only while the same live actor and watch generation still own it.
pub(super) async fn sync(
    browser: &crate::server_browser::Browsers,
    inner: &std::sync::Mutex<super::Inner>,
    changed: &crate::terminal::Notifier,
    thread: &str,
    tab: &str,
    watch: &tokio::sync::watch::Receiver<crate::server_browser::Navigation>,
) {
    let before = *watch.borrow();
    let Ok(history) = browser
        .command(thread, tab, "Page.getNavigationHistory", json!({}))
        .await
    else {
        return;
    };
    let before_url = history["currentIndex"]
        .as_u64()
        .and_then(|index| {
            history["entries"]
                .as_array()
                .and_then(|entries| entries.get(index as usize))
        })
        .and_then(|entry| entry["url"].as_str())
        .map(str::to_string);
    let Ok(metadata)=browser.command(thread,tab,"Runtime.evaluate",json!({"expression":"({url:location.href,title:document.title,readyState:document.readyState})","returnByValue":true,"timeout":1000})).await else{return;};
    let metadata = &metadata["result"]["value"];
    let Some(url) = metadata["url"].as_str() else {
        return;
    };
    let after = *watch.borrow();
    if before.revision != after.revision
        || !browser
            .connection(thread, tab)
            .await
            .is_some_and(|(_, connected)| connected)
        || !browser
            .navigation(thread, tab)
            .await
            .is_some_and(|current| current.same_channel(watch))
    {
        return;
    }
    let Some(nav) = loaded(Loaded {
        before_url: before_url.as_deref().unwrap_or(url),
        after_url: url,
        generation: before.generation,
        after_generation: after.generation,
        loading: before.loading || after.loading || metadata["readyState"] != "complete",
        title: metadata["title"].as_str(),
    }) else {
        return;
    };
    let report = report(thread, tab, nav, Some(&history));
    let mut inner = inner.lock().unwrap();
    let Some(current) = inner.kernel.get(thread, tab) else {
        return;
    };
    if current["navStatus"] == report["navStatus"]
        && current["canGoBack"] == report["canGoBack"]
        && current["canGoForward"] == report["canGoForward"]
    {
        return;
    }
    if inner
        .kernel
        .report_status(&report, || crate::store::iso(crate::store::now_ms()))
        .is_ok()
    {
        inner.flush();
        drop(inner);
        crate::terminal::bump(changed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_native_loaded_reports_and_redaction_match() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/preview-navigation-4288.json"
        ))
        .unwrap();
        assert_eq!(fixture["cases"].as_array().unwrap().len(), 409);
        for (case_number, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let actual = loaded(Loaded {
                before_url: input["beforeUrl"].as_str().unwrap(),
                after_url: input["afterUrl"].as_str().unwrap(),
                generation: input["generation"].as_u64().unwrap(),
                after_generation: input["afterGeneration"].as_u64().unwrap(),
                loading: input["loading"] == true,
                title: if input["titleFails"] == true {
                    None
                } else {
                    input["title"].as_str()
                },
            })
            .map(|nav| {
                report(
                    "thread",
                    "tab",
                    nav,
                    if input["historyFails"] == true {
                        None
                    } else {
                        input.get("history")
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
            assert_eq!(
                json!(actual),
                case["expected"],
                "native report case {case_number}"
            );
        }
        for case in fixture["sanitizers"].as_array().unwrap() {
            assert_eq!(
                without_data_payload(case["input"].as_str().unwrap()),
                case["expected"].as_str().unwrap()
            );
        }
    }
}
