mod support;
#[test]
fn clear_profile_rejects_unsafe_storage() {
    let server = support::start(|_| {});
    let c = server.connect();
    // Real reader, with an existing profile root replaced by a symlink.
    let outside = server.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), "safe").unwrap();
    std::os::unix::fs::symlink(&outside, server.dir.join("browser-profiles")).unwrap();
    assert!(t3_protocol::api::preview::clear_profile(&c, "default").is_err());
    assert!(outside.join("keep").exists());
}

#[test]
fn fixture_browser_lifecycle_and_profile_clear() {
    use t3_protocol::api::preview::{self, PreviewOpen};
    // This test file must run with one thread: O3_BROWSER_BINARY is process-wide.
    std::env::set_var(
        "O3_BROWSER_BINARY",
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"),
    );
    let server = support::start(|_| {});
    let c = server.connect();
    assert_eq!(
        t3_protocol::api::server::config(&c).unwrap()["environment"]["capabilities"]
            ["serverBrowser"],
        true
    );
    assert!(
        !server.dir.join("browser-profiles").exists(),
        "browser remains lazy"
    );
    let open = preview::open(
        &c,
        "browser-thread",
        &PreviewOpen {
            runtime: Some("server".into()),
            profile_id: Some("default".into()),
            url: Some("http://localhost:1234".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let tab = open["tabId"].as_str().unwrap();
    let path = std::fs::read_dir(server.dir.join("browser-profiles"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let pid: i32 = std::fs::read_to_string(path.join("fixture.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    println!(
        "fixture browser child {}",
        status.lines().find(|l| l.starts_with("VmRSS:")).unwrap()
    );
    preview::navigate(
        &c,
        "browser-thread",
        tab,
        "http://localhost:1234/next",
        None,
    )
    .unwrap();
    let commands = std::fs::read_to_string(path.join("commands.jsonl")).unwrap();
    assert!(commands.contains("fixture-session"));
    assert!(commands.contains("Page.navigate"));
    let ticket = c
        .request_json(
            "POST",
            "/api/auth/websocket-ticket/media",
            Some(&serde_json::json!({})),
            true,
        )
        .unwrap()["ticket"]
        .as_str()
        .unwrap()
        .to_owned();
    let address = server.base.trim_start_matches("http://");
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use futures_util::{SinkExt,StreamExt};
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest,Message};
        let tcp=tokio::net::TcpStream::connect(address).await.unwrap();
        let request=format!("ws://{address}/api/preview-stream/ws?threadId=browser-thread&tabId={tab}&wsTicket={ticket}").into_client_request().unwrap();
        let (mut ws,_)=tokio_tungstenite::client_async(request,tcp).await.unwrap();
        let control=ws.next().await.unwrap().unwrap();
        assert!(control.to_text().unwrap().contains("canOperate"));
        let frame=tokio::time::timeout(std::time::Duration::from_secs(3),ws.next()).await.unwrap().unwrap().unwrap();
        assert!(frame.is_binary());assert_eq!(frame.into_data().as_ref(),&[255,216,255,217]);
        ws.send(Message::Text(r#"{"type":"ack"}"#.into())).await.unwrap();
        ws.close(None).await.unwrap();
        let tcp=tokio::net::TcpStream::connect(address).await.unwrap();
        let request=format!("ws://{address}/api/preview-stream/ws?threadId=browser-thread&tabId={tab}").into_client_request().unwrap();
        assert!(tokio_tungstenite::client_async(request,tcp).await.is_err(),"anonymous viewers refused");
    });
    let second = preview::open(
        &c,
        "browser-thread",
        &PreviewOpen {
            runtime: Some("server".into()),
            profile_id: Some("default".into()),
            url: Some("http://localhost:1234/second".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_dir(server.dir.join("browser-profiles"))
            .unwrap()
            .count(),
        1,
        "tabs share a persistent context"
    );
    preview::close(&c, "browser-thread", Some(tab)).unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "closing one tab keeps its context alive"
    );
    preview::navigate(
        &c,
        "browser-thread",
        second["tabId"].as_str().unwrap(),
        "http://localhost:1234/third",
        None,
    )
    .unwrap();
    preview::clear_profile(&c, "default").unwrap();
    assert!(!path.exists());
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "child reaped before profile removal"
    );
    let limited = server.limited(&c, &["orchestration:read"]);
    assert!(preview::clear_profile(&limited, "default").is_err());
}

fn audit_browser_case(case: &str) {
    use serde_json::json;
    std::env::set_var(
        "O3_BROWSER_BINARY",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/browser-cdp.py"),
    );
    let server = support::start(|_| {});
    let c = server.connect();
    for n in 0..8 {
        c.rpc("preview.open", json!({"threadId":"audit","runtime":"server","profileId":format!("audit-{n}"),"url":"http://localhost","viewport":{"_tag":"freeform","width":640,"height":480}})).unwrap();
    }
    let paths: Vec<_> = std::fs::read_dir(server.dir.join("browser-profiles"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    let commands = std::fs::read_to_string(paths[0].join("commands.jsonl")).unwrap();
    if case == "viewport" {
        assert!(
            commands.contains("Emulation.setDeviceMetricsOverride"),
            "initial viewport never applied"
        );
        return;
    }
    let list = t3_protocol::api::preview::list(&c, "audit").unwrap();
    let tab = list["sessions"][0]["tabId"].as_str().unwrap();
    if case == "navigation" {
        assert!(
            t3_protocol::api::preview::navigate(
                &c,
                "audit",
                tab,
                "http://localhost/rejected",
                None
            )
            .is_err(),
            "CDP navigation error ignored"
        );
        return;
    }
    for path in paths {
        let pid: i32 = std::fs::read_to_string(path.join("fixture.pid"))
            .unwrap()
            .parse()
            .unwrap();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(
        support::wait_for(std::time::Duration::from_secs(4), || {
            t3_protocol::api::preview::list(&c, "audit").unwrap()["sessions"]
                .as_array()
                .unwrap()
                .is_empty()
        }),
        "dead tabs retained"
    );
    c.rpc(
        "preview.open",
        json!({"threadId":"audit","runtime":"server","url":"http://localhost"}),
    )
    .unwrap();
}

#[test]
fn initial_viewport_is_applied() {
    audit_browser_case("viewport");
}
#[test]
fn rejected_navigation_reaches_reader() {
    audit_browser_case("navigation");
}
#[test]
fn dead_browsers_release_capacity_and_tabs() {
    audit_browser_case("lifecycle");
}
