mod support;
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};
#[test]
fn private_npm_install_and_pinned_update_drive_real_provider_reader() {
    let s = support::start(|s| {
        o3_server::settings::update(s,&json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"binaryPath":s.dir.join("missing-codex")}}}}})).unwrap();
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
    let bin = s.dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("npm"),r#"#!/usr/bin/python3
import sys,pathlib,json,os
args=sys.argv[1:]
assert args[0]=='--userconfig' and args[2]=='--globalconfig' and args[1]!=args[3],args
assert pathlib.Path(args[1]).read_text()==pathlib.Path(args[3]).read_text()==''
assert args[4]=='--cache' and args[6:8]==['install','--prefix'],args
assert args[9:12]==['--ignore-scripts','--no-audit','--no-fund'],args
assert args[12] in ['@openai/codex@latest','@openai/codex@2.0.0'],args
prefix=pathlib.Path(args[8]);assert prefix==pathlib.Path.cwd()
version='2.0.0' if args[12].endswith('@2.0.0') else '1.2.3'
binary=prefix/'node_modules/.bin/codex';binary.parent.mkdir(parents=True,exist_ok=True)
binary.write_text('#!/bin/sh\necho codex-cli '+version+'\n');binary.chmod(0o700)
manifest=prefix/'node_modules/@openai/codex/package.json';manifest.parent.mkdir(parents=True,exist_ok=True)
manifest.write_text(json.dumps({'version':version}))
"#).unwrap();
    std::fs::set_permissions(bin.join("npm"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    let c = s.connect();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    assert_eq!(config["providers"][0]["setup"]["canInstall"], true);
    let started = c
        .rpc("provider.install.start", json!({"instanceId":"codex"}))
        .unwrap();
    assert_eq!(started["phase"], "downloading");
    let wait = || {
        let done = std::cell::Cell::new(false);
        let mut state = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        c.subscribe(
            "provider.install.subscribe",
            json!({"instanceId":"codex"}),
            || done.get() || Instant::now() > deadline,
            |v| {
                if let Some(v) = v {
                    if v["phase"] == "succeeded" || v["phase"] == "failed" {
                        state = Some(v);
                        done.set(true);
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        let state = state.unwrap();
        assert_eq!(state["phase"], "succeeded", "{state}");
        state
    };
    let installed = wait();
    assert_eq!(installed["installedVersion"], "1.2.3");
    let old = installed["executablePath"].as_str().unwrap().to_owned();
    let config = c.rpc("server.getConfig", json!({})).unwrap();
    let view = t3_protocol::provider_instances::projection(&config["settings"], &config, true);
    assert!(view["instances"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["canUpdate"] == true));
    assert_eq!(
        config["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["instanceId"] == "codex")
            .unwrap()["versionAdvisory"]["canInstallVersion"],
        true
    );
    c.rpc(
        "server.updateProvider",
        json!({"provider":"codex","instanceId":"codex","targetVersion":"2.0.0"}),
    )
    .unwrap();
    let updated = wait();
    assert_eq!(updated["installedVersion"], "2.0.0");
    assert_ne!(updated["executablePath"], old);
    assert!(std::path::Path::new(&old).is_file());
    c.rpc("server.updateSettings",json!({"patch":{"providerInstances":{"codex":{"driver":"codex","enabled":true,"config":{"binaryPath":s.dir.join("missing-again")}}}}})).unwrap();
    let missing = c.rpc("server.getConfig", json!({})).unwrap();
    assert!(missing["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["instanceId"] == "codex" && p["versionAdvisory"]["canInstallVersion"] == true));
    c.rpc(
        "server.updateProvider",
        json!({"provider":"codex","instanceId":"codex","targetVersion":"2.0.0"}),
    )
    .unwrap();
    assert_eq!(wait()["installedVersion"], "2.0.0");
    assert!(c
        .rpc(
            "server.updateProvider",
            json!({"provider":"codex","targetVersion":"2.0.0;exit"})
        )
        .is_err());
}
