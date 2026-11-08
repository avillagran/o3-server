//! Local fixtures run in rootless Podman; GitHub jobs use verified disposable Docker containers.
#[track_caller]
pub fn assert_isolated() {
    let podman = std::path::Path::new("/run/.containerenv").is_file();
    let github_docker = std::env::var("CI").as_deref() == Ok("true")
        && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && std::path::Path::new("/.dockerenv").is_file();
    assert!(
        podman || github_docker,
        "fixtures require rootless Podman locally or a verified GitHub Docker job container"
    );
}
