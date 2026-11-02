//! The environment descriptor only promises pull requests when `gh`, o3's only way to create
//! them, can run. Own binary: it changes PATH.
mod support;
use std::os::unix::fs::PermissionsExt;

#[test]
fn pull_request_capability_follows_the_github_cli() {
    let s = support::start(|_| {});
    let bin = s.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let old = std::env::var_os("PATH").unwrap();
    std::env::set_var("PATH", &bin);
    let without = t3_protocol::fetch_descriptor(&s.base).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let with = t3_protocol::fetch_descriptor(&s.base).unwrap();
    std::env::set_var("PATH", old);
    let capability = |d: &serde_json::Value| d["capabilities"]["pullRequests"].clone();
    assert_eq!(capability(&serde_json::to_value(&without).unwrap()), false);
    assert_eq!(capability(&serde_json::to_value(&with).unwrap()), true);
}
