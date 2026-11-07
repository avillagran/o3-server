//! Native Tailscale Serve inspection and selective removal decisions.
//! No command runs in this leaf; physical enforcement supplies a bounded runner.
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::Ipv6Addr, sync::LazyLock};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeRoute {
    #[serde(serialize_with = "serialize_integer")]
    pub serve_port: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}
fn serialize_integer<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if *value >= i64::MIN as f64 && *value < i64::MAX as f64 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Target {
    pub host: String,
    pub port: u16,
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c| matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn normalize_host(host: &str) -> String {
    let lower = host.to_lowercase();
    let lower = lower.strip_prefix('[').unwrap_or(&lower);
    lower
        .strip_suffix(']')
        .unwrap_or(lower)
        .trim_end_matches('.')
        .into()
}
pub(crate) fn js_integer(text: &str) -> Option<f64> {
    let text = js_trim(text);
    let value = if text.is_empty() {
        0.0
    } else if let Some((digits, radix)) = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map(|s| (s, 16))
        .or_else(|| {
            text.strip_prefix("0b")
                .or_else(|| text.strip_prefix("0B"))
                .map(|s| (s, 2))
        })
        .or_else(|| {
            text.strip_prefix("0o")
                .or_else(|| text.strip_prefix("0O"))
                .map(|s| (s, 8))
        })
    {
        if digits.is_empty() {
            return None;
        }
        digits.chars().try_fold(0.0, |value, c| {
            c.to_digit(radix)
                .map(|digit| value * f64::from(radix) + f64::from(digit))
        })?
    } else {
        static DECIMAL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").unwrap()
        });
        if !DECIMAL.is_match(text) {
            return None;
        }
        text.parse::<f64>().ok()?
    };
    (value.is_finite() && value.fract() == 0.0).then_some(value)
}
fn entries(value: &Value) -> Vec<(&String, &Value)> {
    let mut entries = value.as_object().into_iter().flatten().collect::<Vec<_>>();
    let index = |key: &str| {
        key.parse::<u32>()
            .ok()
            .filter(|number| *number < u32::MAX && number.to_string() == key)
    };
    entries.sort_by_key(|(key, _)| index(key).map_or((1, 0), |index| (0, index)));
    entries
}
fn optional_string(object: &Value, key: &str) -> bool {
    object.get(key).is_none_or(Value::is_string)
}
fn valid_config(value: &Value) -> bool {
    value.is_object()
        && value.get("TCP").is_none_or(|tcp| {
            tcp.as_object().is_some_and(|ports| {
                ports
                    .values()
                    .all(|handler| handler.is_object() && optional_string(handler, "TCPForward"))
            })
        })
        && value.get("Web").is_none_or(|web| {
            web.as_object().is_some_and(|sites| {
                sites.values().all(|site| {
                    site.is_object()
                        && site.get("Handlers").is_none_or(|handlers| {
                            handlers.as_object().is_some_and(|handlers| {
                                handlers.values().all(|handler| {
                                    handler.is_object() && optional_string(handler, "Proxy")
                                })
                            })
                        })
                })
            })
        })
}
fn reaches(hostname: &str, bind_host: &str) -> Option<bool> {
    let mut address = normalize_host(hostname);
    if address.contains(':') {
        if let Ok(ip) = address.parse::<Ipv6Addr>() {
            address = ip.to_string();
        }
    }
    if let Some(mapped) = address.strip_prefix("::ffff:") {
        if let Ok(ip) = mapped.parse::<std::net::Ipv4Addr>() {
            address = ip.to_string();
        } else if let Some((high, low)) = mapped.split_once(':') {
            if let (Ok(high), Ok(low)) =
                (u16::from_str_radix(high, 16), u16::from_str_radix(low, 16))
            {
                address = format!("{}.{}.{}.{}", high >> 8, high & 255, low >> 8, low & 255);
            }
        }
    }
    // Go accepts only four strict decimal octets; names and abbreviated IPv4 resolve elsewhere.
    let literal_v4 = address.split('.').count() == 4
        && address.split('.').all(|part| {
            !part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|c| c.is_ascii_digit())
                && part.parse::<u8>().is_ok()
        });
    if !address.contains(':') && !literal_v4 {
        return Some(true);
    }
    if address == "::" {
        return matches!(bind_host, "127.0.0.1" | "::1").then_some(true);
    }
    let address = if address == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &address
    };
    (address == bind_host).then_some(false)
}
fn reached(url: &str, target: &Target) -> Option<Option<String>> {
    static DIALED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(https?|tcp)://(?:[^@/?#]*@)?(\[[^\]]*\]|[^/?#:]*)(?::([0-9]*))?")
            .unwrap()
    });
    let found = DIALED.captures(url)?;
    let scheme = found.get(1)?.as_str();
    let host = found.get(2)?.as_str();
    let port = found
        .get(3)
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty())
        .unwrap_or(if scheme.eq_ignore_ascii_case("https") {
            "443"
        } else {
            "80"
        });
    if js_integer(port)? != f64::from(target.port) {
        return None;
    }
    reaches(host, &target.host).map(|ambiguous| ambiguous.then(|| host.to_owned()))
}
fn expand_proxy(proxy: &str) -> String {
    if !proxy.is_empty() && proxy.bytes().all(|c| c.is_ascii_digit()) {
        format!("http://127.0.0.1:{proxy}")
    } else if let Some(rest) = proxy.strip_prefix("https+insecure://") {
        format!("https://{rest}")
    } else if proxy.starts_with("http://")
        || proxy.starts_with("https://")
        || proxy.starts_with("unix:")
    {
        proxy.into()
    } else {
        format!("http://{proxy}")
    }
}
fn routes_in(config: &Value, owner: Option<&str>, target: &Target) -> Vec<ServeRoute> {
    let mut routes = Vec::new();
    for (port, handler) in entries(&config["TCP"]) {
        let Some(forward) = handler["TCPForward"]
            .as_str()
            .filter(|forward| !forward.starts_with("unix:"))
        else {
            continue;
        };
        if let (Some(ambiguous), Some(serve_port)) = (
            reached(&format!("tcp://{forward}"), target),
            js_integer(port),
        ) {
            routes.push(ServeRoute {
                serve_port,
                mount: None,
                host: None,
                ambiguous,
                owner: owner.map(str::to_owned),
            });
        }
    }
    for (host_port, site) in entries(&config["Web"]) {
        let separator = host_port.rfind(':');
        let (host, port) = separator.map_or(
            (
                &host_port[..host_port
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index)],
                host_port.as_str(),
            ),
            |index| (&host_port[..index], &host_port[index + 1..]),
        );
        let Some(serve_port) = js_integer(port) else {
            continue;
        };
        for (mount, handler) in entries(&site["Handlers"]) {
            if let Some(ambiguous) = handler["Proxy"]
                .as_str()
                .and_then(|proxy| reached(&expand_proxy(proxy), target))
            {
                routes.push(ServeRoute {
                    serve_port,
                    mount: Some(mount.clone()),
                    host: Some(host.into()),
                    ambiguous,
                    owner: owner.map(str::to_owned),
                });
            }
        }
    }
    routes
}
pub fn parse_serve_routes(raw: &str, target: &Target) -> Option<Vec<ServeRoute>> {
    let status: Value = serde_json::from_str(raw).ok()?;
    if status.is_null() {
        return Some(Vec::new());
    }
    if !valid_config(&status)
        || ["Services", "Foreground"].iter().any(|key| {
            status.get(key).is_some_and(|groups| {
                groups
                    .as_object()
                    .is_none_or(|groups| groups.values().any(|config| !valid_config(config)))
            })
        })
    {
        return None;
    }
    let mut routes = routes_in(&status, None, target);
    for (name, config) in entries(&status["Services"]) {
        routes.extend(routes_in(config, Some(name), target));
    }
    for (_, config) in entries(&status["Foreground"]) {
        routes.extend(routes_in(config, Some("foreground"), target));
    }
    Some(routes)
}
pub fn off_args(route: &ServeRoute) -> Option<Vec<String>> {
    if route.owner.as_deref() == Some("foreground") {
        return None;
    }
    let mut args = vec!["serve".into()];
    if let Some(owner) = &route.owner {
        args.push(format!("--service={owner}"));
    }
    let port = ryu_js::Buffer::new()
        .format_finite(route.serve_port)
        .to_owned();
    match &route.mount {
        None => args.push(format!("--tcp={port}")),
        Some(mount) => {
            args.push(format!("--https={port}"));
            args.push(format!("--set-path={mount}"));
        }
    }
    args.push("off".into());
    Some(args)
}
pub fn parse_status(raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if !value.is_object() || value.get("Self").is_some_and(|value| !value.is_object()) {
        return None;
    }
    let own = &value["Self"];
    let magic = own["DNSName"]
        .as_str()
        .map(js_trim)
        .map(|name| name.strip_suffix('.').unwrap_or(name))
        .filter(|name| !name.is_empty());
    let ips = own["TailscaleIPs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|ip| {
            let parts = ip.split('.').collect::<Vec<_>>();
            if parts.len() != 4 {
                return false;
            }
            let parsed = parts
                .iter()
                .map(|part| {
                    let part = js_trim(part);
                    let digits = part
                        .trim_start_matches(['+', '-'])
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>();
                    if digits.is_empty() {
                        None
                    } else {
                        format!("{}{}", if part.starts_with('-') { "-" } else { "" }, digits)
                            .parse::<i64>()
                            .ok()
                    }
                })
                .collect::<Option<Vec<_>>>();
            parsed.is_some_and(|p| {
                p[0] == 100 && (64..=127).contains(&p[1]) && p.iter().all(|v| (0..=255).contains(v))
            })
        })
        .collect::<Vec<_>>();
    let https = own["CapMap"]
        .as_object()
        .map(|map| map.contains_key("https"));
    Some(json!({"magicDnsName":magic,"tailnetIpv4Addresses":ips,"httpsServeEnabled":https}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_native_tailscale_parser_and_selective_arguments() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-tailscale-4262.json"
        ))
        .unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let target: Target = serde_json::from_value(case["target"].clone()).unwrap();
            let actual =
                serde_json::to_value(parse_serve_routes(case["raw"].as_str().unwrap(), &target))
                    .unwrap();
            assert_eq!(actual, case["expected"], "Serve case {index}: {case}");
        }
        for case in fixture["statuses"].as_array().unwrap() {
            assert_eq!(
                parse_status(case["raw"].as_str().unwrap()).unwrap_or(Value::Null),
                case["expected"],
                "status: {case}"
            );
        }
        for case in fixture["off"].as_array().unwrap() {
            let route: ServeRoute = serde_json::from_value(case["route"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(off_args(&route)).unwrap(),
                case["expected"],
                "off: {case}"
            );
        }
    }
}
