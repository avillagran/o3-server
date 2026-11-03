//! Shared fixtures: a real server on loopback, the real `t3-protocol` client, scratch dirs on disk
//! (`/var/tmp`, never tmpfs) and fake `code`/`gh` executables on PATH.
#![allow(dead_code)]
// Both harnesses can be used independently in the same test crate. Include
// this stateless helper here without loading its module twice.
include!("../support/signals.rs");
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Once,
    },
};
use t3_protocol::{pair_bearer, Connection, ConnectionOptions, PairingClient};

pub fn scratch_root() -> PathBuf {
    std::env::temp_dir()
}
static INIT: Once = Once::new();
static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn bin_dir() -> PathBuf {
    scratch_root().join(format!("{}-bin", std::process::id()))
}

fn init_process() {
    INIT.call_once(|| {
        let bin = bin_dir();
        let _ = std::fs::remove_dir_all(&bin);
        std::fs::create_dir_all(&bin).unwrap();
        let script = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script("code", &format!("printf '%s\\n' \"$@\" > {}/code.out", bin.display()));
        script("gh", &format!("echo \"$@\" >> {0}/gh.log\ncase \"$1 $2\" in\n\"pr list\") if [ -f .test-gh-fail ]; then exit 1; elif [ -f .test-gh-pr-list.json ]; then cat .test-gh-pr-list.json; else echo '[]'; fi;;\n\"pr create\") echo https://github.com/o/r/pull/7;;\nesac", bin.display()));
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        // SAFETY: set once, before any server thread of this process exists.
        unsafe {
            std::env::set_var("PATH", path);
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_AUTHOR_NAME", "Test");
            std::env::set_var("GIT_AUTHOR_EMAIL", "test@example.invalid");
            std::env::set_var("GIT_COMMITTER_NAME", "Test");
            std::env::set_var("GIT_COMMITTER_EMAIL", "test@example.invalid");
        }
    });
}

pub struct Env {
    pub base: String,
    pub scratch: PathBuf,
    pub state: PathBuf,
    pub conn: Connection,
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

pub fn setup(name: &str) -> Env {
    init_process();
    let scratch = scratch_root().join(format!(
        "{}-{}-{}",
        std::process::id(),
        name,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    let state = scratch.join("userdata");
    std::fs::create_dir_all(&state).unwrap();
    let st = o3_server::store::State::open(&state).unwrap();
    let pairing = st.issue_pairing(Some("test"), 60_000).unwrap()["credential"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(st);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dir = state.clone();
    std::thread::spawn(move || {
        let st = o3_server::store::State::open(&dir).unwrap();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .thread_stack_size(512 * 1024)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                o3_server::serve(
                    tokio::net::TcpListener::from_std(listener).unwrap(),
                    std::sync::Arc::new(st),
                )
                .await
                .unwrap();
            })
    });
    shield_client_thread_from_sigchld();
    let paired = pair_bearer(
        &base,
        &pairing,
        None,
        &PairingClient::default(),
        &ConnectionOptions::default(),
    )
    .unwrap();
    let conn = paired.connection(&ConnectionOptions::default()).unwrap();
    Env {
        base,
        scratch,
        state,
        conn,
    }
}

impl Env {
    pub fn dir(&self, name: &str) -> PathBuf {
        let p = self.scratch.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    pub fn project(&self, id: &str, root: &Path) -> Value {
        self.conn.rpc("projects.mutate", json!({"type":"project.create","commandId":format!("pc-{id}"),"projectId":id,"title":id,"workspaceRoot":root.to_str().unwrap(),"scripts":[]})).unwrap()
    }
    pub fn thread(&self, id: &str, project: &str) {
        self.conn.rpc("orchestration.dispatchCommand", json!({"type":"thread.create","commandId":format!("tc-{id}"),"threadId":id,"projectId":project,"title":format!("Thread {id}"),"modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
    }
    pub fn message(&self, thread: &str, id: &str, text: &str) {
        self.conn.rpc("orchestration.dispatchCommand", json!({"type":"message.dispatch","commandId":format!("m-{id}"),"messageId":id,"threadId":thread,"text":text,"attachments":[],"dispatchMode":{"type":"defer_start"}})).unwrap();
    }
    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.state.join("o3.sqlite")).unwrap()
    }
}

/// Minimal HTTP/1.1 client: (status, lowercase header block, body).
pub fn http(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let addr = base.trim_start_matches("http://");
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !headers.iter().any(|(k, _)| {
        k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("transfer-encoding")
    }) {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}
pub fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
}
pub fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}
pub fn write(path: &Path, text: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Collects stream items until `stop` says so (or the stream ends).
pub fn collect(
    conn: &Connection,
    tag: &str,
    payload: Value,
    stop: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    let done = std::cell::Cell::new(false);
    let mut items = Vec::new();
    let _ = conn.subscribe(
        tag,
        payload,
        || done.get(),
        |item| {
            if let Some(item) = item {
                if stop(&item) {
                    done.set(true);
                }
                items.push(item);
            }
            Ok(())
        },
    );
    items
}

pub fn err_text(r: t3_protocol::Result<Value>) -> String {
    r.expect_err("expected an error").to_string()
}
