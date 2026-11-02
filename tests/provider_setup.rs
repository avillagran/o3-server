//! Provider setup surfaces through the real client: sign-in flows driving the vendor CLI, install state,
//! `server.updateProvider` and the ACP registry. The vendor CLIs are scripted stand-ins on PATH.
mod support;
use serde_json::json;
use std::{io::Write, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use support::{raw_exit, start, subscribe};

const T: Duration = Duration::from_secs(10);

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn registry_server(body: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/registry.json", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
        }
    });
    url
}

#[test]
fn sign_in_install_update_and_registry() {
    let bin = std::env::temp_dir().join(format!("o3-setup-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bin);
    std::fs::create_dir_all(&bin).unwrap();
    let signed = bin.join("signed-in");
    script(
        &bin.join("claude"),
        &format!(
            r#"case "$1 $2" in
"--version ") echo "2.1.0 (Claude Code)";;
"auth login") echo "Open https://claude.example/oauth?code_challenge=abc to sign in"; if [ -f {d}/hang ]; then sleep 30; fi; read code; [ "$code" = good ] && touch {s};;
"auth status") [ -f {s} ];;
"auth logout") rm -f {s};;
"update ") echo updating >> {d}/update.log;;
esac"#,
            d = bin.display(),
            s = signed.display()
        ),
    );
    script(&bin.join("npx"), "exit 0");
    script(
        &bin.join("codex"),
        "case \"$1\" in --version) echo codex-cli 0.9.1;; esac",
    );
    let registry = registry_server(
        json!({"agents":[
            {"id":"alpha","name":"Alpha Agent","version":"1.0.0","description":"Writes code","authors":["a"],"license":"MIT","icon":"https://cdn.agentclientprotocol.com/a.svg","distribution":{"npx":{"package":"alpha@1"}}},
            {"id":"beta","name":"Beta","version":"2.0.0","description":"Other","distribution":{"binary":{"linux-x86_64":{"archive":"x","cmd":"y"}}}},
            {"id":"gamma","name":"Gamma","version":"1","description":"no dist","distribution":{}}
        ]}).to_string(),
    );
    // SAFETY: this file has one test and no server thread exists yet.
    unsafe {
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        std::env::set_var("O3_ACP_REGISTRY_URL", &registry);
    }
    let server = start(|_| {});
    let c = server.connect();

    // Install state is reported, never managed.
    let mut install = subscribe(
        server.connect(),
        "provider.install.subscribe",
        json!({"instanceId":"claudeAgent"}),
    );
    let st = install.next(T);
    assert_eq!(
        (
            st["driver"].as_str(),
            st["installedVersion"].as_str(),
            st["source"].as_str(),
            st["canRemove"].as_bool()
        ),
        (
            Some("claudeAgent"),
            Some("2.1.0"),
            Some("local"),
            Some(false)
        )
    );
    assert_eq!(st["phase"], "idle");
    let failure = raw_exit(
        &server,
        "provider.install.start",
        json!({"instanceId":"claudeAgent"}),
    );
    assert_eq!(
        failure["exit"]["cause"][0]["error"]["_tag"],
        "ProviderSetupError"
    );
    assert!(c
        .rpc(
            "provider.install.remove",
            json!({"instanceId":"claudeAgent"})
        )
        .is_err());

    // A full sign-in: the CLI prints a URL, the client answers on its terminal, the flow verifies.
    let mut auth = subscribe(
        server.connect(),
        "provider.auth.subscribe",
        json!({"instanceId":"claudeAgent"}),
    );
    assert_eq!(auth.next(T)["phase"], "idle");
    let started = c
        .rpc("provider.auth.start", json!({"instanceId":"claudeAgent"}))
        .unwrap();
    assert_eq!(started["instanceId"], "claudeAgent");
    let flow = started["flowId"].as_str().unwrap().to_owned();
    let waiting = auth.until(T, |v| v["phase"] == "waiting")[..]
        .last()
        .unwrap()
        .clone();
    assert!(waiting["authorizationUrl"]
        .as_str()
        .unwrap()
        .starts_with("https://claude.example/oauth"));
    assert_eq!(waiting["interaction"]["type"], "browser");
    assert!(
        c.rpc("provider.auth.start", json!({"instanceId":"claudeAgent"}))
            .is_err(),
        "one flow at a time"
    );
    let wrong = c.rpc("provider.auth.respond", json!({"instanceId":"claudeAgent","flowId":"other","interactionId":"browser","response":{"type":"terminal","data":"good\n"}}));
    assert!(wrong.is_err());
    c.rpc("provider.auth.respond", json!({"instanceId":"claudeAgent","flowId":flow,"interactionId":"browser","response":{"type":"terminal","data":"good\n"}})).unwrap();
    let done = auth.until(T, |v| v["phase"] == "succeeded");
    assert!(
        done.iter().any(|v| v["phase"] == "verifying")
            || done.last().unwrap()["phase"] == "succeeded"
    );
    assert!(signed.exists());

    // Logout runs the CLI and resets the flow.
    let out = c
        .rpc("provider.auth.logout", json!({"instanceId":"claudeAgent"}))
        .unwrap();
    assert_eq!(out["phase"], "idle");
    assert!(!signed.exists());

    // A wrong code fails honestly; a hung CLI can be cancelled and is killed.
    let started = c
        .rpc("provider.auth.start", json!({"instanceId":"claudeAgent"}))
        .unwrap();
    auth.until(T, |v| {
        v["phase"] == "waiting" && v["flowId"] == started["flowId"]
    });
    c.rpc("provider.auth.respond", json!({"instanceId":"claudeAgent","flowId":started["flowId"],"interactionId":"browser","response":{"type":"terminal","data":"bad\n"}})).unwrap();
    let failed = auth.until(T, |v| v["phase"] == "failed");
    assert!(!signed.exists());
    assert!(failed.last().unwrap()["message"].is_string());
    std::fs::write(bin.join("hang"), "").unwrap();
    let started = c
        .rpc("provider.auth.start", json!({"instanceId":"claudeAgent"}))
        .unwrap();
    auth.until(T, |v| {
        v["phase"] == "waiting" && v["flowId"] == started["flowId"]
    });
    let cancelled = c
        .rpc(
            "provider.auth.cancel",
            json!({"instanceId":"claudeAgent","flowId":started["flowId"]}),
        )
        .unwrap();
    assert!(["waiting", "cancelled"].contains(&cancelled["phase"].as_str().unwrap()));
    auth.until(T, |v| v["phase"] == "cancelled");
    // Codex signs in through its device-auth command; an unknown instance is a typed error.
    assert!(c
        .rpc("provider.auth.start", json!({"instanceId":"nope"}))
        .is_err());
    // Scope: setup needs providers:manage.
    let limited = server.limited(&c, &["orchestration:read"]);
    let denied = limited
        .rpc("provider.auth.start", json!({"instanceId":"claudeAgent"}))
        .unwrap_err()
        .to_string();
    assert!(denied.contains("write_not_authorized"), "{denied}");

    // Update runs `claude update`; other drivers are refused with a typed reason.
    let upd = c
        .rpc("server.updateProvider", json!({"provider":"claudeAgent"}))
        .unwrap();
    assert!(upd["providers"].is_array());
    assert!(std::fs::read_to_string(bin.join("update.log"))
        .unwrap()
        .contains("updating"));
    let refused = raw_exit(
        &server,
        "server.updateProvider",
        json!({"provider":"codex"}),
    );
    assert_eq!(
        refused["exit"]["cause"][0]["error"]["_tag"],
        "ServerProviderUpdateError"
    );

    // ACP registry: entries without a supported distribution are dropped; icons must be on the CDN.
    let all = c
        .rpc("server.searchAcpRegistry", json!({"query":""}))
        .unwrap();
    let ids: Vec<_> = all["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["alpha", "beta"]);
    assert_eq!(all["agents"][0]["distribution"], "npx");
    let found = c
        .rpc("server.searchAcpRegistry", json!({"query":"writes"}))
        .unwrap();
    assert_eq!(found["agents"].as_array().unwrap().len(), 1);
    let prepared = t3_protocol::api::server::prepare_acp_registry_agent(&c, "alpha");
    assert!(prepared
        .unwrap_err()
        .to_string()
        .contains("ACP execution driver"));
    let ok = raw_exit(
        &server,
        "server.prepareAcpRegistryAgent",
        json!({"agentId":"beta"}),
    );
    assert_eq!(
        ok["exit"]["cause"][0]["error"]["_tag"],
        "AcpRegistryOperationError"
    );
    assert_eq!(
        ok["exit"]["cause"][0]["error"]["reason"],
        "unsupported_distribution"
    );
    let missing = raw_exit(
        &server,
        "server.prepareAcpRegistryAgent",
        json!({"agentId":"zzz"}),
    );
    assert_eq!(
        missing["exit"]["cause"][0]["error"]["reason"],
        "agent_not_found"
    );
    let _ = std::fs::remove_dir_all(&bin);
}
