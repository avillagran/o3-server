//! `preview.*`, `subscribePreviewEvents` and `subscribeDiscoveredLocalServers`. The server only keeps the
//! registry of tabs a client renders; it has no browser, so `runtime: "server"` is refused.
mod support;
use serde_json::json;
use std::{io::Write, time::Duration};
use support::{raw_exit, start, subscribe};

const T: Duration = Duration::from_secs(10);

#[test]
fn preview_registry_events_and_refusal_of_the_server_runtime() {
    let server = start(|_| {});
    let c = server.connect();
    let mut events = subscribe(server.connect(), "subscribePreviewEvents", json!({}));
    // This stream sends no first item to wait on, and requests on separate sockets do not order.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let opened = c
        .rpc(
            "preview.open",
            json!({"threadId":"t1","url":"localhost:5173/app"}),
        )
        .unwrap();
    let tab = opened["tabId"].as_str().unwrap().to_owned();
    assert!(tab.starts_with("tab_"));
    assert_eq!(
        opened["navStatus"],
        json!({"_tag":"Loading","url":"http://localhost:5173/app","title":""})
    );
    assert_eq!(opened["viewport"], json!({"_tag":"fill"}));
    let first = events.next(T);
    assert_eq!(
        (
            first["type"].as_str(),
            first["tabId"].as_str(),
            first["revision"].as_u64()
        ),
        (Some("opened"), Some(tab.as_str()), Some(1))
    );
    let epoch = first["serverEpoch"].as_str().unwrap().to_owned();

    let nav = c.rpc("preview.navigate", json!({"threadId":"t1","tabId":tab,"url":"https://example.com","resolvedTitle":"Example"})).unwrap();
    assert_eq!(
        nav["navStatus"],
        json!({"_tag":"Success","url":"https://example.com/","title":"Example"})
    );
    c.rpc("preview.resize", json!({"threadId":"t1","tabId":tab,"viewport":{"_tag":"freeform","width":800,"height":600}})).unwrap();
    c.rpc("preview.adjust", json!({"threadId":"t1","tabId":tab,"colorScheme":"dark","zoomFactor":1.25,"hardReload":true})).unwrap();
    c.rpc("preview.reportStatus", json!({"threadId":"t1","tabId":tab,"navStatus":{"_tag":"LoadFailed","url":"https://example.com/","title":"","code":-105,"description":"DNS"},"canGoBack":true,"canGoForward":false})).unwrap();
    let kinds: Vec<_> = (0..4)
        .map(|_| events.next(T))
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_owned(),
                e["revision"].as_u64().unwrap(),
                e["serverEpoch"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        kinds.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(),
        ["navigated", "resized", "resized", "failed"]
    );
    assert_eq!(kinds.iter().map(|k| k.1).collect::<Vec<_>>(), [2, 3, 4, 5]);
    assert!(kinds.iter().all(|k| k.2 == epoch));
    let adjusted = &events.seen[3];
    assert_eq!(adjusted["request"], json!({"hardReload":true}));
    assert_eq!(adjusted["snapshot"]["colorScheme"], "dark");

    let list = c.rpc("preview.list", json!({"threadId":"t1"})).unwrap();
    assert_eq!(
        (
            list["serverEpoch"].as_str(),
            list["revision"].as_u64(),
            list["sessions"].as_array().unwrap().len()
        ),
        (Some(epoch.as_str()), Some(5), 1)
    );
    assert_eq!(list["sessions"][0]["canGoBack"], true);
    assert_eq!(
        list["sessions"][0]["viewport"],
        json!({"_tag":"freeform","width":800,"height":600})
    );
    assert!(
        c.rpc("preview.list", json!({"threadId":"other"})).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Typed errors.
    let missing = raw_exit(
        &server,
        "preview.navigate",
        json!({"threadId":"t1","tabId":"tab_zz","url":"https://a.b"}),
    );
    assert_eq!(
        missing["exit"]["cause"][0]["error"]["_tag"],
        "PreviewSessionLookupError"
    );
    let bad = raw_exit(
        &server,
        "preview.navigate",
        json!({"threadId":"t1","tabId":tab,"url":"ftp://a.b/x"}),
    );
    assert_eq!(
        (
            bad["exit"]["cause"][0]["error"]["_tag"].as_str(),
            bad["exit"]["cause"][0]["error"]["reason"].as_str()
        ),
        (Some("PreviewInvalidUrlError"), Some("unsupported-protocol"))
    );
    assert!(c.rpc("preview.resize", json!({"threadId":"t1","tabId":tab,"viewport":{"_tag":"freeform","width":10,"height":10}})).is_err());
    assert!(c
        .rpc(
            "preview.adjust",
            json!({"threadId":"t1","tabId":tab,"zoomFactor":1.3})
        )
        .is_err());
    let server_runtime = raw_exit(
        &server,
        "preview.open",
        json!({"threadId":"t1","runtime":"server"}),
    );
    assert_eq!(
        server_runtime["exit"]["cause"][0]["error"]["_tag"], "RpcNotImplemented",
        "no server browser"
    );
    assert_eq!(
        c.rpc("server.getConfig", json!({})).unwrap()["environment"]["capabilities"]
            .get("serverBrowser"),
        None
    );

    c.rpc("preview.close", json!({"threadId":"t1","tabId":tab}))
        .unwrap();
    let closed = events.next(T);
    assert_eq!(closed["type"], "closed");
    assert!(
        c.rpc("preview.list", json!({"threadId":"t1"})).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    c.rpc("preview.clearProfile", json!({"profileId":"default"}))
        .unwrap();
    let scoped = server.limited(&c, &["orchestration:read"]);
    assert!(scoped
        .rpc("preview.open", json!({"threadId":"t1"}))
        .unwrap_err()
        .to_string()
        .contains("write_not_authorized"));
}

#[test]
fn discovered_local_servers_lists_http_listeners_only_while_subscribed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = [0u8; 512];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\ncontent-length: 0\r\n\r\n");
        }
    });
    // A listener that does not speak HTTP is not a web server.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in silent.incoming() {
            std::mem::forget(s);
        }
    });

    let server = start(|_| {});
    let mut stream = subscribe(
        server.connect(),
        "subscribeDiscoveredLocalServers",
        json!({}),
    );
    let list = stream.until(Duration::from_secs(20), |v| {
        v["servers"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["port"] == port))
    });
    let found = list.last().unwrap();
    let mine = found["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["port"] == port)
        .unwrap();
    assert_eq!(
        (mine["host"].as_str(), mine["url"].as_str()),
        (
            Some("localhost"),
            Some(format!("http://localhost:{port}").as_str())
        )
    );
    assert!(mine["pid"].as_u64().is_some(), "owner resolved from /proc");
    assert!(!found["servers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["port"] == silent_port));
    assert!(found["scannedAt"].is_string());
}
