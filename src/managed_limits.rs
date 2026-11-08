//! Pure managed client and reachability rules from nightly 4262.
//! Consumer readiness stays gated until transport and Tailscale enforcement exist.
use crate::managed_policy::{Client, Document};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum AuthPolicy {
    #[serde(rename = "desktop-managed-local")]
    Desktop,
    #[serde(rename = "loopback-browser")]
    LocalBrowser,
    #[serde(rename = "remote-reachable")]
    Remote,
    #[serde(rename = "unsafe-no-auth")]
    Unsafe,
}

pub fn allows_auth(document: Option<&Document>, auth: AuthPolicy) -> bool {
    let Some(document) = document else {
        return true;
    };
    let client = match auth {
        AuthPolicy::Unsafe => return false,
        AuthPolicy::Desktop => Client::Desktop,
        AuthPolicy::LocalBrowser => Client::LocalBrowser,
        AuthPolicy::Remote => Client::Remote,
    };
    document
        .limits
        .as_ref()
        .and_then(|limits| limits.clients.as_ref())
        .is_none_or(|clients| clients.0.contains(&client))
}

pub fn allows_remote(document: Option<&Document>) -> bool {
    allows_auth(document, AuthPolicy::Remote)
}

pub fn allows_connect(document: Option<&Document>) -> bool {
    document
        .and_then(|document| document.limits.as_ref())
        .is_none_or(|limits| limits.connect != Some(false))
        && allows_remote(document)
}

/// Match Node's literal-only bind check, including its hostname normalization.
pub fn loopback_bind_address(host: Option<&str>) -> Option<String> {
    let Some(host) = host else {
        return Some("127.0.0.1".into());
    };
    let lower = host.to_lowercase();
    let address = lower.strip_prefix('[').unwrap_or(&lower);
    let address = address
        .strip_suffix(']')
        .unwrap_or(address)
        .trim_end_matches('.');
    if address == "localhost" {
        return Some("127.0.0.1".into());
    }
    if address == "::1"
        || address
            .parse::<std::net::Ipv4Addr>()
            .ok()
            .is_some_and(|ip| ip.octets()[0] == 127)
    {
        Some(address.into())
    } else {
        None
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reachability {
    pub auth_policy: AuthPolicy,
    pub host: Option<String>,
    pub tailscale_serve_enabled: bool,
    pub private_network_only: Option<bool>,
}

pub fn check_reachability(
    document: Option<&Document>,
    input: &Reachability,
) -> Result<String, String> {
    let organization = document
        .map(|document| document.organization.as_str())
        .unwrap_or("Your organization");
    let refuse = |action: &str, detail: Option<&str>| {
        format!(
            "{organization}'s policy does not allow {action}{}.",
            detail
                .map(|detail| format!(", {detail}"))
                .unwrap_or_default()
        )
    };
    let cannot_listen = || {
        let detail = if input.private_network_only == Some(true) {
            "so the background service cannot listen on private networks; set T3CODE_HOST=127.0.0.1 in its service.env".into()
        } else {
            format!(
                "so the server cannot listen on {}",
                match input.host.as_deref() {
                    Some("") => "every interface",
                    Some(host) => host,
                    None => "this host",
                }
            )
        };
        refuse("remote clients", Some(&detail))
    };
    if !allows_auth(document, input.auth_policy) {
        return Err(match input.auth_policy {
            AuthPolicy::Remote => cannot_listen(),
            AuthPolicy::Desktop => refuse("the desktop app", None),
            AuthPolicy::LocalBrowser => refuse("browsers on this machine", None),
            AuthPolicy::Unsafe => refuse("running without authentication", None),
        });
    }
    if allows_remote(document) {
        return Ok(input.host.clone().unwrap_or_else(|| "127.0.0.1".into()));
    }
    if input.tailscale_serve_enabled {
        return Err(refuse(
            "remote clients",
            Some("so Tailscale Serve cannot be turned on"),
        ));
    }
    loopback_bind_address(input.host.as_deref()).ok_or_else(cannot_listen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn matches_pinned_native_client_and_reachability_cases() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/managed-limits-4262.json"))
                .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let document: Option<Document> =
                serde_json::from_value(case["document"].clone()).unwrap();
            let input: Reachability = serde_json::from_value(case["input"].clone()).unwrap();
            assert_eq!(
                allows_auth(document.as_ref(), input.auth_policy),
                case["allowsAuth"],
                "{case}"
            );
            assert_eq!(
                allows_connect(document.as_ref()),
                case["allowsConnect"],
                "{case}"
            );
            let actual = match check_reachability(document.as_ref(), &input) {
                Ok(host) => serde_json::json!({"host":host}),
                Err(error) => serde_json::json!({"error":error}),
            };
            assert_eq!(actual, case["result"], "{case}");
        }
    }
}
