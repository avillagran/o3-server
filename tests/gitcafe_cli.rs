mod support;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[test]
fn documented_cafe_api_uses_cli_login_without_exposing_credentials() {
    let root = std::env::temp_dir().join(format!("cafe-fixture-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("HOME", &root);
    std::env::remove_var("CAFE_TOKEN");
    std::env::remove_var("BITBUCKET_ACCESS_TOKEN");
    std::env::remove_var("FORGEJO_TOKEN");
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", root.display()));
    for name in ["gh", "glab", "az", "tea"] {
        std::fs::write(root.join(name), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(root.join("cafe"), "#!/bin/sh\nif [ \"$*\" = '--host https://git.cafe/api --no-update-check --no-input --json api /auth/principal' ]; then echo '{\"actorId\":\"fixture\",\"handle\":\"fixture-reader\"}'; else exit 1; fi\n").unwrap();
    std::fs::set_permissions(root.join("cafe"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = support::start(|_| {});
    let c = s.connect();
    let result = c.rpc("server.discoverSourceControl", json!({})).unwrap();
    let cafe = result["sourceControlProviders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "gitcafe")
        .unwrap();
    assert_eq!(cafe["auth"]["status"], "authenticated", "{cafe}");
    assert_eq!(cafe["auth"]["account"]["value"], "fixture-reader");
    std::fs::remove_dir_all(root).unwrap();
}
