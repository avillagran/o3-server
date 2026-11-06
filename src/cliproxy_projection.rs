//! Pure native hub account projection, compared with the complete pinned readAccounts method.
use serde_json::{json, Value};
use std::sync::LazyLock;
fn trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn number(value: f64) -> Value {
    serde_json::from_str(ryu_js::Buffer::new().format(value)).unwrap()
}
fn percent(value: f64) -> Value {
    number(if value.is_finite() {
        value.clamp(0., 100.)
    } else {
        0.
    })
}
fn civil_days(mut year: i64, month: i64, day: i64) -> i64 {
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}
fn iso_millis(value: f64) -> Option<String> {
    if !value.is_finite() || value.abs() > 8_640_000_000_000_000. {
        return None;
    }
    let millis = value.trunc() as i64;
    let day = millis.div_euclid(86_400_000);
    let time = millis.rem_euclid(86_400_000);
    let z = day + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{}{:06}", if year < 0 { "-" } else { "+" }, year.abs())
    };
    Some(format!(
        "{year}-{month:02}-{date:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000
    ))
}
static ISO: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?P<year>[+-][0-9]{6}|[0-9]{4})(?:-(?P<month>[0-9]{2})(?:-(?P<day>[0-9]{2}))?)?(?:[Tt ](?P<hour>[0-9]{2}):(?P<minute>[0-9]{2})(?::(?P<second>[0-9]{2})(?:\.(?P<fraction>[0-9]+))?)?)?(?P<zone>[Zz]|[+-][0-9]{2}:?[0-9]{2})?$").unwrap()
});
fn date_string(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if let Some(parts) = ISO.captures(value) {
        let get = |name: &str, default: i64| -> Option<i64> {
            parts
                .name(name)
                .map_or(Some(default), |value| value.as_str().parse().ok())
        };
        if parts.name("year")?.as_str() == "-000000" {
            return None;
        }
        let year = get("year", 0)?;
        let month = get("month", 1)?;
        let day = get("day", 1)?;
        let hour = get("hour", 0)?;
        let minute = get("minute", 0)?;
        let second = get("second", 0)?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 24
            || minute > 59
            || second > 59
            || (parts.name("hour").is_some() && parts.name("day").is_none())
        {
            return None;
        }
        let fraction = parts.name("fraction").map_or("", |value| value.as_str());
        if hour == 24 && (minute != 0 || second != 0 || fraction.bytes().any(|value| value != b'0'))
        {
            return None;
        }
        let fraction = format!("{fraction:0<3}");
        let millis: f64 = fraction[..3].parse().ok()?;
        let mut offset = 0;
        if let Some(zone) = parts
            .name("zone")
            .map(|zone| zone.as_str())
            .filter(|zone| zone.len() > 1)
        {
            parts.name("hour")?;
            let digits = zone[1..].replace(':', "");
            let h: i64 = digits[..2].parse().ok()?;
            let m: i64 = digits[2..].parse().ok()?;
            if h > 23 || m > 59 {
                return None;
            }
            offset = (h * 60 + m) * if zone.starts_with('-') { -1 } else { 1 };
        }
        return iso_millis(
            (civil_days(year, month, day) * 86_400_000
                + (hour * 3600 + minute * 60 + second - offset * 60) * 1000) as f64
                + millis,
        );
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .and_then(|date| iso_millis(date.timestamp_millis() as f64))
}
fn epoch_seconds(value: &Value) -> Option<String> {
    let value = value.as_f64()?;
    (value > 0.).then(|| iso_millis(value * 1000.)).flatten()
}
fn plan(value: &str) -> Option<&'static str> {
    Some(match value {
        "free" => "ChatGPT Free Subscription",
        "go" => "ChatGPT Go Subscription",
        "plus" => "ChatGPT Plus Subscription",
        "pro" => "ChatGPT Pro 200 Subscription",
        "prolite" => "ChatGPT Pro 100 Subscription",
        "promax" => "ChatGPT Pro 500 Subscription",
        "self_serve_business_prolite" => "ChatGPT Business Premium Subscription",
        "team" | "self_serve_business_usage_based" | "business" => "ChatGPT Business Subscription",
        "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" | "enterprise" => {
            "ChatGPT Enterprise Subscription"
        }
        "edu" | "edu_plus" | "edu_pro" => "ChatGPT Edu Subscription",
        "unknown" => "ChatGPT Subscription",
        _ => return None,
    })
}
fn ratio(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let used = used?;
    let limit = limit?;
    if !used.is_finite() || !limit.is_finite() || used < 0. || limit <= 0. {
        return None;
    }
    let value = used / limit * 100.;
    value.is_finite().then(|| value.clamp(0., 100.))
}
fn natural(value: &Value) -> Option<f64> {
    let value = value.as_f64()?;
    (value.is_finite() && (0. ..=9_007_199_254_740_991.).contains(&value) && value.fract() == 0.)
        .then_some(value)
}
fn money(value: &Value) -> Option<(f64, &str, f64)> {
    let currency = value["currency"].as_str()?;
    if currency.is_empty() {
        return None;
    }
    Some((
        natural(&value["amount_minor"])?,
        currency,
        natural(&value["exponent"])?,
    ))
}
fn spend(usage: &Value) -> Option<f64> {
    let budget = &usage["spend"];
    if budget["enabled"] == true {
        if let (Some((used, currency, scale)), Some((limit, other_currency, other_scale))) =
            (money(&budget["used"]), money(&budget["limit"]))
        {
            if !trim(currency).is_empty()
                && trim(currency) == trim(other_currency)
                && scale == other_scale
            {
                if let Some(value) = ratio(Some(used), Some(limit)) {
                    return Some(value);
                }
            }
        }
    }
    let extra = &usage["extra_usage"];
    if extra["is_enabled"] != true {
        return None;
    }
    let value = ratio(
        extra["used_credits"].as_f64(),
        extra["monthly_limit"].as_f64(),
    )
    .or_else(|| extra["utilization"].as_f64())?;
    (value.is_finite() && value >= 0.).then(|| value.clamp(0., 100.))
}
fn scoped_id(name: &str) -> String {
    let mut id = String::from("seven_day_");
    let mut bad = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            id.push(c);
            bad = false;
        } else if !bad {
            id.push('_');
            bad = true;
        }
    }
    id
}
fn windows(codex: bool, usage: &Value, plan: Option<&str>) -> Vec<Value> {
    let mut windows = Vec::new();
    if codex {
        for (id, key, fallback) in [
            (
                "primary",
                "primary_window",
                if matches!(plan, Some("free" | "go")) {
                    43200.
                } else {
                    300.
                },
            ),
            ("secondary", "secondary_window", 10080.),
        ] {
            let value = &usage["rate_limit"][key];
            let Some(used) = value["used_percent"].as_f64() else {
                continue;
            };
            let duration = value["limit_window_seconds"]
                .as_f64()
                .map_or(fallback, |value| value / 60.);
            let (kind, label) = if duration >= 43200. {
                ("monthly", "Monthly")
            } else if duration >= 10080. {
                ("weekly", "Weekly")
            } else {
                ("session", "Session")
            };
            let mut window = json!({"id":id,"kind":kind,"label":label,"usedPercent":percent(used),"windowDurationMins":number(duration)});
            if let Some(reset) = epoch_seconds(&value["reset_at"]) {
                window["resetsAt"] = json!(reset);
            }
            windows.push(window);
        }
    } else {
        for (id, kind, label, duration) in [
            ("five_hour", "session", "Session", 300),
            ("seven_day", "weekly", "Weekly", 10080),
        ] {
            let value = &usage[id];
            let Some(used) = value["utilization"].as_f64() else {
                continue;
            };
            let mut window = json!({"id":id,"kind":kind,"label":label,"windowDurationMins":duration,"usedPercent":percent(used)});
            if let Some(reset) = value["resets_at"].as_str().and_then(date_string) {
                window["resetsAt"] = json!(reset);
            }
            windows.push(window);
        }
        for entry in usage["limits"].as_array().into_iter().flatten() {
            if entry["kind"] != "weekly_scoped" {
                continue;
            }
            let (Some(name), Some(used)) = (
                entry["scope"]["model"]["display_name"].as_str(),
                entry["percent"].as_f64(),
            ) else {
                continue;
            };
            let mut window = json!({"id":scoped_id(name),"kind":"weekly","label":format!("Weekly · {name}"),"windowDurationMins":10080,"usedPercent":percent(used)});
            if let Some(reset) = entry["resets_at"].as_str().and_then(date_string) {
                window["resetsAt"] = json!(reset);
            }
            windows.push(window);
        }
        if let Some(value) = spend(usage) {
            windows.push(json!({"id":"monthly_spend","kind":"monthly","label":"Monthly spend","usedPercent":percent(value)}));
        }
    }
    let kind = |value: &Value| match value["kind"].as_str() {
        Some("session") => 0,
        Some("weekly") => 1,
        Some("monthly") => 2,
        _ => 3,
    };
    let weight = |id: &str| {
        id.bytes()
            .map(|c| if c == b'_' { 0 } else { c })
            .collect::<Vec<_>>()
    };
    windows.sort_by(|a, b| {
        kind(a)
            .cmp(&kind(b))
            .then_with(|| weight(a["id"].as_str().unwrap()).cmp(&weight(b["id"].as_str().unwrap())))
    });
    windows
}
/// Input usage/credit values have already passed the real native API schemas.
pub(crate) fn project(
    account: &Value,
    usage: Option<&Value>,
    available: Option<&[Value]>,
    checked_at: &str,
) -> Value {
    let codex = account["provider"] == "codex";
    let mut result = json!({"id":account["id"],"driver":if codex{"codex"}else{"claudeAgent"}});
    if let Some(email) = account["email"].as_str().filter(|value| !value.is_empty()) {
        result["email"] = json!(email);
    }
    if let Some(workspace) = account["id_token"]["chatgpt_account_id"]
        .as_str()
        .map(trim)
        .filter(|value| !value.is_empty())
    {
        result["workspaceId"] = json!(workspace);
    }
    let Some(usage) = usage else {
        result["usageLimits"] = json!({"checkedAt":checked_at,"windows":[],"unavailable":{"reason":"probeFailed","message":"The hub could not read this account's usage."}});
        return result;
    };
    let plan_type = usage["plan_type"]
        .as_str()
        .or_else(|| account["id_token"]["plan_type"].as_str())
        .or_else(|| account["id_token"]["chatgpt_plan_type"].as_str());
    if !codex {
        result["plan"] = json!("Claude Subscription");
    } else if let Some(plan) = plan_type.and_then(plan) {
        result["plan"] = json!(plan);
    }
    result["usageLimits"] =
        json!({"checkedAt":checked_at,"windows":windows(codex,usage,plan_type)});
    if codex {
        if let Some(available) = available {
            let mut credits = json!({"availableCount":available.len()});
            if let Some(next) = available.first() {
                credits["nextCreditId"] = next["id"].clone();
                if let Some(at) = next["expires_at"].as_str().and_then(date_string) {
                    credits["nextExpiresAt"] = json!(at);
                }
            }
            result["usageLimits"]["resetCredits"] = credits;
        }
    }
    result
}
