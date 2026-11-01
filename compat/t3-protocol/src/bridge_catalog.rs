//! Credential-free bridge discovery. The host must authenticate the publisher and
//! recipient BEFORE calling this reducer. Discovery never grants T3 access.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, String>;
const MAX_FUTURE_ISSUED_AT_SKEW_MS: u64 = 30_000;
fn error<T>() -> Result<T> {
    Err("invalid_bridge_catalog".into())
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .ok_or_else(|| "invalid_bridge_catalog".into())
}
fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    let Some(o) = v.as_object() else {
        return error();
    };
    if o.keys().any(|k| !allowed.contains(&k.as_str())) {
        return error();
    }
    Ok(())
}
fn number(v: &Value, k: &str) -> Result<u64> {
    v[k].as_u64().ok_or_else(|| "invalid_bridge_catalog".into())
}
fn route(v: &Value) -> Result<Value> {
    fields(v, &["id", "url", "priority"])?;
    let id = text(v, "id")?;
    let raw = v["url"]
        .as_str()
        .filter(|s| s.len() <= 2048)
        .ok_or("invalid_bridge_catalog")?;
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid_bridge_catalog")?;
    let Some(host) = url.host_str() else {
        return error();
    };
    let host = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let invalid_ip = host.parse::<std::net::IpAddr>().is_ok_and(|ip| {
        ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || match ip {
                std::net::IpAddr::V4(ip) => ip.is_link_local(),
                std::net::IpAddr::V6(ip) => {
                    ip.is_unicast_link_local()
                        || ip.to_ipv4_mapped().is_some_and(|v| {
                            v.is_loopback() || v.is_unspecified() || v.is_link_local()
                        })
                }
            }
    });
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || host == "localhost"
        || host.ends_with(".localhost")
        || invalid_ip
        || url.port() == Some(0)
    {
        return Err("bridge_route_not_portable".into());
    }
    Ok(json!({"id":id,"url":url.as_str(),"priority":number(v,"priority")?}))
}
/// Build a client-specific manifest from the credential-free desktop view and
/// descriptor-verified gateway routes. `authorized` MUST come from the bridge's
/// authenticated client grant, never a request parameter or the desktop's access.
pub fn publish(
    view: &Value,
    authorized: &Value,
    endpoints: &Value,
    context: &Value,
) -> Result<Value> {
    fields(context, &["bridgeId", "clientId", "revision", "now"])?;
    let bridge = text(context, "bridgeId")?;
    let client = text(context, "clientId")?;
    let now = number(context, "now")?;
    let expires = now.checked_add(600_000).ok_or("invalid_bridge_catalog")?;
    let grants = authorized
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or("invalid_bridge_catalog")?;
    let mut allowed = BTreeSet::new();
    for id in grants {
        let id = id
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
            .ok_or("invalid_bridge_catalog")?;
        if !allowed.insert(id) {
            return Err("duplicate_bridge_environment".into());
        }
    }
    let view = view
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or("invalid_bridge_catalog")?;
    let endpoints = endpoints
        .as_array()
        .filter(|a| a.len() <= 2048)
        .ok_or("invalid_bridge_catalog")?;
    let mut environments = Vec::new();
    let mut seen = BTreeSet::new();
    for e in view {
        let id = text(e, "id")?;
        if !seen.insert(id) {
            return Err("duplicate_bridge_environment".into());
        }
        if e["enabled"] != true || !allowed.contains(id) {
            continue;
        }
        let mut routes = Vec::new();
        for ep in endpoints.iter().filter(|ep| ep["environmentId"] == id) {
            fields(
                ep,
                &[
                    "environmentId",
                    "descriptorEnvironmentId",
                    "id",
                    "url",
                    "priority",
                ],
            )?;
            if ep["descriptorEnvironmentId"] != id {
                return Err("t3_environment_mismatch".into());
            }
            routes.push(route(
                &json!({"id":ep["id"],"url":ep["url"],"priority":ep["priority"]}),
            )?);
        }
        environments.push(json!({"environmentId":id,"label":text(e,"name")?,"routes":routes}));
    }
    let manifest = json!({"version":1,"bridgeId":bridge,"clientId":client,"revision":number(context,"revision")?,"issuedAt":now,"expiresAt":expires,"environments":environments});
    // The same strict contract validates export and import.
    reconcile(
        &manifest,
        &Value::Null,
        &json!({"bridgeId":bridge,"clientId":client,"now":now}),
    )?;
    Ok(manifest)
}

