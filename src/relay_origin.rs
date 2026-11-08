//! Native managed-endpoint webhook URLs; this does not authenticate relay deliveries.
use crate::native_goal::trim;
use serde_json::Value;
use std::path::Path;

pub(crate) fn hook_base_url(relay_url: &str, tunnel_name: Option<&str>) -> Option<String> {
    let endpoint = tunnel_name?.rsplit('-').next()?;
    let relay = relay_url.trim_end_matches('/');
    if relay.is_empty()
        || endpoint.len() != 16
        || !endpoint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some(format!("{relay}/v1/hooks/{endpoint}"))
}

fn decoded_tunnel_name(config: &Value) -> Option<Option<String>> {
    let config = config.as_object()?;
    if !matches!(
        config.get("providerKind")?.as_str()?,
        "manual" | "cloudflare_tunnel" | "t3_relay"
    ) {
        return None;
    }
    if trim(config.get("connectorToken")?.as_str()?).is_empty() {
        return None;
    }
    for key in ["tunnelId", "tunnelName"] {
        if let Some(value) = config.get(key) {
            if trim(value.as_str()?).is_empty() {
                return None;
            }
        }
    }
    Some(
        config
            .get("tunnelName")
            .and_then(Value::as_str)
            .map(|value| trim(value).into()),
    )
}

pub(crate) fn from_private_directory(data_dir: &Path) -> Option<String> {
    let secrets = data_dir.join("secrets");
    let relay = std::fs::read(secrets.join("cloud-relay-url.bin")).ok()?;
    let config = std::fs::read(secrets.join("cloud-endpoint-runtime-config.bin")).ok()?;
    let config: Value = serde_json::from_str(&String::from_utf8_lossy(&config)).ok()?;
    let tunnel = decoded_tunnel_name(&config)?;
    hook_base_url(&String::from_utf8_lossy(&relay), tunnel.as_deref())
}

pub(crate) fn decorate(mut task: Value, base: Option<&str>) -> Value {
    if task["schedule"]["type"] != "webhook" || !task["webhook"].is_object() {
        return task;
    }
    let url = base.and_then(|base| {
        task["webhook"]["path"]
            .as_str()?
            .strip_prefix("/api/hooks/")
            .map(|path| format!("{base}/{path}"))
    });
    task["webhook"]["url"] = url.map(Value::String).unwrap_or(Value::Null);
    task
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_origin_and_runtime_configuration_oracle() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/relay-origin-4262.json")).unwrap();
        for case in fixture["origins"].as_array().unwrap() {
            assert_eq!(
                serde_json::json!(hook_base_url(
                    case["relayUrl"].as_str().unwrap(),
                    case["tunnelName"].as_str()
                )),
                case["expected"],
                "{case}"
            );
        }
        for case in fixture["configs"].as_array().unwrap() {
            assert_eq!(
                decoded_tunnel_name(&case["input"])
                    .map(|tunnel| serde_json::json!({"tunnelName":tunnel}))
                    .unwrap_or(Value::Null),
                case["expected"],
                "{case}"
            );
        }
    }
}
