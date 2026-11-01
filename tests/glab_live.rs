mod common;
use common::*;
use serde_json::json;

#[test]
#[ignore = "requires O3_LIVE_GLAB pointing to a real installed glab; local host fixture"]
fn real_glab_status_through_server_and_lib_reader() {
    use std::{
        io::{Read, Write},
        os::unix::fs::{symlink, PermissionsExt},
    };
    let cli = std::env::var_os("O3_LIVE_GLAB").expect("O3_LIVE_GLAB required");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let home = std::env::temp_dir().join(format!("glab-live-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let config = home.join("config.yml");
    write(
        &config,
        format!("hosts:\n  {host}:\n    api_protocol: http\n    token: fixture-only\n"),
    );
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::env::set_var("GLAB_CONFIG_DIR", &home);
    std::env::set_var("GITLAB_HOST", &host);
    std::env::set_var("GLAB_CHECK_UPDATE", "false");
    std::env::set_var("GLAB_SEND_TELEMETRY", "false");
    let e = setup("glab-live");
    symlink(cli, bin_dir().join("glab")).unwrap();
    let root = e.dir("repo");
    repo(&root);
    write(&root.join("a"), "a\n");
    commit_all(&root, "initial");
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            &format!("http://{host}/owner/repo.git"),
        ],
    );
    git(&root, &["switch", "-c", "feature"]);
    let server = std::thread::spawn(move || {
        for status in ["200 OK", "403 Forbidden"] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0; 8192];
            let n = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(
                request.contains("source_branch=feature")
                    && request.contains("state=all")
                    && request.contains("per_page=1"),
                "{request}"
            );
            let body = if status.starts_with("200") {
                r#"[{"iid":7,"title":"Proof","web_url":"https://example.test/owner/repo/-/merge_requests/7","target_branch":"main","source_branch":"feature","state":"opened","draft":true,"updated_at":"2026-01-01T00:00:00Z"}]"#
            } else {
                r#"{"message":"Forbidden"}"#
            };
            write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let value = t3_protocol::api::vcs::refresh_status(&e.conn, root.to_str().unwrap()).unwrap();
    assert_eq!(value["pr"]["number"], 7);
    assert_eq!(value["pr"]["state"], "open");
    assert_eq!(value["pr"]["isDraft"], true);
    let local = e
        .conn
        .rpc("vcs.refreshStatus", json!({"cwd":root,"localOnly":true}))
        .unwrap();
    assert!(local["pr"].is_null());
    let failed = t3_protocol::api::vcs::refresh_status(&e.conn, root.to_str().unwrap()).unwrap();
    assert!(failed["pr"].is_null());
    assert_eq!(failed["sourceControlProvider"]["kind"], "gitlab");
    server.join().unwrap();
    std::fs::remove_dir_all(home).unwrap();
}
