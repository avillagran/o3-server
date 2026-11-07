//! Star RPC admission and public settings keep host-scoped credentials private.
#[path = "support/reference.rs"]
mod reference;
mod support;
use serde_json::json;

#[test]
fn star_controls_require_write_scope_and_redact_keyed_credentials() {
    let server = support::start(|_| {});
    let owner = server.connect();
    let secret_ref = json!({"provider":"vault","reference":"github/token"});
    owner.rpc("server.updateSettings",json!({"patch":{"sourceControlHosts":{"github":{"hosts":{"github.com":{"enabled":false}},"tokens":{"github.com":"fixture-private-token","ref.test":secret_ref},"signIns":{"github.com":"fixture-private-sign-in"}}}}})).unwrap();
    for tag in ["server.getSettings", "server.getConfig"] {
        let public = owner.rpc(tag, json!({})).unwrap();
        let settings = if tag == "server.getConfig" {
            &public["settings"]
        } else {
            &public
        };
        let hosts = &settings["sourceControlHosts"]["github"];
        assert_eq!(hosts["tokens"]["github.com"], o3_server::settings::SENTINEL);
        assert_eq!(
            hosts["signIns"]["github.com"],
            o3_server::settings::SENTINEL
        );
        assert_eq!(
            hosts["tokens"]["ref.test"],
            o3_server::settings::SENTINEL,
            "clients without secret-reference negotiation see the native saved-secret marker"
        );
        assert!(!public.to_string().contains("fixture-private"));
    }
    let public = owner.rpc("server.getSettings", json!({})).unwrap();
    owner
        .rpc(
            "server.updateSettings",
            json!({"patch":{"sourceControlHosts":public["sourceControlHosts"]}}),
        )
        .unwrap();
    let stored = o3_server::settings::read(&server.state).unwrap();
    assert_eq!(
        stored["sourceControlHosts"]["github"]["tokens"]["github.com"],
        o3_server::settings::SENTINEL,
        "private settings keep the saved-secret marker"
    );
    let materialized =
        o3_server::settings_secrets::materialize_profile(&server.dir, &stored).unwrap();
    assert_eq!(
        materialized["sourceControlHosts"]["github"]["tokens"]["github.com"],
        "fixture-private-token",
        "round-tripping public settings preserves the private credential slot"
    );
    assert_eq!(
        o3_server::settings::read(&server.state).unwrap()["sourceControlHosts"]["github"]["tokens"]
            ["ref.test"],
        secret_ref,
        "round-tripping the saved-secret marker preserves the private reference"
    );
    let read = server.limited(&owner, &["orchestration:read"]);
    let payload = json!({"provider":"github","host":"github.com","repository":"o/r"});
    assert_eq!(
        read.rpc("sourceControl.starred", payload.clone()).unwrap(),
        false
    );
    assert!(read
        .rpc("sourceControl.star", payload.clone())
        .unwrap_err()
        .contains("source-control:write"));
    assert_eq!(
        owner.rpc("sourceControl.star", payload).unwrap(),
        false,
        "disabled host never falls back to another credential"
    );
    for repository in ["o/..", "../r", "/r", "o/", "o/r/extra"] {
        assert_eq!(
            owner
                .rpc(
                    "sourceControl.star",
                    json!({"provider":"github","host":"github.com","repository":repository})
                )
                .unwrap(),
            false
        );
    }
    assert_eq!(
        owner
            .rpc(
                "sourceControl.star",
                json!({"provider":"unregistered-forge","host":"github.com","repository":"o/r"})
            )
            .unwrap(),
        false
    );
}

#[test]
#[ignore = "official nightly 4262; use tools/test-headless-reference --test forge in rootless Podman"]
fn official_nightly_matches_star_fallbacks_input_defects_and_permissions() {
    let server = support::start(|_| {});
    let owner = server.connect();
    let (native, process) = reference::start(&server.dir);
    let disabled = json!({"patch":{"sourceControlHosts":{"github":{"hosts":{"github.com":{"enabled":false}}}}}});
    owner
        .rpc("server.updateSettings", disabled.clone())
        .unwrap();
    native.rpc("server.updateSettings", disabled).unwrap();
    let raw = support::RawClient::pair(&server);
    for tag in ["sourceControl.star", "sourceControl.starred"] {
        for payload in [
            json!({}),
            json!({"provider":"github"}),
            json!({"provider":"github","host":"github.com"}),
            json!({"provider":2,"host":"github.com","repository":"o/r"}),
            json!({"provider":"github","host":" ","repository":"o/r"}),
            json!({"provider":"github","host":"github.com","repository":[]}),
        ] {
            assert_eq!(
                raw.exit(tag, payload.clone()),
                process.raw_exit(tag, payload.clone()),
                "{tag} {payload}"
            );
        }
        for payload in [
            json!({"provider":"unregistered-forge","host":"github.com","repository":"o/r"}),
            json!({"provider":"github","host":"github.com","repository":"o/r"}),
            json!({"provider":"github","host":"github.com","repository":"o/.."}),
            json!({"provider":"github","host":"https://github.com","repository":"o/r"}),
        ] {
            assert_eq!(
                owner.rpc(tag, payload.clone()).unwrap(),
                native.rpc(tag, payload.clone()).unwrap(),
                "{tag} {payload}"
            );
        }
    }
    let reference = std::env::var("T3_REFERENCE_BIN").unwrap();
    let issued = std::process::Command::new(reference)
        .env("T3CODE_HOME", server.dir.join("reference"))
        .args([
            "auth",
            "session",
            "issue",
            "--scope",
            "orchestration:read",
            "--subject",
            "forge-read",
            "--ttl",
            "1h",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(issued.status.success());
    let token = String::from_utf8(issued.stdout).unwrap();
    let native_read =
        support::RawClient::bearer(native.base_url().trim_end_matches('/'), token.trim());
    let grant = server
        .state
        .issue_pairing_scoped(Some("forge reader"), 60000, "orchestration:read", "owner")
        .unwrap();
    let session = server
        .state
        .exchange_pairing(grant["credential"].as_str().unwrap(), &json!({}))
        .unwrap()
        .unwrap();
    let ours_read =
        support::RawClient::bearer(&server.base, session["access_token"].as_str().unwrap());
    for payload in [
        json!({}),
        json!({"provider":"github","host":"github.com","repository":"o/r"}),
    ] {
        assert_eq!(
            ours_read.exit("sourceControl.star", payload.clone()),
            native_read.exit("sourceControl.star", payload)
        );
    }
    println!("OFFICIAL_NIGHTLY_FORGE_PASS 22 fallback, malformed-input and permission comparisons; no external star requests");
}
