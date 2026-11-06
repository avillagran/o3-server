//! Source-control discovery probes run concurrently; one file so PATH is process-wide safe.
use std::os::unix::fs::PermissionsExt;

#[test]
fn slow_host_clis_are_probed_concurrently() {
    let dir = std::env::temp_dir().join(format!(
        "o3-discovery-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for exe in ["gh", "glab", "az"] {
        let path = dir.join(exe);
        std::fs::write(&path, "#!/bin/sh\nsleep 2\necho fake 1.0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // The only test in this binary; no other thread reads the environment yet.
    std::env::set_var("PATH", path);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let start = std::time::Instant::now();
    let v = runtime.block_on(o3_server::vcs::discover()).unwrap();
    let elapsed = start.elapsed();
    std::fs::remove_dir_all(&dir).unwrap();
    let kinds: Vec<_> = v["sourceControlProviders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        &kinds[..3],
        ["github", "gitlab", "azure-devops"],
        "order kept"
    );
    for p in &v["sourceControlProviders"].as_array().unwrap()[..3] {
        assert_eq!(p["status"], "available", "{p}");
        assert_eq!(p["auth"]["status"], "authenticated", "{p}");
    }
    // Sequential: 3 CLIs × (version + auth) × 2 s = 12 s. Concurrent: about 4 s.
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "took {elapsed:?}"
    );
}
