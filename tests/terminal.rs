//! Terminal conformance: the real `t3-protocol` client against o3-server's PTY layer.
mod support;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use support::{start, subscribe, wait_for, Server, Stream};
use t3_protocol::{terminal::Output, Connection};

const T: Duration = Duration::from_secs(10);

/// A shell with a quiet environment and a recognisable prompt.
fn server() -> (Server, PathBuf) {
    let s = start(|state| {
        let dir = std::env::temp_dir().join(format!(
            "o3-shell-{}-{}",
            std::process::id(),
            state.environment_id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("shell.sh");
        std::fs::write(&script, "#!/bin/sh\nexec env -i PATH=\"$PATH\" HOME=/nonexistent PS1='READY> ' TERM=xterm /bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        state.streams.terminals.set_shell(script.to_str().unwrap());
    });
    let cwd = s.dir.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    (s, cwd)
}

fn open(c: &Connection, thread: &str, id: &str, cwd: &PathBuf) -> Value {
    c.rpc(
        "terminal.open",
        json!({"threadId":thread,"terminalId":id,"cwd":cwd,"cols":100,"rows":30}),
    )
    .unwrap()
}
fn write(c: &Connection, thread: &str, id: &str, data: &str) {
    c.terminal_write(thread, id, data).unwrap();
}
fn attach(c: Connection, thread: &str, id: &str, extra: Value) -> Stream {
    let mut p = json!({"threadId":thread,"terminalId":id});
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    subscribe(c, "terminal.attach", p)
}

/// Reads until the output seen so far contains `needle` (events may cut it anywhere); returns that output.
fn read_until_text(stream: &mut Stream, within: Duration, needle: &str) -> String {
    let mut text = String::new();
    stream.until(within, |e| {
        if e["type"] == "output" {
            text.push_str(e["data"].as_str().unwrap());
        }
        text.contains(needle)
    });
    text
}

/// What a renderer would show: snapshots replace, output appends, using the client's own state machine.
#[derive(Default)]
struct Screen {
    text: String,
    state: Output,
}
impl Screen {
    fn feed(&mut self, event: &Value) {
        match event["type"].as_str().unwrap() {
            "snapshot" | "restarted" | "started" => {
                self.text = self.state.apply(event).unwrap().unwrap_or_default();
            }
            "cleared" => {
                self.state.apply(event).unwrap();
                self.text.clear();
            }
            _ => {
                if let Some(t) = self.state.apply(event).unwrap() {
                    self.text.push_str(&t);
                }
            }
        }
    }
}

#[test]
fn open_returns_a_running_snapshot_and_write_reaches_the_shell() {
    let (s, cwd) = server();
    let c = s.connect();
    let snap = open(&c, "t1", "term-1", &cwd);
    assert_eq!(snap["status"], "running");
    assert!(snap["pid"].as_i64().unwrap() > 0);
    assert_eq!(snap["label"], "Terminal 1");
    assert_eq!(snap["cwd"], json!(cwd));
    assert_eq!(snap["worktreePath"], Value::Null);
    assert!(snap["sequence"].as_u64().unwrap() >= 1);
    let mut obs = subscribe(
        s.connect(),
        "terminal.observe",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    let mut screen = Screen::default();
    screen.feed(&obs.next(T));
    write(&c, "t1", "term-1", "echo marker-$((20+22))\n");
    for e in obs.until(T, |e| {
        screen.feed(e);
        screen.text.contains("marker-42\r\n")
    }) {
        let _ = e;
    }
    // The same session answers from another connection (each `rpc` call is its own websocket).
    assert_eq!(open(&c, "t1", "term-1", &cwd)["pid"], snap["pid"]);
    // Missing sessions and bad directories are typed failures.
    let err = c
        .rpc(
            "terminal.write",
            json!({"threadId":"t1","terminalId":"nope","data":"x"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("TerminalSessionLookupError"), "{err}");
    let err = c
        .rpc(
            "terminal.open",
            json!({"threadId":"t1","terminalId":"term-2","cwd":"/nonexistent-dir"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("TerminalCwdNotFoundError"), "{err}");
    let err = c
        .rpc(
            "terminal.open",
            json!({"threadId":"t1","terminalId":"term-2","cwd":"/etc/hostname"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("TerminalCwdNotDirectoryError"), "{err}");
}

#[test]
fn attach_snapshot_then_live_tail_loses_and_duplicates_nothing() {
    let (s, cwd) = server();
    let c = s.connect();
    open(&c, "t1", "term-1", &cwd);
    // Output is flowing while the subscriber attaches, so the snapshot lands mid-stream.
    write(
        &c,
        "t1",
        "term-1",
        "i=0; while [ $i -lt 4000 ]; do echo line$i; i=$((i+1)); done; echo FINISHED\n",
    );
    let mut stream = attach(s.connect(), "t1", "term-1", json!({}));
    let first = stream.next(T);
    assert_eq!(first["type"], "snapshot");
    let mut screen = Screen::default();
    screen.feed(&first);
    let finished = |screen: &Screen| {
        screen.text.contains("\r\nFINISHED\r\n") && screen.text.ends_with("READY> ")
    };
    // The snapshot may already hold everything if the shell was quick.
    if !finished(&screen) {
        stream.until(Duration::from_secs(30), |e| {
            screen.feed(e);
            finished(&screen)
        });
    }

    // A fresh snapshot is the ground truth for the same session.
    let mut check = subscribe(
        s.connect(),
        "terminal.observe",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    let truth = check.next(T)["snapshot"]["history"]
        .as_str()
        .unwrap()
        .to_owned();
    let rendered = screen.text.clone();
    // Output lines are `lineN` terminated by CRLF; the echoed command has `line$i` instead.
    let mut numbers = Vec::new();
    for part in rendered.split("\r\n") {
        let tail = part.rsplit('\r').next().unwrap();
        if let Some(n) = tail
            .strip_prefix("line")
            .and_then(|r| r.parse::<u32>().ok())
        {
            numbers.push(n);
        }
    }
    assert_eq!(
        numbers,
        (0..4000).collect::<Vec<_>>(),
        "every line exactly once, in order"
    );
    assert_eq!(rendered, truth);
}

#[test]
fn resize_clear_restart_and_exit_are_reported_on_the_stream() {
    let (s, cwd) = server();
    let c = s.connect();
    let first = open(&c, "t1", "term-1", &cwd);
    let mut stream = attach(s.connect(), "t1", "term-1", json!({}));
    stream.next(T);
    c.terminal_resize("t1", "term-1", 77, 21).unwrap();
    write(&c, "t1", "term-1", "stty size\n");
    read_until_text(&mut stream, T, "21 77");
    c.rpc(
        "terminal.clear",
        json!({"threadId":"t1","terminalId":"term-1"}),
    )
    .unwrap();
    let cleared = stream.until(T, |e| e["type"] == "cleared");
    assert!(cleared.last().unwrap()["sequence"].as_u64().unwrap() > 0);
    let snap = {
        let mut o = subscribe(
            s.connect(),
            "terminal.observe",
            json!({"threadId":"t1","terminalId":"term-1"}),
        );
        o.next(T)["snapshot"].clone()
    };
    assert!(!snap["history"].as_str().unwrap().contains("21 77"));
    let restarted = c
        .rpc(
            "terminal.restart",
            json!({"threadId":"t1","terminalId":"term-1","cwd":cwd,"cols":90,"rows":25}),
        )
        .unwrap();
    assert_ne!(restarted["pid"], first["pid"]);
    let e = stream.until(T, |e| e["type"] == "restarted");
    assert_eq!(e.last().unwrap()["snapshot"]["pid"], restarted["pid"]);
    write(&c, "t1", "term-1", "exit 3\n");
    let exited = stream.until(T, |e| e["type"] == "exited");
    assert_eq!(exited.last().unwrap()["exitCode"], 3);
    assert_eq!(exited.last().unwrap()["exitSignal"], Value::Null);
    // Writing to an exited terminal is not an error; the session stays for inspection.
    c.terminal_write("t1", "term-1", "x").unwrap();
    // Attaching with restartIfNotRunning starts a fresh shell, with a fresh pid.
    let mut again = attach(
        s.connect(),
        "t1",
        "term-1",
        json!({"cwd":cwd,"restartIfNotRunning":true}),
    );
    let snap = again.next(T);
    assert_eq!(snap["snapshot"]["status"], "running");
    assert_ne!(snap["snapshot"]["pid"], restarted["pid"]);
    // `resume` preserves a stopped terminal instead of restarting it.
    write(&c, "t1", "term-1", "exit\n");
    again.until(T, |e| e["type"] == "exited");
    let mut resumed = attach(
        s.connect(),
        "t1",
        "term-1",
        json!({"cwd":cwd,"resume":true,"restartIfNotRunning":true}),
    );
    assert_eq!(resumed.next(T)["snapshot"]["status"], "exited");
}

fn is_dead(pid: i64) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

#[test]
fn close_kills_the_whole_process_group_and_tells_subscribers() {
    let (s, cwd) = server();
    let c = s.connect();
    open(&c, "t1", "term-1", &cwd);
    let mut stream = attach(s.connect(), "t1", "term-1", json!({}));
    stream.next(T);
    write(&c, "t1", "term-1", "sleep 300 & echo BG=$!\n");
    let mut screen = Screen::default();
    let mut pid = None;
    stream.until(T, |e| {
        screen.feed(e);
        // The echoed command reads `BG=$!`; the shell's answer is `BG=<digits>` and a newline.
        pid = screen.text.split("BG=").skip(1).find_map(|r| {
            let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty() && r[digits.len()..].starts_with('\r'))
                .then(|| digits.parse::<i64>().unwrap())
        });
        pid.is_some()
    });
    let pid = pid.expect("background pid");
    assert!(!is_dead(pid));
    c.rpc(
        "terminal.close",
        json!({"threadId":"t1","terminalId":"term-1"}),
    )
    .unwrap();
    stream.until(T, |e| e["type"] == "closed");
    assert!(
        wait_for(Duration::from_secs(5), || is_dead(pid)),
        "background job {pid} survived close"
    );
    let err = c
        .rpc(
            "terminal.write",
            json!({"threadId":"t1","terminalId":"term-1","data":"x"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("TerminalSessionLookupError"), "{err}");
    // A resumed attach to a closed terminal reports it closed instead of recreating it.
    let mut again = attach(s.connect(), "t1", "term-1", json!({"resume":true}));
    assert_eq!(again.next(T)["type"], "closed");
}

#[test]
fn multibyte_output_split_across_reads_is_decoded_incrementally() {
    let (s, cwd) = server();
    let c = s.connect();
    open(&c, "t1", "term-1", &cwd);
    let mut stream = attach(s.connect(), "t1", "term-1", json!({}));
    stream.next(T);
    write(&c, "t1", "term-1", "printf '\\342\\202'; sleep 0.4; printf '\\254 \\360\\237'; sleep 0.4; printf '\\230\\200 END\\n'\n");
    let mut seen = String::new();
    let mut pieces = 0;
    stream.until(T, |e| {
        if e["type"] == "output" {
            pieces += 1;
            seen.push_str(e["data"].as_str().unwrap());
        }
        seen.contains("END\r\n")
    });
    assert!(
        pieces >= 3,
        "the writes were delayed, so they arrive as separate events: {pieces}"
    );
    assert!(seen.contains("€ 😀 END"), "{seen:?}");
    assert!(!seen.contains('\u{FFFD}'), "{seen:?}");
}

#[test]
fn scrollback_is_bounded_per_terminal() {
    let (s, cwd) = server();
    let c = s.connect();
    open(&c, "t1", "term-1", &cwd);
    let mut stream = attach(s.connect(), "t1", "term-1", json!({}));
    stream.next(T);
    // 2 MiB of numbered lines: far more than the 512 KiB scrollback.
    write(&c, "t1", "term-1", "i=0; while [ $i -lt 40000 ]; do echo \"scrollback-line-$i-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; i=$((i+1)); done; echo DONE-$i\n");
    // A subscriber that cannot keep up is resynchronised with a snapshot, which replaces its screen.
    let mut screen = Screen::default();
    stream.until(Duration::from_secs(60), |e| {
        screen.feed(e);
        screen.text.contains("DONE-40000\r\n")
    });
    assert!(screen.text.len() <= 512 * 1024 + 64 * 1024);
    let mut check = subscribe(
        s.connect(),
        "terminal.observe",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    let history = check.next(T)["snapshot"]["history"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(history.len() <= 512 * 1024, "{}", history.len());
    assert!(history.len() > 256 * 1024);
    assert!(history.contains("DONE-40000"));
    assert!(
        !history.contains("scrollback-line-0-"),
        "the oldest output must be gone"
    );
    assert!(history.lines().count() <= 5000);
}

#[test]
fn metadata_stream_follows_terminal_lifecycle() {
    let (s, cwd) = server();
    let c = s.connect();
    let mut meta = subscribe(s.connect(), "subscribeTerminalMetadata", json!({}));
    assert_eq!(meta.next(T), json!({"type":"snapshot","terminals":[]}));
    let mut events = subscribe(s.connect(), "subscribeTerminalEvents", json!({}));
    std::thread::sleep(Duration::from_millis(300));
    open(&c, "t1", "term-1", &cwd);
    let up = meta.until(T, |e| e["type"] == "upsert");
    let t = &up.last().unwrap()["terminal"];
    assert_eq!(
        (
            t["threadId"].as_str(),
            t["terminalId"].as_str(),
            t["status"].as_str()
        ),
        (Some("t1"), Some("term-1"), Some("running"))
    );
    assert_eq!(t["hasRunningSubprocess"], false);
    // A child process makes the activity flag flip, with the command as its label.
    write(&c, "t1", "term-1", "sleep 30\n");
    let busy = meta.until(Duration::from_secs(8), |e| {
        e["type"] == "upsert" && e["terminal"]["hasRunningSubprocess"] == true
    });
    assert_eq!(busy.last().unwrap()["terminal"]["label"], "sleep");
    c.rpc(
        "terminal.close",
        json!({"threadId":"t1","terminalId":"term-1"}),
    )
    .unwrap();
    events.until(T, |e| e["type"] == "closed" && e["terminalId"] == "term-1");
    let gone = meta.until(T, |e| e["type"] == "remove");
    assert_eq!(gone.last().unwrap()["terminalId"], "term-1");
}

#[test]
fn terminal_scopes_split_read_from_operate() {
    let (s, cwd) = server();
    let owner = s.connect();
    open(&owner, "t1", "term-1", &cwd);
    let reader = s.limited(&owner, &["terminal:read"]);
    let mut obs = subscribe(
        reader.clone(),
        "terminal.observe",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    assert_eq!(obs.next(T)["type"], "snapshot");
    let err = reader
        .rpc(
            "terminal.write",
            json!({"threadId":"t1","terminalId":"term-1","data":"x"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("terminal:operate"), "{err}");
    let err = reader
        .rpc(
            "terminal.open",
            json!({"threadId":"t1","terminalId":"term-1","cwd":cwd}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("terminal:operate"), "{err}");
    let mut denied = subscribe(
        reader.clone(),
        "terminal.attach",
        json!({"threadId":"t1","terminalId":"term-1"}),
    );
    let end = wait_for(T, || denied.ended().is_some());
    assert!(end, "attach with only terminal:read must be refused");
    let _ = denied.try_next(Duration::from_millis(10));
}

#[test]
fn output_flow_control_bounds_unacknowledged_chunks() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let (s, cwd) = server();
    let c = s.connect();
    open(&c, "t1", "term-1", &cwd);
    let secret = s.state.issue_pairing(Some("raw"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = t3_protocol::pair_bearer(
        &s.base,
        &secret,
        None,
        &t3_protocol::PairingClient::default(),
        &t3_protocol::ConnectionOptions::default(),
    )
    .unwrap()
    .access_token
    .to_string();
    let addr = s.base.trim_start_matches("http://").to_owned();
    // A producer far faster than a client that never acknowledges.
    write(
        &c,
        "t1",
        "term-1",
        "yes 0123456789abcdef0123456789abcdef | head -c 3000000; echo ENDED\n",
    );
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
        let mut request = format!("ws://{addr}/ws?orchestrationProtocol=2").into_client_request().unwrap();
        request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::client_async(request, tcp).await.unwrap();
        ws.send(Message::Text(json!({"_tag":"Request","id":"a","tag":"terminal.attach","payload":{"threadId":"t1","terminalId":"term-1"},"headers":[]}).to_string().into())).await.unwrap();
        let mut chunks = 0;
        let mut bytes = 0;
        while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(800), ws.next()).await {
            let text = m.into_text().unwrap();
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["_tag"] == "Chunk" {
                chunks += 1;
                bytes += text.len();
            }
        }
        assert!((1..=8).contains(&chunks), "unacknowledged chunks must stay within the window: {chunks}");
        assert!(bytes < 8 * 80 * 1024, "{bytes}");
        // Acknowledging what it received reopens the window.
        for _ in 0..chunks {
            ws.send(Message::Text(json!({"_tag":"Ack","requestId":"a"}).to_string().into())).await.unwrap();
        }
        // Acknowledging as it goes, the subscriber reaches the present: a later write arrives.
        write(&c, "t1", "term-1", "echo AFTER-ACK\n");
        let mut seen = String::new();
        while !seen.contains("AFTER-ACK") {
            let m = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.expect("chunk after ack").unwrap().unwrap();
            let text = m.into_text().unwrap();
            if serde_json::from_str::<Value>(&text).unwrap()["_tag"] == "Chunk" {
                seen.push_str(&text);
                ws.send(Message::Text(json!({"_tag":"Ack","requestId":"a"}).to_string().into())).await.unwrap();
            }
        }
    });
}