/// Reconcile a complete authenticated snapshot with this publisher's cached slice.
/// Inputs/outputs have no token fields. Removed entries become tombstones so drafts,
/// selection and cached threads survive; local/unrelated rows are outside this slice.
pub fn reconcile(snapshot: &Value, cached: &Value, context: &Value) -> Result<Value> {
    fields(
        snapshot,
        &[
            "version",
            "bridgeId",
            "clientId",
            "revision",
            "issuedAt",
            "expiresAt",
            "environments",
        ],
    )?;
    fields(context, &["bridgeId", "clientId", "now"])?;
    let bridge = text(context, "bridgeId")?;
    let client = text(context, "clientId")?;
    if snapshot["version"] != 1
        || text(snapshot, "bridgeId")? != bridge
        || text(snapshot, "clientId")? != client
    {
        return Err("bridge_catalog_identity_mismatch".into());
    }
    let now = number(context, "now")?;
    let issued = number(snapshot, "issuedAt")?;
    let expires = number(snapshot, "expiresAt")?;
    if expires <= issued {
        return Err("bridge_catalog_invalid_window".into());
    }
    if expires - issued > 600_000 {
        return Err("bridge_catalog_ttl_exceeded".into());
    }
    if issued.saturating_sub(now) > MAX_FUTURE_ISSUED_AT_SKEW_MS {
        return Err("bridge_catalog_from_future".into());
    }
    if expires <= now {
        return Err("bridge_catalog_expired".into());
    }
    let revision = number(snapshot, "revision")?;
    let old_revision = if cached.is_null() {
        None
    } else {
        fields(
            cached,
            &[
                "version",
                "bridgeId",
                "clientId",
                "revision",
                "environments",
            ],
        )?;
        if cached["version"] != 1 || cached["bridgeId"] != bridge || cached["clientId"] != client {
            return Err("bridge_catalog_identity_mismatch".into());
        }
        Some(number(cached, "revision")?)
    };
    if old_revision.is_some_and(|old| revision < old) {
        return Err("bridge_catalog_rollback".into());
    }
    let entries = snapshot["environments"]
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or("invalid_bridge_catalog")?;
    let mut rows = BTreeMap::new();
    for e in entries {
        fields(e, &["environmentId", "label", "routes"])?;
        let id = text(e, "environmentId")?;
        let label = text(e, "label")?;
        if rows.contains_key(id) {
            return Err("duplicate_bridge_environment".into());
        }
        let routes = e["routes"]
            .as_array()
            .filter(|a| a.len() <= 16)
            .ok_or("invalid_bridge_catalog")?;
        let mut ids = BTreeSet::new();
        let mut routes = routes.iter().map(route).collect::<Result<Vec<_>>>()?;
        for r in &routes {
            if !ids.insert(text(r, "id")?) {
                return Err("duplicate_bridge_route".into());
            }
        }
        routes.sort_by_key(|r| r["priority"].as_u64().unwrap_or_default());
        rows.insert(
            id.to_string(),
            json!({"environmentId":id,"label":label,"routes":routes,"removed":false}),
        );
    }
    let mut incoming: Vec<Value> = rows.values().cloned().collect();
    if let Some(old) = old_revision {
        let previous = cached["environments"]
            .as_array()
            .ok_or("invalid_bridge_catalog")?;
        if previous.len() > 256 {
            return error();
        }
        if revision == old {
            let mut active = previous
                .iter()
                .filter(|e| e["removed"] == false)
                .cloned()
                .collect::<Vec<_>>();
            active.sort_by_key(|e| e["environmentId"].as_str().unwrap_or_default().to_string());
            if active != incoming {
                return Err("bridge_catalog_revision_reused".into());
            }
            return Ok(cached.clone());
        }
        for e in previous {
            let id = text(e, "environmentId")?;
            if !rows.contains_key(id) {
                // Keep identity and label, NEVER stale routes after withdrawal.
                incoming.push(
                    json!({"environmentId":id,"label":text(e,"label")?,"routes":[],"removed":true}),
                );
            }
        }
    }
    if incoming.len() > 256 {
        return Err("bridge_catalog_cache_full".into());
    }
    incoming.sort_by_key(|e| e["environmentId"].as_str().unwrap_or_default().to_string());
    Ok(
        json!({"version":1,"bridgeId":bridge,"clientId":client,"revision":revision,"environments":incoming}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Value {
        json!({"bridgeId":"vq-bridge","clientId":"android-client","now":1000})
    }
    fn snapshot() -> Value {
        json!({"version":1,"bridgeId":"vq-bridge","clientId":"android-client","revision":1,"issuedAt":900,"expiresAt":2000,"environments":[
            {"environmentId":"vq","label":"VQ","routes":[{"id":"public","url":"https://vq.example/","priority":1}]},
            {"environmentId":"node","label":"NODE","routes":[{"id":"gateway","url":"https://node.example:39670/","priority":1}]}
        ]})
    }
    #[test]
    fn bounded_future_issue_time_does_not_extend_expiration() {
        let mut c = context();
        c["now"] = json!(100_000);
        for ahead in [0, 1, 764, 30_000] {
            let mut s = snapshot();
            s["issuedAt"] = json!(100_000 + ahead);
            s["expiresAt"] = json!(160_000);
            let cache = reconcile(&s, &Value::Null, &c)
                .unwrap_or_else(|error| panic!("ahead={ahead}: {error}"));
            assert_eq!(cache["revision"], 1);
            let mut at_deadline = c.clone();
            at_deadline["now"] = s["expiresAt"].clone();
            assert_eq!(
                reconcile(&s, &cache, &at_deadline).unwrap_err(),
                "bridge_catalog_expired"
            );
        }
    }
    #[test]
    fn future_issue_limit_and_numeric_boundaries_are_strict() {
        let mut c = context();
        c["now"] = json!(100_000);
        let mut s = snapshot();
        s["issuedAt"] = json!(130_001);
        s["expiresAt"] = json!(160_000);
        assert_eq!(
            reconcile(&s, &Value::Null, &c).unwrap_err(),
            "bridge_catalog_from_future"
        );
        c["now"] = json!(u64::MAX - 60_000);
        s["issuedAt"] = json!(u64::MAX - 30_000);
        s["expiresAt"] = json!(u64::MAX);
        assert!(reconcile(&s, &Value::Null, &c).is_ok());
        c["now"] = json!(u64::MAX);
        assert_eq!(
            reconcile(&s, &Value::Null, &c).unwrap_err(),
            "bridge_catalog_expired"
        );
    }
    #[test]
    fn validity_window_errors_are_distinct_from_expiration() {
        let c = context();
        for (issued, expires, expected) in [
            (900, 1_000, "bridge_catalog_expired"),
            (1_000, 1_000, "bridge_catalog_invalid_window"),
            (1_001, 1_000, "bridge_catalog_invalid_window"),
            (900, 600_901, "bridge_catalog_ttl_exceeded"),
        ] {
            let mut s = snapshot();
            s["issuedAt"] = json!(issued);
            s["expiresAt"] = json!(expires);
            assert_eq!(reconcile(&s, &Value::Null, &c).unwrap_err(), expected);
        }
        let mut s = snapshot();
        s["issuedAt"] = json!(1_000);
        s["expiresAt"] = json!(601_000);
        assert!(reconcile(&s, &Value::Null, &c).is_ok());
    }
    #[test]
    fn clock_tolerance_keeps_identity_revision_and_field_guards() {
        let c = context();
        let mut s = snapshot();
        let cache = reconcile(&s, &Value::Null, &c).unwrap();
        s["issuedAt"] = json!(1_764);
        assert_eq!(reconcile(&s, &cache, &c).unwrap(), cache);
        s["revision"] = json!(0);
        assert_eq!(
            reconcile(&s, &cache, &c).unwrap_err(),
            "bridge_catalog_rollback"
        );
        s["revision"] = json!(1);
        s["environments"][0]["label"] = json!("changed");
        assert_eq!(
            reconcile(&s, &cache, &c).unwrap_err(),
            "bridge_catalog_revision_reused"
        );
        s["clientId"] = json!("another-client");
        assert_eq!(
            reconcile(&s, &cache, &c).unwrap_err(),
            "bridge_catalog_identity_mismatch"
        );
        let mut override_context = c.clone();
        override_context["futureSkewMs"] = json!(u64::MAX);
        assert_eq!(
            reconcile(&snapshot(), &cache, &override_context).unwrap_err(),
            "invalid_bridge_catalog"
        );
    }
    #[test]
    fn publisher_grant_is_not_inherited_from_desktop_access() {
        let view = json!([{"id":"vq","name":"VQ","enabled":true,"credentials":"private-must-never-leave"},{"id":"node","name":"NODE","enabled":true}]);
        let endpoints = json!([{"environmentId":"node","descriptorEnvironmentId":"node","id":"gateway","url":"https://node.example/","priority":1}]);
        let c = json!({"bridgeId":"vq-bridge","clientId":"android-client","now":1000,"revision":1});
        let restricted = publish(&view, &json!(["vq"]), &endpoints, &c).unwrap();
        assert_eq!(restricted["environments"].as_array().unwrap().len(), 1);
        assert!(!restricted.to_string().contains("private-must-never-leave"));
        let both = publish(&view, &json!(["vq", "node"]), &endpoints, &c).unwrap();
        assert_eq!(
            both["environments"][1]["routes"][0]["url"],
            "https://node.example/"
        );
        let mut wrong = endpoints.clone();
        wrong[0]["descriptorEnvironmentId"] = json!("vq");
        assert_eq!(
            publish(&view, &json!(["node"]), &wrong, &c).unwrap_err(),
            "t3_environment_mismatch"
        );
    }
    #[test]
    fn vq_node_add_update_withdraw_recover_is_idempotent() {
        let mut s = snapshot();
        let c = context();
        let first = reconcile(&s, &Value::Null, &c).unwrap();
        assert_eq!(first["environments"].as_array().unwrap().len(), 2);
        assert_eq!(reconcile(&s, &first, &c).unwrap(), first);
        s["revision"] = json!(2);
        s["environments"].as_array_mut().unwrap().remove(1);
        let removed = reconcile(&s, &first, &c).unwrap();
        assert_eq!(removed["environments"][0]["environmentId"], "node");
        assert_eq!(removed["environments"][0]["removed"], true);
        assert_eq!(removed["environments"][0]["routes"], json!([]));
        s = snapshot();
        s["revision"] = json!(3);
        s["environments"][1]["routes"][0]["url"] = json!("https://new-node.example/");
        let recovered = reconcile(&s, &removed, &c).unwrap();
        assert_eq!(recovered["environments"][0]["removed"], false);
        assert_eq!(
            recovered["environments"][0]["routes"][0]["url"],
            "https://new-node.example/"
        );
    }
    #[test]
    fn rejects_cross_client_rollback_and_changed_equal_revision() {
        let s = snapshot();
        let c = context();
        let old = reconcile(&s, &Value::Null, &c).unwrap();
        let mut bad = s.clone();
        bad["clientId"] = json!("someone-else");
        assert!(reconcile(&bad, &old, &c).is_err());
        bad = s.clone();
        bad["revision"] = json!(0);
        assert_eq!(
            reconcile(&bad, &old, &c).unwrap_err(),
            "bridge_catalog_rollback"
        );
        bad = s.clone();
        bad["environments"][0]["label"] = json!("changed");
        assert_eq!(
            reconcile(&bad, &old, &c).unwrap_err(),
            "bridge_catalog_revision_reused"
        );
        bad = s.clone();
        bad["expiresAt"] = json!(1000);
        assert!(reconcile(&bad, &old, &c).is_err());
    }
    #[test]
    fn refuses_desktop_loopbacks_and_secret_fields() {
        for url in [
            "http://node.example/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://localhost/",
            "https://host.localhost/",
            "https://169.254.1.1/",
            "https://node.example/?token=secret",
            "https://user:secret@node.example/",
        ] {
            let mut s = snapshot();
            s["environments"][1]["routes"][0]["url"] = json!(url);
            assert!(reconcile(&s, &Value::Null, &context()).is_err(), "{url}");
        }
        let mut s = snapshot();
        s["environments"][0]["token"] = json!("must-not-export");
        assert!(reconcile(&s, &Value::Null, &context()).is_err());
        s = snapshot();
        s["environments"][0]["routes"][0]["bearer"] = json!("never");
        assert!(reconcile(&s, &Value::Null, &context()).is_err());
    }
    #[test]
    fn offline_is_not_a_withdrawal_and_invalid_snapshots_do_not_mutate_cache() {
        let s = snapshot();
        let c = context();
        let old = reconcile(&s, &Value::Null, &c).unwrap();
        let preserved = old.clone();
        let mut bad = s.clone();
        bad["environments"] = json!(null);
        assert!(reconcile(&bad, &old, &c).is_err());
        assert_eq!(old, preserved);
        let mut dup = s.clone();
        let e = dup["environments"][0].clone();
        dup["environments"].as_array_mut().unwrap().push(e);
        assert!(reconcile(&dup, &old, &c).is_err());
    }
}

#[cfg(test)]
mod abi_tests {
    use serde_json::json;
    #[test]
    fn domain_registry_exposes_common_contract_without_network() {
        let manifest = crate::domain::call(
            "connectionCatalog.publishBridge",
            &json!({
                "view":[{"id":"node","name":"NODE","enabled":true}],"authorized":["node"],
                "endpoints":[],"context":{"bridgeId":"b","clientId":"c","revision":1,"now":1000}
            }),
        )
        .unwrap();
        let cached = crate::domain::call(
            "connectionCatalog.reconcileBridge",
            &json!({
                "snapshot":manifest,"cache":{},"context":{"bridgeId":"b","clientId":"c","now":1001}
            }),
        )
        .unwrap();
        assert_eq!(cached["environments"][0]["environmentId"], "node");
        assert_eq!(cached["environments"][0]["removed"], false);
    }
}
