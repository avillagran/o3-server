mod support;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[test]
#[ignore = "downloads both real npm packages into isolated private prefixes"]
fn real_npm_installs_runnable_provider_binaries_in_private_prefixes() {
    let s = support::start(|s| {
        o3_server::settings::update(s, &json!({"patch":{"providerInstances":{
            "codex":{"driver":"codex","enabled":true,"config":{"binaryPath":s.dir.join("missing-codex")}},
            "claudeAgent":{"driver":"claudeAgent","enabled":true,"config":{"binaryPath":s.dir.join("missing-claude")}}
        }}})).unwrap();
        let defaults = Arc::new(o3_server::provider::Providers {
            claude: None,
            codex: Arc::new(o3_server::codex::CodexProvider::new(
                s.dir.join("missing-codex"),
                None,
            )),
        });
        *s.provider.lock().unwrap() = Some(Arc::new(
            o3_server::provider::ConfiguredProviders::new(defaults, s.dir.clone()),
        ));
    });
    let c = s.connect();
    for id in ["codex", "claudeAgent"] {
        let config = c.rpc("server.getConfig", json!({})).unwrap();
        assert!(
            config["providers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["instanceId"] == id && p["setup"]["canInstall"] == true),
            "{config}"
        );
        c.rpc("provider.install.start", json!({"instanceId":id}))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(320);
        let mut result = None;
        c.subscribe(
            "provider.install.subscribe",
            json!({"instanceId":id}),
            || Instant::now() > deadline,
            |v| {
                if let Some(v) = v {
                    if matches!(
                        v["phase"].as_str(),
                        Some("succeeded" | "failed" | "cancelled")
                    ) {
                        result = Some(v);
                        return Err("finished".into());
                    }
                }
                Ok(())
            },
        )
        .ok();
        let result = result.expect("installation finished");
        assert_eq!(result["phase"], "succeeded", "{id}: {result}");
        let binary = std::path::PathBuf::from(result["executablePath"].as_str().unwrap());
        assert!(binary.starts_with(s.dir.join("provider-clis")));
        let out = std::process::Command::new(binary)
            .arg("--version")
            .env("HOME", &s.dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout)
            .contains(result["installedVersion"].as_str().unwrap()));
    }
}
