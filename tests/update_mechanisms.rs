mod support;
use serde_json::json;
use std::{os::unix::fs::{symlink, PermissionsExt}, path::Path};
fn script(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path,format!("#!/bin/sh\n{text}\n")).unwrap();
    std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[test]
fn receipt_and_native_layout_updates_use_original_installer() {
    let server = support::start(|s| *s.provider.lock().unwrap()=Some(std::sync::Arc::new(o3_server::provider::ConfiguredProviders::new(std::sync::Arc::new(o3_server::provider::ScriptedProvider),s.dir.clone()))));
    let c=server.connect();
    let root=server.dir.join("installers");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::env::set_var("PATH", root.join("bin"));
    let keg=root.join("Cellar/codex/1.0.0");
    script(&keg.join("bin/codex"),"echo codex-cli 1.0.0");
    std::fs::write(keg.join("INSTALL_RECEIPT.json"),r#"{"source":{"path":"/fixture/Formula/c/codex.rb","tap":"homebrew/core"},"poured_from_bottle":true}"#).unwrap();
    script(&root.join("bin/brew"),&format!("printf '%s\\n' \"$@\" > '{}'",root.join("argv").display()));
    let native=root.join("native/.local/share/claude/versions/1.0.0");
    script(&native,&format!("if [ \"$1\" = update ]; then echo update > '{}'; else echo 1.0.0; fi",root.join("native-argv").display()));
    let visible=root.join("native/.local/bin/claude");
    std::fs::create_dir_all(visible.parent().unwrap()).unwrap();
    symlink(&native,&visible).unwrap();
    for (id,driver,binary,marker,expected) in [("brewFixture","codex",keg.join("bin/codex"),root.join("argv"),"upgrade\n--formula\ncodex\n"),("nativeFixture","claudeAgent",visible,root.join("native-argv"),"update\n")] {
        c.rpc("server.updateSettings",json!({"patch":{},"providerInstanceMutation":{"operation":"create","instanceId":id,"instance":{"driver":driver,"config":{"binaryPath":binary}}}})).unwrap();
        let config=t3_protocol::api::server::config(&c).unwrap();
        let rows=t3_protocol::provider_instances::projection(&config["settings"],&config,true);
        let row=rows["instances"].as_array().unwrap().iter().find(|r|r["id"]==id).unwrap();
        assert!(!marker.exists(),"reading config must not update");
        if unsafe {libc::geteuid()}==0 {assert_eq!(row["canUpdate"],false);continue;}
        assert_eq!(row["canUpdate"],true,"{row}");
        t3_protocol::api::provider::update(&c,driver,None,Some(id)).unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(),expected);
    }
    std::fs::remove_file(keg.join("INSTALL_RECEIPT.json")).unwrap();
    let config=t3_protocol::api::server::config(&c).unwrap();
    let rows=t3_protocol::provider_instances::projection(&config["settings"],&config,true);
    assert_eq!(rows["instances"].as_array().unwrap().iter().find(|r|r["id"]=="brewFixture").unwrap()["canUpdate"],false);
}
