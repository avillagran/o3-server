//! PTY terminals: the manager that backs `terminal.*` and the terminal subscriptions.
//!
//! Memory is bounded per terminal: scrollback (`History`, 512 KiB / 5000 lines), a replay log of
//! recent events (`SESSION_LOG_BUDGET`) and one shared ring for the manager-wide streams
//! (`GLOBAL_LOG_BUDGET`). Subscribers hold a sequence cursor, never a queue; one that falls behind
//! the retained log is resynchronised with a fresh snapshot, so output is never lost or reordered.
use crate::{
    pty,
    store::{iso, now_ms, State},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    os::fd::OwnedFd,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::unix::AsyncFd, sync::oneshot};

pub type Notifier = Arc<tokio::sync::watch::Sender<u64>>;
pub fn bump(n: &Notifier) {
    n.send_modify(|v| *v = v.wrapping_add(1));
}

const HISTORY_BYTES: usize = 512 * 1024;
const HISTORY_LINES: usize = 5_000;
const SESSION_LOG_BUDGET: usize = 192 * 1024;
const GLOBAL_LOG_BUDGET: usize = 256 * 1024;
const EVENT_OVERHEAD: usize = 128;
const BATCH_BYTES: usize = 64 * 1024;
const WINDOW_CHUNKS: usize = 8;
const KILL_GRACE: Duration = Duration::from_millis(1_000);
const MAX_INACTIVE: usize = 128;
const MAX_LIVE: usize = 64;
const MAX_PENDING_CONTROL: usize = 8 * 1024;
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;
async fn await_scope(completion: Option<tokio::sync::watch::Receiver<bool>>) -> Result<(), String> {
    if let Some(mut done) = completion {
        while !*done.borrow() {
            done.changed()
                .await
                .map_err(|_| "Setup process scope ended without confirming cleanup".to_owned())?;
        }
    }
    Ok(())
}
const BLOCKLIST: [&str; 10] = [
    "PORT",
    "ELECTRON_RENDERER_PORT",
    "ELECTRON_RUN_AS_NODE",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "WINDOW",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
];

// ---------------------------------------------------------------- incremental UTF-8

/// Splits PTY bytes at character boundaries: a multi-byte character cut by a read waits for its tail.
#[derive(Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}
impl Utf8Stream {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut out = String::with_capacity(data.len());
        let mut rest: &[u8] = &data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    break;
                }
                Err(e) => {
                    let (valid, tail) = rest.split_at(e.valid_up_to());
                    out.push_str(std::str::from_utf8(valid).unwrap_or(""));
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            rest = &tail[n..];
                        }
                        None => {
                            self.pending = tail.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
    pub fn finish(&mut self) -> String {
        if std::mem::take(&mut self.pending).is_empty() {
            String::new()
        } else {
            "\u{FFFD}".into()
        }
    }
}

// ---------------------------------------------------------------- scrollback

/// Bounded scrollback: a deque of chunks trimmed by bytes and by lines.
pub struct History {
    chunks: VecDeque<String>,
    bytes: usize,
    breaks: usize,
    max_bytes: usize,
    max_lines: usize,
}
impl History {
    pub fn new(max_bytes: usize, max_lines: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            breaks: 0,
            max_bytes,
            max_lines,
        }
    }
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
        self.breaks = 0;
    }
    pub fn len(&self) -> usize {
        self.bytes
    }
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }
    pub fn append(&mut self, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(16 * 1024);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (piece, tail) = rest.split_at(end);
            let room = self
                .chunks
                .back()
                .is_some_and(|c| c.len() + piece.len() <= 16 * 1024);
            if room {
                self.chunks.back_mut().unwrap().push_str(piece);
            } else {
                self.chunks.push_back(piece.to_owned());
            }
            self.bytes += piece.len();
            self.breaks += piece.bytes().filter(|b| *b == b'\n').count();
            rest = tail;
            self.trim();
        }
    }
    fn drop_front(&mut self, count: usize) {
        let Some(first) = self.chunks.front_mut() else {
            return;
        };
        if count >= first.len() {
            let c = self.chunks.pop_front().unwrap();
            self.bytes -= c.len();
            self.breaks -= c.bytes().filter(|b| *b == b'\n').count();
        } else {
            let mut cut = count;
            while !first.is_char_boundary(cut) {
                cut += 1;
            }
            let removed: String = first.drain(..cut).collect();
            self.bytes -= removed.len();
            self.breaks -= removed.bytes().filter(|b| *b == b'\n').count();
            if first.is_empty() {
                self.chunks.pop_front();
            }
        }
    }
    fn trim(&mut self) {
        while self.bytes > self.max_bytes {
            self.drop_front(self.bytes - self.max_bytes);
        }
        loop {
            let trailing = self.chunks.back().is_some_and(|c| c.ends_with('\n'));
            let lines = self.breaks + usize::from(!trailing);
            if lines <= self.max_lines || self.chunks.is_empty() {
                break;
            }
            let mut drop_lines = lines - self.max_lines;
            let first = self.chunks.front().unwrap();
            let first_breaks = first.bytes().filter(|b| *b == b'\n').count();
            if first_breaks < drop_lines {
                let n = first.len();
                self.drop_front(n);
                continue;
            }
            let mut offset = 0;
            while drop_lines > 0 {
                offset += first[offset..].find('\n').map_or(first.len(), |i| i + 1);
                drop_lines -= 1;
            }
            self.drop_front(offset);
        }
    }
    pub fn value(&self) -> String {
        let mut out = String::with_capacity(self.bytes);
        for c in &self.chunks {
            out.push_str(c);
        }
        out
    }
}

// ---------------------------------------------------------------- history sanitizer

fn csi_final(c: char) -> bool {
    ('\u{40}'..='\u{7e}').contains(&c)
}
fn strip_csi(body: &str, fin: char) -> bool {
    let digits = |s: &str, extra: &str| s.chars().all(|c| c.is_ascii_digit() || extra.contains(c));
    match fin {
        'n' => true,
        'R' => digits(body, ";?"),
        'c' => digits(body, ">;?"),
        'p' | 'y' => body.ends_with('$') && digits(&body[..body.len() - 1], ";?"),
        'q' => body.starts_with('>') && digits(&body[1..], ";"),
        'u' => body.starts_with('?'),
        _ => false,
    }
}
fn strip_osc(content: &str) -> bool {
    ["10", "11", "12"].iter().any(|n| {
        content
            .strip_prefix(n)
            .and_then(|r| r.strip_prefix(';'))
            .is_some_and(|r| r.starts_with('?') || r.starts_with("rgb:"))
    })
}
fn strip_dcs(content: &str) -> bool {
    let c = content.strip_prefix(['0', '1']).unwrap_or(content);
    let mut it = c.chars();
    matches!(it.next(), Some('$' | '+')) && matches!(it.next(), Some('q' | 'r'))
}
fn string_end(input: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i < input.len() {
        match input[i] {
            '\u{7}' | '\u{9c}' => return Some(i + 1),
            '\u{1b}' if input.get(i + 1) == Some(&'\\') => return Some(i + 2),
            _ => i += 1,
        }
    }
    None
}
fn string_content(seq: &[char]) -> String {
    let s: String = seq.iter().collect();
    s.strip_suffix("\u{1b}\\")
        .or_else(|| s.strip_suffix(['\u{7}', '\u{9c}']))
        .unwrap_or(&s)
        .to_owned()
}

/// Removes terminal query traffic (DSR, DA, DECRQM, OSC colour queries...) from what is stored as
/// scrollback, so replaying history cannot make a renderer answer questions nobody asked.
/// A control sequence cut by the chunk boundary is returned as `pending` for the next call.
pub fn sanitize_history(pending: &str, data: &str) -> (String, String) {
    let interesting = |c: char| {
        matches!(
            c,
            '\u{1b}' | '\u{90}' | '\u{9b}' | '\u{9d}' | '\u{9e}' | '\u{9f}'
        )
    };
    if pending.is_empty() && !data.chars().any(interesting) {
        return (data.to_owned(), String::new());
    }
    let input: Vec<char> = pending.chars().chain(data.chars()).collect();
    let scan_start = pending.chars().count().saturating_sub(1);
    let mut out = String::new();
    let mut i = 0;
    let rest = |i: usize| input[i..].iter().collect::<String>();
    while i < input.len() {
        let c = input[i];
        if c == '\u{1b}' {
            let Some(&next) = input.get(i + 1) else {
                return (out, rest(i));
            };
            if next == '[' {
                match input[i + 2..].iter().position(|c| csi_final(*c)) {
                    Some(p) => {
                        let end = i + 2 + p;
                        let body: String = input[i + 2..end].iter().collect();
                        if !strip_csi(&body, input[end]) {
                            out.extend(&input[i..=end]);
                        }
                        i = end + 1;
                    }
                    None => return (out, rest(i)),
                }
                continue;
            }
            if matches!(next, ']' | 'P' | '^' | '_') {
                let Some(end) = string_end(&input, (i + 2).max(scan_start)) else {
                    return (out, rest(i));
                };
                let content = string_content(&input[i + 2..end]);
                let strip =
                    (next == ']' && strip_osc(&content)) || (next == 'P' && strip_dcs(&content));
                if !strip {
                    out.extend(&input[i..end]);
                }
                i = end;
                continue;
            }
            // Other escapes: intermediates (0x20..=0x2f) then one final byte.
            let mut j = i + 1;
            while j < input.len() && ('\u{20}'..='\u{2f}').contains(&input[j]) {
                j += 1;
            }
            if j >= input.len() {
                return (out, rest(i));
            }
            let end = if ('\u{30}'..='\u{7e}').contains(&input[j]) {
                j + 1
            } else {
                i + 1
            };
            out.extend(&input[i..end]);
            i = end;
            continue;
        }
        if c == '\u{9b}' {
            match input[i + 1..].iter().position(|c| csi_final(*c)) {
                Some(p) => {
                    let end = i + 1 + p;
                    let body: String = input[i + 1..end].iter().collect();
                    if !strip_csi(&body, input[end]) {
                        out.extend(&input[i..=end]);
                    }
                    i = end + 1;
                }
                None => return (out, rest(i)),
            }
            continue;
        }
        if matches!(c, '\u{9d}' | '\u{90}' | '\u{9e}' | '\u{9f}') {
            let Some(end) = string_end(&input, (i + 1).max(scan_start)) else {
                return (out, rest(i));
            };
            let content = string_content(&input[i + 1..end]);
            let strip =
                (c == '\u{9d}' && strip_osc(&content)) || (c == '\u{90}' && strip_dcs(&content));
            if !strip {
                out.extend(&input[i..end]);
            }
            i = end;
            continue;
        }
        let run_end = input[i + 1..]
            .iter()
            .position(|c| interesting(*c))
            .map_or(input.len(), |p| i + 1 + p);
        out.extend(&input[i..run_end]);
        i = run_end;
    }
    (out, String::new())
}

// ---------------------------------------------------------------- sessions

struct Logged {
    seq: u64,
    size: usize,
    event: Arc<Value>,
}

struct Proc {
    completion: Option<tokio::sync::watch::Receiver<bool>>,
    master: Arc<AsyncFd<OwnedFd>>,
    /// Dropping (or sending on) this stops the process group: HUP and TERM, then KILL after a grace period.
    _stop: oneshot::Sender<()>,
}

struct SessionState {
    scoped: bool,
    shell_program: String,
    cwd: String,
    worktree_path: Option<String>,
    env: Option<BTreeMap<String, String>>,
    status: &'static str,
    pid: Option<i32>,
    exit_code: Option<i32>,
    exit_signal: Option<i32>,
    history: History,
    pending_ctl: String,
    updated_at: String,
    seq: u64,
    input_count: u64,
    cols: u16,
    rows: u16,
    has_subprocess: bool,
    child_label: Option<String>,
    gen: u64,
    proc: Option<Proc>,
    log: VecDeque<Logged>,
    log_bytes: usize,
    closed: bool,
}

pub struct Session {
    pub thread_id: String,
    pub terminal_id: String,
    st: Mutex<SessionState>,
    notify: Notifier,
    global: Arc<Mutex<Ring>>,
    persistence: Mutex<Option<std::path::PathBuf>>,
}

#[derive(Default)]
pub struct Ring {
    head: u64,
    bytes: usize,
    items: VecDeque<(u64, usize, Arc<Value>)>,
}
impl Ring {
    fn push(&mut self, size: usize, event: Arc<Value>) {
        self.head += 1;
        self.bytes += size;
        self.items.push_back((self.head, size, event));
        while self.bytes > GLOBAL_LOG_BUDGET && self.items.len() > 1 {
            let (_, s, _) = self.items.pop_front().unwrap();
            self.bytes -= s;
        }
    }
}

pub fn terminal_label(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("terminal-")
        .or_else(|| lower.strip_prefix("term-"));
    if let Some(rest) = rest {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let tail = &rest[digits.len()..];
        if !digits.is_empty() && (tail.is_empty() || is_uuid_suffix(tail)) {
            return format!("Terminal {digits}");
        }
    }
    id.to_owned()
}
fn is_uuid_suffix(s: &str) -> bool {
    let Some(u) = s.strip_prefix('-') else {
        return false;
    };
    let parts: Vec<&str> = u.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

impl SessionState {
    fn label(&self, terminal_id: &str) -> String {
        let l = if self.has_subprocess {
            self.child_label
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
        } else {
            None
        };
        let label = l.map_or_else(|| terminal_label(terminal_id), str::to_owned);
        label.chars().take(128).collect()
    }
    fn advance(&mut self) -> u64 {
        self.updated_at = iso(now_ms());
        self.seq += 1;
        self.seq
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    fn new(
        thread: &str,
        terminal: &str,
        cwd: &str,
        worktree: Option<String>,
        env: Option<BTreeMap<String, String>>,
        cols: u16,
        rows: u16,
        notify: Notifier,
        global: Arc<Mutex<Ring>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            thread_id: thread.into(),
            terminal_id: terminal.into(),
            notify,
            global,
            persistence: Mutex::new(None),
            st: Mutex::new(SessionState {
                scoped: false,
                shell_program: String::new(),
                cwd: cwd.into(),
                worktree_path: worktree,
                env,
                status: "starting",
                pid: None,
                exit_code: None,
                exit_signal: None,
                history: History::new(HISTORY_BYTES, HISTORY_LINES),
                pending_ctl: String::new(),
                updated_at: iso(now_ms()),
                seq: 0,
                input_count: 0,
                cols,
                rows,
                has_subprocess: false,
                child_label: None,
                gen: 0,
                proc: None,
                log: VecDeque::new(),
                log_bytes: 0,
                closed: false,
            }),
        })
    }

    fn snapshot_of(&self, st: &SessionState) -> Value {
        json!({"threadId":self.thread_id,"terminalId":self.terminal_id,"cwd":st.cwd,"worktreePath":st.worktree_path,"status":st.status,"pid":st.pid,"history":st.history.value(),"cols":st.cols,"rows":st.rows,"exitCode":st.exit_code,"exitSignal":st.exit_signal,"label":st.label(&self.terminal_id),"updatedAt":st.updated_at,"sequence":st.seq})
    }
    pub fn snapshot(&self) -> Value {
        self.snapshot_of(&self.st.lock().unwrap())
    }
    pub fn summary(&self) -> Value {
        let st = self.st.lock().unwrap();
        json!({"threadId":self.thread_id,"terminalId":self.terminal_id,"cwd":st.cwd,"worktreePath":st.worktree_path,"status":st.status,"pid":st.pid,"exitCode":st.exit_code,"exitSignal":st.exit_signal,"hasRunningSubprocess":st.has_subprocess,"label":st.label(&self.terminal_id),"updatedAt":st.updated_at})
    }

    /// Records one event under the session lock, in both replay logs, and wakes subscribers.
    fn publish(&self, st: &mut SessionState, mut event: Value, seq: u64) {
        if let Some(path) = self.persistence.lock().unwrap().as_ref() {
            let result = if st.closed {
                std::fs::remove_file(path)
            } else {
                let temp = path.with_extension("pending");
                std::fs::write(&temp, self.snapshot_of(st).to_string())
                    .and_then(|_| std::fs::rename(temp, path))
            };
            if let Err(error) = result {
                eprintln!("terminal history persistence failed: {error}");
            }
        }
        event["threadId"] = json!(self.thread_id);
        event["terminalId"] = json!(self.terminal_id);
        event["sequence"] = json!(seq);
        let size = EVENT_OVERHEAD
            + event["data"].as_str().map_or(0, str::len)
            + event["snapshot"]["history"].as_str().map_or(0, str::len);
        let event = Arc::new(event);
        st.log.push_back(Logged {
            seq,
            size,
            event: event.clone(),
        });
        st.log_bytes += size;
        while st.log_bytes > SESSION_LOG_BUDGET && st.log.len() > 1 {
            st.log_bytes -= st.log.pop_front().unwrap().size;
        }
        self.global.lock().unwrap().push(size, event);
        bump(&self.notify);
    }

    fn output(&self, gen: u64, text: String) {
        if text.is_empty() {
            return;
        }
        let mut st = self.st.lock().unwrap();
        if st.gen != gen || st.status != "running" {
            return;
        }
        let (visible, pending) = sanitize_history(&st.pending_ctl, &text);
        st.pending_ctl = if pending.len() > MAX_PENDING_CONTROL {
            String::new()
        } else {
            pending
        };
        if !visible.is_empty() {
            st.history.append(&visible);
        }
        let seq = st.advance();
        self.publish(&mut st, json!({"type":"output","data":text}), seq);
    }

    fn exited(&self, gen: u64, code: Option<i32>, signal: Option<i32>) {
        let mut st = self.st.lock().unwrap();
        if st.gen != gen || st.status != "running" {
            return;
        }
        st.proc = None;
        st.pid = None;
        st.has_subprocess = false;
        st.child_label = None;
        st.status = "exited";
        st.pending_ctl.clear();
        st.exit_code = code;
        st.exit_signal = signal;
        let seq = st.advance();
        self.publish(
            &mut st,
            json!({"type":"exited","exitCode":code,"exitSignal":signal}),
            seq,
        );
    }

    /// Detaches the process: its supervisor kills the group, and its events are ignored from now on.
    fn stop_process(st: &mut SessionState) {
        if st.proc.take().is_some() {
            st.gen += 1;
            st.pid = None;
            st.has_subprocess = false;
            st.child_label = None;
            st.status = "exited";
            st.pending_ctl.clear();
            st.updated_at = iso(now_ms());
        }
    }
}

// ---------------------------------------------------------------- pump (one task per live process)

async fn kill_group(child: &mut pty::Child, pid: i32) {
    if child.stop_scope() {
        let _ = child.wait().await;
        return;
    }
    pty::signal_session(pid, libc::SIGHUP);
    pty::signal_session(pid, libc::SIGTERM);
    let timed_out = tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err();
    // A shell can exit before its job-control groups do. Always finish those
    // session members, even when waiting for the leader already succeeded.
    pty::signal_session(pid, libc::SIGKILL);
    if timed_out {
        let _ = child.wait().await;
    }
}

async fn pump(
    sess: Arc<Session>,
    gen: u64,
    master: Arc<AsyncFd<OwnedFd>>,
    mut child: pty::Child,
    pid: i32,
    mut stop: oneshot::Receiver<()>,
) {
    use std::os::unix::process::ExitStatusExt;
    let scoped = child.completion().is_some();
    let mut buf = vec![0u8; 16 * 1024];
    let mut decoder = Utf8Stream::default();
    let mut status = None;
    let mut eof = false;
    while status.is_none() && !eof {
        tokio::select! {
            biased;
            _ = &mut stop => {
                kill_group(&mut child, pid).await;
                return;
            }
            result = child.wait() => status = Some(result),
            ready = master.readable() => {
                let Ok(mut guard) = ready else { eof = true; continue };
                match guard.try_io(|fd| pty::read(fd, &mut buf)) {
                    Ok(Ok(0)) | Ok(Err(_)) => eof = true,
                    Ok(Ok(n)) => sess.output(gen, decoder.push(&buf[..n])),
                    Err(_would_block) => {}
                }
            }
        }
    }
    if status.is_none() {
        // The slave side hung up; the exit status follows.
        tokio::select! {
            biased;
            _ = &mut stop => { kill_group(&mut child, pid).await; return; }
            result = child.wait() => status = Some(result),
        }
    }
    // Read what the process wrote before it exited.
    for _ in 0..256 {
        match pty::read(&master, &mut buf) {
            Ok(n) if n > 0 => sess.output(gen, decoder.push(&buf[..n])),
            _ => break,
        }
    }
    sess.output(gen, decoder.finish());
    let status = status.and_then(Result::ok);
    if !scoped {
        pty::signal_group(pid, libc::SIGHUP);
    }
    sess.exited(
        gen,
        status.and_then(|s| s.code()),
        status.and_then(|s| s.signal()),
    );
}

// ---------------------------------------------------------------- manager

pub struct Manager {
    agent_scopes: std::sync::OnceLock<Arc<crate::agent_scope::Registry>>,
    sessions: Mutex<HashMap<(String, String), Arc<Session>>>,
    closed: Mutex<HashSet<(String, String)>>,
    global: Arc<Mutex<Ring>>,
    notify: Notifier,
    lifecycle: tokio::sync::Mutex<()>,
    shell: Mutex<Option<String>>,
    poller: std::sync::atomic::AtomicBool,
    history_dir: Mutex<Option<std::path::PathBuf>>,
}

fn terr(tag: &str, fields: Value) -> Value {
    let mut v = json!({ "_tag": tag });
    if let (Some(o), Some(f)) = (v.as_object_mut(), fields.as_object()) {
        o.extend(f.clone());
    }
    v
}
fn lookup_error(t: &str, id: &str) -> Value {
    terr(
        "TerminalSessionLookupError",
        json!({"threadId":t,"terminalId":id}),
    )
}
fn invalid(message: &str) -> Value {
    json!({"_tag":"RpcInvalidPayload","message":message})
}

fn key_of(p: &Value) -> Result<(String, String), Value> {
    let t = p["threadId"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("threadId required"))?;
    let id = p["terminalId"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| invalid("terminalId must be 1-128 characters"))?;
    Ok((t.into(), id.into()))
}
fn dim(p: &Value, name: &str, max: u64, default: Option<u16>) -> Result<Option<u16>, Value> {
    match p.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=max).contains(n))
            .map(|n| Some(n as u16))
            .ok_or_else(|| invalid(&format!("{name} out of range"))),
    }
}
fn env_of(p: &Value) -> Result<Option<BTreeMap<String, String>>, Value> {
    let Some(o) = p.get("env").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let o = o
        .as_object()
        .filter(|o| o.len() <= 128)
        .ok_or_else(|| invalid("env must be an object of at most 128 entries"))?;
    let mut out = BTreeMap::new();
    for (k, v) in o {
        let ok = !k.is_empty()
            && k.len() <= 128
            && !k.starts_with(|c: char| c.is_ascii_digit())
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let v = v.as_str().filter(|v| v.len() <= 8192);
        match (ok, v) {
            (true, Some(v)) => out.insert(k.clone(), v.to_owned()),
            _ => return Err(invalid("invalid env entry")),
        };
    }
    Ok(if out.is_empty() { None } else { Some(out) })
}
fn opt_path(p: &Value, name: &str) -> Result<Option<Option<String>>, Value> {
    match p.get(name) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Some(Some(s.to_owned())))
            .ok_or_else(|| invalid(&format!("{name} must not be empty"))),
    }
}
fn cwd_of(p: &Value) -> Result<String, Value> {
    p["cwd"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid("cwd required"))
}
fn check_cwd(cwd: &str) -> Result<(), Value> {
    match std::fs::metadata(Path::new(cwd)) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(terr("TerminalCwdNotDirectoryError", json!({"cwd":cwd}))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(terr("TerminalCwdNotFoundError", json!({"cwd":cwd})))
        }
        Err(e) => Err(terr(
            "TerminalCwdStatError",
            json!({"cwd":cwd,"cause":e.to_string()}),
        )),
    }
}

struct Launch {
    scoped: bool,
    thread: String,
    terminal: String,
    cwd: String,
    worktree: Option<Option<String>>,
    cols: Option<u16>,
    rows: Option<u16>,
    env: Option<BTreeMap<String, String>>,
}
fn launch_of(s: &State, p: &Value, cwd_required: bool) -> Result<Launch, Value> {
    let (thread, terminal) = key_of(p)?;
    Ok(Launch {
        scoped: false,
        cwd: if cwd_required || p.get("cwd").is_some() {
            cwd_of(p)?
        } else {
            String::new()
        },
        worktree: opt_path(p, "worktreePath")?,
        cols: dim(p, "cols", 1000, None)?,
        rows: dim(p, "rows", 500, None)?,
        env: provider_env(s, p)?,
        thread,
        terminal,
    })
}

fn provider_env(s: &State, p: &Value) -> Result<Option<BTreeMap<String, String>>, Value> {
    let mut env = BTreeMap::new();
    if let Some(id) = p["providerInstanceId"].as_str() {
        let settings = crate::settings::read(s).map_err(|e| invalid(&e))?;
        for var in settings["providerInstances"][id]["environment"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(value)) = (var["name"].as_str(), var["value"].as_str()) {
                if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                    return Err(invalid("invalid provider environment"));
                }
                env.insert(name.to_owned(), value.to_owned());
            }
        }
    }
    env.extend(env_of(p)?.unwrap_or_default());
    Ok((!env.is_empty()).then_some(env))
}

fn spawn_env(runtime: &Option<BTreeMap<String, String>>) -> Vec<(String, String)> {
    let mut env: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| {
            let u = k.to_ascii_uppercase();
            !(u.starts_with("T3CODE_") || u.starts_with("VITE_") || BLOCKLIST.contains(&u.as_str()))
        })
        .collect();
    if let Some(r) = runtime {
        env.extend(r.clone());
    }
    env.entry("TERM".into())
        .or_insert_with(|| "xterm-256color".into());
    if !env.contains_key("COLORTERM") {
        env.insert("COLORTERM".into(), "truecolor".into());
    }
    let features = env.get("fish_features").cloned();
    if !features
        .as_deref()
        .is_some_and(|f| f.contains("query-term"))
    {
        env.insert(
            "fish_features".into(),
            features.map_or("no-query-term".into(), |f| format!("{f},no-query-term")),
        );
    }
    env.into_iter().collect()
}

impl Manager {
    pub fn new(notify: Notifier) -> Arc<Self> {
        Arc::new(Self {
            agent_scopes: Default::default(),
            sessions: Mutex::default(),
            closed: Mutex::default(),
            global: Arc::default(),
            notify,
            lifecycle: tokio::sync::Mutex::new(()),
            shell: Mutex::new(None),
            poller: std::sync::atomic::AtomicBool::new(false),
            history_dir: Mutex::new(None),
        })
    }
    pub(crate) fn set_agent_scopes(&self, registry: Arc<crate::agent_scope::Registry>) {
        let _ = self.agent_scopes.set(registry);
    }
    /// Restore bounded snapshots only: processes and environment are never resurrected.
    pub fn restore(&self, dir: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        *self.history_dir.lock().unwrap() = Some(dir.to_owned());
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().extension().is_none_or(|x| x != "json")
                || entry.metadata().map_err(|e| e.to_string())?.len()
                    > 6 * HISTORY_BYTES as u64 + 65536
            {
                continue;
            }
            let Ok(v) = std::fs::read(entry.path())
                .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(std::io::Error::other))
            else {
                continue;
            };
            let Ok((thread, terminal)) = key_of(&v) else {
                continue;
            };
            let Some(cwd) = v["cwd"].as_str() else {
                continue;
            };
            let sess = Session::new(
                &thread,
                &terminal,
                cwd,
                v["worktreePath"].as_str().map(str::to_owned),
                None,
                v["cols"]
                    .as_u64()
                    .unwrap_or(DEFAULT_COLS as u64)
                    .clamp(1, 1000) as u16,
                v["rows"]
                    .as_u64()
                    .unwrap_or(DEFAULT_ROWS as u64)
                    .clamp(1, 500) as u16,
                self.notify.clone(),
                self.global.clone(),
            );
            {
                let mut st = sess.st.lock().unwrap();
                st.status = "exited";
                st.history.append(v["history"].as_str().unwrap_or(""));
                st.seq = v["sequence"].as_u64().unwrap_or(0);
                st.updated_at = v["updatedAt"].as_str().unwrap_or("").to_owned();
            }
            *sess.persistence.lock().unwrap() = Some(entry.path());
            self.sessions
                .lock()
                .unwrap()
                .insert((thread, terminal), sess);
            self.evict_inactive();
        }
        self.evict_inactive();
        Ok(())
    }

    fn history_path(&self, thread: &str, terminal: &str) -> Option<std::path::PathBuf> {
        use sha2::{Digest, Sha256};
        let key = serde_json::to_vec(&(thread, terminal)).unwrap();
        self.history_dir
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.join(format!("{:x}.json", Sha256::digest(key))))
    }

    /// Overrides the shell program (otherwise `$SHELL`, then `bash`).
    pub fn set_shell(&self, shell: &str) {
        *self.shell.lock().unwrap() = Some(shell.into());
    }
    fn get(&self, key: &(String, String)) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(key).cloned()
    }
    fn require(&self, key: &(String, String)) -> Result<Arc<Session>, Value> {
        self.get(key).ok_or_else(|| lookup_error(&key.0, &key.1))
    }
    pub fn live_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.st.lock().unwrap().status == "running")
            .count()
    }
    pub fn summaries(&self) -> Vec<Value> {
        let mut all: Vec<Value> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.summary())
            .collect();
        all.sort_by(|a, b| {
            b["updatedAt"]
                .as_str()
                .cmp(&a["updatedAt"].as_str())
                .then(a["threadId"].as_str().cmp(&b["threadId"].as_str()))
                .then(a["terminalId"].as_str().cmp(&b["terminalId"].as_str()))
        });
        all
    }
    fn global_head(&self) -> u64 {
        self.global.lock().unwrap().head
    }

    fn evict_inactive(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        let mut inactive: Vec<_> = sessions
            .iter()
            .filter_map(|(k, s)| {
                let st = s.st.lock().unwrap();
                (st.status != "running" && st.status != "starting")
                    .then(|| (st.updated_at.clone(), k.clone()))
            })
            .collect();
        if inactive.len() <= MAX_INACTIVE {
            return;
        }
        inactive.sort();
        let mut closed = self.closed.lock().unwrap();
        for (_, k) in inactive.drain(..inactive.len() - MAX_INACTIVE) {
            if let Some(s) = sessions.remove(&k) {
                s.st.lock().unwrap().closed = true;
                if let Some(path) = s.persistence.lock().unwrap().as_ref() {
                    let _ = std::fs::remove_file(path);
                }
            }
            closed.insert(k);
        }
    }

    async fn start_session(
        self: &Arc<Self>,
        sess: &Arc<Session>,
        cols: u16,
        rows: u16,
        restarted: bool,
    ) {
        if let Some(registry) = self.agent_scopes.get() {
            registry.initialize().await;
        }
        let mut st = sess.st.lock().unwrap();
        Session::stop_process(&mut st);
        st.status = "starting";
        st.cols = cols;
        st.rows = rows;
        st.exit_code = None;
        st.exit_signal = None;
        st.has_subprocess = false;
        st.child_label = None;
        st.updated_at = iso(now_ms());
        let requested = self
            .shell
            .lock()
            .unwrap()
            .clone()
            .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "bash".into());
        let env = spawn_env(&st.env);
        let mut candidates = vec![requested.clone()];
        if requested != "/bin/sh" {
            candidates.push("/bin/sh".into());
        }
        let mut last_error = String::new();
        let mut spawned = None;
        for shell in &candidates {
            let launch = self.agent_scopes.get().map_or_else(
                || Ok(crate::agent_scope::TerminalCommand::plain(shell, &env)),
                |registry| registry.terminal_command(shell, Path::new(&st.cwd), &env),
            );
            let launch = launch.map_err(std::io::Error::other).and_then(|launch| {
                pty::spawn_prepared(&launch, Path::new(&st.cwd), cols, rows, st.scoped)
            });
            match launch {
                Ok(p) => {
                    st.shell_program = shell.clone();
                    spawned = Some(p);
                    break;
                }
                Err(e) => last_error = format!("failed to start {shell}: {e}"),
            }
        }
        match spawned {
            Some(p) => {
                st.gen += 1;
                let gen = st.gen;
                let master = Arc::new(p.master);
                let (stop_tx, stop_rx) = oneshot::channel();
                st.proc = Some(Proc {
                    completion: p.child.completion(),
                    master: master.clone(),
                    _stop: stop_tx,
                });
                st.pid = Some(p.pid);
                st.status = "running";
                let seq = st.advance();
                let event = json!({"type":if restarted {"restarted"} else {"started"},"snapshot":sess.snapshot_of(&st)});
                sess.publish(&mut st, event, seq);
                tokio::spawn(pump(sess.clone(), gen, master, p.child, p.pid, stop_rx));
                drop(st);
                self.ensure_poller();
            }
            None => {
                st.status = "error";
                st.pid = None;
                let seq = st.advance();
                sess.publish(&mut st, json!({"type":"error","message":last_error}), seq);
            }
        }
    }

    fn new_session(&self, l: &Launch, cols: u16, rows: u16) -> Arc<Session> {
        let sess = Session::new(
            &l.thread,
            &l.terminal,
            &l.cwd,
            l.worktree.clone().flatten(),
            l.env.clone(),
            cols,
            rows,
            self.notify.clone(),
            self.global.clone(),
        );
        sess.st.lock().unwrap().scoped = l.scoped;
        *sess.persistence.lock().unwrap() = self.history_path(&l.thread, &l.terminal);
        sess
    }

    /// `terminal.open`: start, or reconcile with, the session named by the input.
    async fn open_locked(self: &Arc<Self>, l: &Launch) -> Result<Arc<Session>, Value> {
        check_cwd(&l.cwd)?;
        let key = (l.thread.clone(), l.terminal.clone());
        let Some(live) = self.get(&key) else {
            if self.sessions.lock().unwrap().len() >= MAX_LIVE {
                return Err(invalid("too many open terminals"));
            }
            let (cols, rows) = (
                l.cols.unwrap_or(DEFAULT_COLS),
                l.rows.unwrap_or(DEFAULT_ROWS),
            );
            let sess = self.new_session(l, cols, rows);
            self.sessions
                .lock()
                .unwrap()
                .insert(key.clone(), sess.clone());
            self.closed.lock().unwrap().remove(&key);
            self.evict_inactive();
            self.start_session(&sess, cols, rows, false).await;
            return Ok(sess);
        };
        let (cols, rows, restart) = {
            let mut st = live.st.lock().unwrap();
            let target = (l.cols.unwrap_or(st.cols), l.rows.unwrap_or(st.rows));
            let next_worktree = l
                .worktree
                .clone()
                .unwrap_or_else(|| st.worktree_path.clone());
            let changed = st.cwd != l.cwd
                || (l.scoped && !st.scoped)
                || (st.proc.is_some() && st.env != l.env)
                || st.worktree_path != next_worktree;
            if changed {
                Session::stop_process(&mut st);
                st.cwd = l.cwd.clone();
                st.worktree_path = next_worktree;
                st.env = l.env.clone();
                st.scoped |= l.scoped;
                st.history.clear();
                st.pending_ctl.clear();
            } else if matches!(st.status, "exited" | "error") {
                st.env = l.env.clone();
                st.worktree_path = next_worktree;
            }
            if st.proc.is_none() {
                (target.0, target.1, true)
            } else {
                if (st.cols, st.rows) != target {
                    if let Some(p) = &st.proc {
                        let _ = pty::resize(&p.master, target.0, target.1);
                    }
                    st.cols = target.0;
                    st.rows = target.1;
                    st.updated_at = iso(now_ms());
                }
                (target.0, target.1, false)
            }
        };
        if restart {
            self.start_session(&live, cols, rows, false).await;
        }
        Ok(live)
    }

    /// `terminal.attach`: the session to stream, `None` when it was closed and the client only resumes.
    async fn attach_locked(
        self: &Arc<Self>,
        s: &State,
        p: &Value,
        resume: bool,
    ) -> Result<Option<Arc<Session>>, Value> {
        let l = launch_of(s, p, false)?;
        if crate::events::get(&s.db.lock().unwrap(), "thread", &l.thread)
            .is_ok_and(|t| !t["deletedAt"].is_null())
        {
            return if resume {
                Ok(None)
            } else {
                Err(invalid("The thread was deleted."))
            };
        }
        let key = (l.thread.clone(), l.terminal.clone());
        let restart_if_stopped = p["restartIfNotRunning"] == true;
        let Some(session) = self.get(&key) else {
            if resume && self.closed.lock().unwrap().contains(&key) {
                return Ok(None);
            }
            if l.cwd.is_empty() {
                return Err(lookup_error(&key.0, &key.1));
            }
            return self.open_locked(&l).await.map(Some);
        };
        let stopped = session.st.lock().unwrap().proc.is_none();
        if !resume && stopped && !l.cwd.is_empty() && restart_if_stopped {
            return self.open_locked(&l).await.map(Some);
        }
        let mut st = session.st.lock().unwrap();
        let target = (l.cols.unwrap_or(st.cols), l.rows.unwrap_or(st.rows));
        if st.status == "running" && (st.cols, st.rows) != target {
            if let Some(p) = &st.proc {
                let _ = pty::resize(&p.master, target.0, target.1);
            }
            st.cols = target.0;
            st.rows = target.1;
            st.updated_at = iso(now_ms());
        }
        drop(st);
        Ok(Some(session))
    }

    pub(crate) async fn cleanup(&self, thread: &str) -> Result<(), String> {
        let _gate = self.lifecycle.lock().await;
        let keys: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .keys()
            .filter(|(t, _)| t == thread)
            .cloned()
            .collect();
        for key in keys {
            if let Some(session) = self.get(&key) {
                session.persistence.lock().unwrap().take();
            }
            await_scope(self.close_session(&key)).await?;
        }
        let dir = self.history_dir.lock().unwrap().clone();
        if let Some(dir) = dir {
            for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                    || entry.path().extension().is_none_or(|e| e != "json")
                    || entry.metadata().map_err(|e| e.to_string())?.len()
                        > 6 * HISTORY_BYTES as u64 + 65536
                {
                    continue;
                }
                let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
                if serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .is_some_and(|v| v["threadId"] == thread)
                {
                    std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
    fn close_session(&self, key: &(String, String)) -> Option<tokio::sync::watch::Receiver<bool>> {
        let removed = self.sessions.lock().unwrap().remove(key);
        self.closed.lock().unwrap().insert(key.clone());
        if self.closed.lock().unwrap().len() > 4096 {
            self.closed.lock().unwrap().clear();
        }
        if let Some(sess) = removed {
            let mut st = sess.st.lock().unwrap();
            let completion = st.proc.as_ref().and_then(|p| p.completion.clone());
            Session::stop_process(&mut st);
            st.closed = true;
            let seq = st.advance();
            sess.publish(&mut st, json!({"type":"closed"}), seq);
            st.history.clear();
            st.pending_ctl.clear();
            return completion;
        }
        None
    }

    fn ensure_poller(self: &Arc<Self>) {
        if self.poller.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if !this.poll_activity() {
                    this.poller
                        .store(false, std::sync::atomic::Ordering::Release);
                    // A terminal may have started between the last poll and clearing the flag.
                    if this.live_count() == 0
                        || this.poller.swap(true, std::sync::atomic::Ordering::AcqRel)
                    {
                        return;
                    }
                }
            }
        });
    }

    /// Notes whether each shell has a child process, as T3's activity events do. `false` when idle.
    fn poll_activity(&self) -> bool {
        let sessions: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().cloned().collect();
        let mut any = false;
        for sess in sessions {
            let mut st = sess.st.lock().unwrap();
            let Some(pid) = st.pid.filter(|_| st.status == "running") else {
                continue;
            };
            any = true;
            let (busy, label) = shell_child(pid);
            if busy != st.has_subprocess || (busy && label != st.child_label) {
                st.has_subprocess = busy;
                st.child_label = label;
                let event = json!({"type":"activity","hasRunningSubprocess":busy,"label":st.label(&sess.terminal_id)});
                let seq = st.advance();
                sess.publish(&mut st, event, seq);
            }
        }
        any
    }
}

/// The first child of `pid` and its command name, read from /proc.
fn shell_child(pid: i32) -> (bool, Option<String>) {
    let Ok(children) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) else {
        return (false, None);
    };
    match children.split_whitespace().next() {
        None => (false, None),
        Some(child) => {
            let name = std::fs::read_to_string(format!("/proc/{child}/comm"))
                .ok()
                .map(|s| s.trim().to_owned());
            (true, name)
        }
    }
}

// ---------------------------------------------------------------- unary RPCs

pub async fn call(s: &Arc<State>, tag: &str, p: &Value) -> Result<Value, Value> {
    let m = s.streams.terminals.clone();
    match tag {
        "terminal.open" => {
            let l = launch_of(s, p, true)?;
            let _guard = m.lifecycle.lock().await;
            if crate::events::get(&s.db.lock().unwrap(), "thread", &l.thread)
                .is_ok_and(|t| !t["deletedAt"].is_null())
            {
                return Err(invalid("The thread was deleted."));
            }
            let sess = m.open_locked(&l).await?;
            Ok(sess.snapshot())
        }
        "terminal.write" => {
            let key = key_of(p)?;
            let data = p["data"]
                .as_str()
                .filter(|d| !d.is_empty() && d.len() <= 65_536)
                .ok_or_else(|| invalid("data must be 1-65536 bytes"))?;
            let sess = m.require(&key)?;
            let (master, pid) = {
                let mut st = sess.st.lock().unwrap();
                let master = st.proc.as_ref().map(|p| p.master.clone());
                match (master, st.status) {
                    (Some(master), "running") => {
                        st.input_count += 1;
                        (master, st.pid.unwrap_or(0))
                    }
                    (_, "exited") => return Ok(Value::Null),
                    _ => {
                        return Err(terr(
                            "TerminalNotRunningError",
                            json!({"threadId":key.0,"terminalId":key.1}),
                        ))
                    }
                }
            };
            pty::write_all(&master, data.as_bytes(), Duration::from_secs(2))
                .await
                .map(|_| Value::Null)
                .map_err(|e| terr("TerminalWriteError", json!({"threadId":key.0,"terminalId":key.1,"terminalPid":pid,"cause":e.to_string()})))
        }
        "terminal.resize" => {
            let key = key_of(p)?;
            let cols = dim(p, "cols", 1000, None)?.ok_or_else(|| invalid("cols required"))?;
            let rows = dim(p, "rows", 500, None)?.ok_or_else(|| invalid("rows required"))?;
            let _guard = m.lifecycle.lock().await;
            let Some(sess) = m.get(&key) else {
                return Ok(Value::Null);
            };
            let mut st = sess.st.lock().unwrap();
            let Some(proc) = st.proc.as_ref().filter(|_| st.status == "running") else {
                return Ok(Value::Null);
            };
            pty::resize(&proc.master, cols, rows).map_err(|e| terr("TerminalResizeError", json!({"threadId":key.0,"terminalId":key.1,"terminalPid":st.pid.unwrap_or(0),"cols":cols,"rows":rows,"cause":e.to_string()})))?;
            st.cols = cols;
            st.rows = rows;
            st.updated_at = iso(now_ms());
            Ok(Value::Null)
        }
        "terminal.clear" => {
            let key = key_of(p)?;
            let _guard = m.lifecycle.lock().await;
            let sess = m.require(&key)?;
            let mut st = sess.st.lock().unwrap();
            st.history.clear();
            st.pending_ctl.clear();
            let seq = st.advance();
            sess.publish(&mut st, json!({"type":"cleared"}), seq);
            Ok(Value::Null)
        }
        "terminal.restart" => {
            let l = launch_of(s, p, true)?;
            if l.cols.is_none() || l.rows.is_none() {
                return Err(invalid("cols and rows required"));
            }
            let _guard = m.lifecycle.lock().await;
            if crate::events::get(&s.db.lock().unwrap(), "thread", &l.thread)
                .is_ok_and(|t| !t["deletedAt"].is_null())
            {
                return Err(invalid("The thread was deleted."));
            }
            check_cwd(&l.cwd)?;
            let key = (l.thread.clone(), l.terminal.clone());
            let sess = match m.get(&key) {
                Some(sess) => {
                    let mut st = sess.st.lock().unwrap();
                    Session::stop_process(&mut st);
                    st.cwd = l.cwd.clone();
                    st.worktree_path = l.worktree.clone().flatten();
                    st.env = l.env.clone();
                    drop(st);
                    sess
                }
                None => {
                    let sess = m.new_session(&l, l.cols.unwrap(), l.rows.unwrap());
                    m.sessions.lock().unwrap().insert(key.clone(), sess.clone());
                    m.closed.lock().unwrap().remove(&key);
                    m.evict_inactive();
                    sess
                }
            };
            {
                let mut st = sess.st.lock().unwrap();
                st.history.clear();
                st.pending_ctl.clear();
            }
            m.start_session(&sess, l.cols.unwrap(), l.rows.unwrap(), true)
                .await;
            Ok(sess.snapshot())
        }
        "terminal.close" => {
            let thread = p["threadId"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("threadId required"))?
                .to_owned();
            let _guard = m.lifecycle.lock().await;
            match p.get("terminalId").and_then(Value::as_str) {
                Some(id) => await_scope(m.close_session(&(thread, id.to_owned())))
                    .await
                    .map_err(|e| invalid(&e))?,
                None => {
                    let keys: Vec<_> = m
                        .sessions
                        .lock()
                        .unwrap()
                        .keys()
                        .filter(|k| k.0 == thread)
                        .cloned()
                        .collect();
                    for key in keys {
                        await_scope(m.close_session(&key))
                            .await
                            .map_err(|e| invalid(&e))?;
                    }
                }
            }
            Ok(Value::Null)
        }
        _ => Err(invalid("unknown terminal method")),
    }
}

// ---------------------------------------------------------------- streams

/// Start a project script in a visible terminal, after its interactive shell is ready.
/// A generation check prevents delayed input reaching a restarted/rebound terminal.
pub(crate) async fn project_script(
    s: &Arc<State>,
    p: &Value,
    command: String,
) -> Result<(), Value> {
    begin_project_script(s, p, command, false).await.map(|_| ())
}
pub(crate) async fn observed_project_script(
    s: &Arc<State>,
    p: &Value,
    command: String,
) -> Result<ScriptObserver, Value> {
    begin_project_script(s, p, command, true).await
}
async fn begin_project_script(
    s: &Arc<State>,
    p: &Value,
    mut command: String,
    observe: bool,
) -> Result<ScriptObserver, Value> {
    let mut changed = s.streams.terminals.notify.subscribe();
    let mut launch = launch_of(s, p, true)?;
    // Project setup owns its descendants even when they daemonize. Ordinary
    // interactive terminals retain their existing lifecycle.
    launch.scoped = cfg!(target_os = "linux");
    let (session, readiness_sequence) = {
        let manager = &s.streams.terminals;
        let _guard = manager.lifecycle.lock().await;
        if crate::events::get(&s.db.lock().unwrap(), "thread", &launch.thread)
            .is_ok_and(|t| !t["deletedAt"].is_null())
        {
            return Err(invalid("The thread was deleted."));
        }
        // Restored history and a previous shell's line editor state must not
        // make this shell ready. Capture the live event cursor before opening.
        let sequence = manager
            .get(&(launch.thread.clone(), launch.terminal.clone()))
            .map_or(0, |session| session.st.lock().unwrap().seq);
        (manager.open_locked(&launch).await?, sequence)
    };
    let generation = session.st.lock().unwrap().gen;
    let token = if observe {
        format!(
            "__T3_SETUP_DONE___{}:",
            crate::store::random_id().map_err(|e| invalid(&e))?
        )
    } else {
        String::new()
    };
    if observe {
        let program = session.st.lock().unwrap().shell_program.clone();
        let name = Path::new(&program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let body = command.replace("\r\n", "\n").replace('\n', "\r");
        command=match name {
            "fish"=>format!("begin\r{body}\rend; printf '\\n{token}%s\\n' $status"),
            "pwsh"|"powershell"=>format!("$global:LASTEXITCODE = $null; & {{\r{body}\r}}; if ($null -ne $LASTEXITCODE) {{ $__t3c = $LASTEXITCODE }} elseif ($?) {{ $__t3c = 0 }} else {{ $__t3c = 1 }}; Write-Host \"{token}$__t3c\""),
            _=>format!("( {body}\r); printf '\\n{token}%s\\n' \"$?\""),
        };
    }
    let input_error = Arc::new(Mutex::new(None));
    let observer = ScriptObserver {
        session: session.clone(),
        generation,
        sequence: session.st.lock().unwrap().seq,
        partial: String::new(),
        token,
        input_error: input_error.clone(),
    };
    let weak = Arc::downgrade(s);
    let cwd = p["cwd"].clone();
    let tracking = p["_setupTrackingId"].as_str().map(str::to_owned);
    tokio::spawn(async move {
        let mut sequence = readiness_sequence;
        let mut carry = String::new();
        let mut paste = false;
        let mut quiet = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut prompted = false;
        loop {
            if weak.strong_count() == 0 {
                return;
            }
            let busy = {
                let st = session.st.lock().unwrap();
                if st.gen != generation || st.status != "running" {
                    return;
                }
                for item in st.log.iter().filter(|item| item.seq > sequence) {
                    if let Some(data) = item.event["data"]
                        .as_str()
                        .filter(|_| item.event["type"] == "output")
                    {
                        quiet = tokio::time::Instant::now() + Duration::from_secs(3);
                        let text = format!("{carry}{data}");
                        if let Some(on) = text.rfind("\x1b[?2004h") {
                            paste = text.rfind("\x1b[?2004l").is_none_or(|off| on > off);
                        } else if text.contains("\x1b[?2004l") {
                            paste = false;
                        }
                        carry = text
                            .chars()
                            .rev()
                            .take(16)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                }
                sequence = st.seq;
                st.has_subprocess
            };
            if paste {
                break;
            }
            if tokio::time::Instant::now() >= quiet {
                if !busy {
                    break;
                }
                if !prompted {
                    if let (Some(s), Some(id)) = (weak.upgrade(), tracking.as_deref()) {
                        let _ = crate::worktree_setup::stage(
                            &s,
                            id,
                            "setup-script",
                            "running",
                            Some("Waiting for shell startup prompt"),
                        );
                    }
                    prompted = true;
                }
                quiet = tokio::time::Instant::now() + Duration::from_secs(3);
            }
            tokio::select! {r=changed.changed()=>if r.is_err(){return;},_=tokio::time::sleep_until(quiet)=>{}}
        }
        let Some(s) = weak.upgrade() else {
            return;
        };
        if !crate::events::get(&s.db.lock().unwrap(), "thread", &session.thread_id)
            .is_ok_and(|t| t["deletedAt"].is_null() && t["worktreePath"] == cwd)
        {
            return;
        }
        let master = {
            let mut st = session.st.lock().unwrap();
            if st.gen != generation || st.status != "running" {
                return;
            }
            st.input_count += 1;
            st.proc.as_ref().map(|p| p.master.clone())
        };
        if let Some(master) = master {
            if let Err(error) = pty::write_all(
                &master,
                format!(" {command}\r").as_bytes(),
                Duration::from_secs(2),
            )
            .await
            {
                eprintln!("Project script input failed: {error}");
                *input_error.lock().unwrap() =
                    Some(format!("Project script input failed: {error}"));
                bump(&session.notify);
            }
        } else {
            *input_error.lock().unwrap() = Some("Setup terminal is unavailable".into());
            bump(&session.notify);
        }
    });
    Ok(observer)
}

pub(crate) struct ScriptObserver {
    session: Arc<Session>,
    generation: u64,
    sequence: u64,
    partial: String,
    token: String,
    input_error: Arc<Mutex<Option<String>>>,
}
impl ScriptObserver {
    pub(crate) async fn wait(
        &mut self,
        mut line: impl FnMut(String),
    ) -> Result<Option<i32>, String> {
        let mut changes = self.session.notify.subscribe();
        static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let ansi = ANSI.get_or_init(|| {
            regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap()
        });
        loop {
            if let Some(error) = self.input_error.lock().unwrap().clone() {
                return Err(error);
            }
            let (chunks, ended) = {
                let st = self.session.st.lock().unwrap();
                let chunks = st
                    .log
                    .iter()
                    .filter(|e| e.seq > self.sequence)
                    .filter_map(|e| {
                        e.event["data"]
                            .as_str()
                            .filter(|_| e.event["type"] == "output")
                            .map(str::to_owned)
                    })
                    .collect::<Vec<_>>();
                self.sequence = st.seq;
                (chunks, st.gen != self.generation || st.status != "running")
            };
            for chunk in chunks {
                self.partial.push_str(&chunk);
                while let Some(end) = self.partial.find('\n') {
                    let text = self.partial[..end].to_owned();
                    self.partial.drain(..=end);
                    let clean = ansi.replace_all(&text, "");
                    let clean = clean
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t')
                        .collect::<String>();
                    if let Some(value) = clean
                        .trim()
                        .strip_prefix(&self.token)
                        .filter(|_| !self.token.is_empty())
                    {
                        if let Ok(code) = value.trim().parse::<i32>() {
                            return Ok(Some(code));
                        }
                    }
                    if !clean.contains(&self.token) && !clean.is_empty() {
                        line(clean);
                    }
                }
                if self.partial.len() > 16384 {
                    let mut cut = self.partial.len() - 16384;
                    while !self.partial.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.partial.drain(..cut);
                }
            }
            if ended {
                return Ok(None);
            }
            changes
                .changed()
                .await
                .map_err(|_| "Setup terminal notifications ended".to_owned())?;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Attach,
    Observe,
    Events,
    Metadata,
}

pub struct Cursor {
    kind: Kind,
    key: (String, String),
    session: Option<Arc<Session>>,
    last: u64,
    initial: Option<Value>,
    window: VecDeque<usize>,
}

pub enum Step {
    Values(Vec<Value>),
    Idle,
    Done,
}

impl Cursor {
    pub fn is_full(&self) -> bool {
        self.window.len() >= WINDOW_CHUNKS || self.window.iter().sum::<usize>() >= BATCH_BYTES
    }
    pub fn ack(&mut self) {
        self.window.pop_front();
    }
    fn record(&mut self, values: &[Value]) {
        self.window
            .push_back(values.iter().map(|v| v.to_string().len()).sum());
    }
    pub fn next(&mut self, s: &State) -> Step {
        if self.is_full() {
            return Step::Idle;
        }
        let step = match self.kind {
            Kind::Attach | Kind::Observe => self.next_session(s),
            Kind::Events | Kind::Metadata => self.next_global(s),
        };
        if let Step::Values(v) = &step {
            self.record(v);
        }
        step
    }

    fn next_session(&mut self, s: &State) -> Step {
        if let Some(v) = self.initial.take() {
            return Step::Values(vec![v]);
        }
        let Some(sess) = self.session.clone() else {
            return Step::Idle;
        };
        let st = sess.st.lock().unwrap();
        let first = st.log.front().map(|l| l.seq);
        if first.is_some_and(|f| f > self.last + 1) {
            // Fell behind the retained log: replace what the subscriber knows with a fresh snapshot.
            self.last = st.seq;
            let snap = sess.snapshot_of(&st);
            return Step::Values(vec![json!({"type":"snapshot","snapshot":snap})]);
        }
        let mut out = Vec::new();
        let mut bytes = 0;
        let after = self.last;
        for l in st.log.iter().filter(|l| l.seq > after) {
            if !out.is_empty() && bytes + l.size > BATCH_BYTES {
                break;
            }
            bytes += l.size;
            self.last = l.seq;
            out.push(if l.event["type"] == "started" {
                json!({"type":"snapshot","snapshot":l.event["snapshot"]})
            } else {
                (*l.event).clone()
            });
        }
        if !out.is_empty() {
            return Step::Values(out);
        }
        let closed = st.closed;
        drop(st);
        if closed {
            // The terminal was closed; follow a new session that reuses its identifiers.
            if let Some(next) = s
                .streams
                .terminals
                .get(&self.key)
                .filter(|n| !Arc::ptr_eq(n, &sess))
            {
                let snap = next.snapshot();
                self.last = snap["sequence"].as_u64().unwrap_or(0);
                self.session = Some(next);
                return Step::Values(vec![json!({"type":"snapshot","snapshot":snap})]);
            }
        }
        Step::Idle
    }

    fn next_global(&mut self, s: &State) -> Step {
        let m = &s.streams.terminals;
        if let Some(v) = self.initial.take() {
            return Step::Values(vec![v]);
        }
        let ring = m.global.lock().unwrap();
        if ring
            .items
            .front()
            .is_some_and(|(g, _, _)| *g > self.last + 1)
        {
            if self.kind == Kind::Events {
                return Step::Done;
            }
            drop(ring);
            let head = m.global_head();
            self.last = head;
            return Step::Values(vec![json!({"type":"snapshot","terminals":m.summaries()})]);
        }
        let mut events = Vec::new();
        let mut bytes = 0;
        let after = self.last;
        for (g, size, e) in ring.items.iter().filter(|(g, _, _)| *g > after) {
            if !events.is_empty() && bytes + size > BATCH_BYTES {
                break;
            }
            bytes += size;
            self.last = *g;
            events.push(e.clone());
        }
        drop(ring);
        if self.kind == Kind::Events {
            return if events.is_empty() {
                Step::Idle
            } else {
                Step::Values(events.iter().map(|e| (**e).clone()).collect())
            };
        }
        let mut out = Vec::new();
        for e in events {
            let ty = e["type"].as_str().unwrap_or("");
            if !matches!(
                ty,
                "started" | "restarted" | "exited" | "closed" | "error" | "activity"
            ) {
                continue;
            }
            if ty == "closed" {
                out.push(
                    json!({"type":"remove","threadId":e["threadId"],"terminalId":e["terminalId"]}),
                );
            } else if let (Some(t), Some(i)) = (e["threadId"].as_str(), e["terminalId"].as_str()) {
                if let Some(sess) = m.get(&(t.to_owned(), i.to_owned())) {
                    out.push(json!({"type":"upsert","terminal":sess.summary()}));
                }
            }
        }
        if out.is_empty() {
            Step::Idle
        } else {
            Step::Values(out)
        }
    }
}

/// Opens `terminal.attach`, `terminal.observe`, `subscribeTerminalEvents` or `subscribeTerminalMetadata`.
pub async fn open_stream(s: &Arc<State>, tag: &str, p: &Value) -> Result<Cursor, Value> {
    let m = s.streams.terminals.clone();
    let kind = match tag {
        "terminal.attach" => Kind::Attach,
        "terminal.observe" => Kind::Observe,
        "subscribeTerminalEvents" => Kind::Events,
        _ => Kind::Metadata,
    };
    let mut cursor = Cursor {
        kind,
        key: (String::new(), String::new()),
        session: None,
        last: 0,
        initial: None,
        window: VecDeque::new(),
    };
    match kind {
        Kind::Events => cursor.last = m.global_head(),
        Kind::Metadata => {
            cursor.last = m.global_head();
            cursor.initial = Some(json!({"type":"snapshot","terminals":m.summaries()}));
        }
        Kind::Attach | Kind::Observe => {
            let key = key_of(p)?;
            cursor.key = key.clone();
            let _guard = m.lifecycle.lock().await;
            let session = if kind == Kind::Attach {
                m.attach_locked(s, p, p["resume"] == true).await?
            } else {
                Some(m.require(&key)?)
            };
            cursor.initial = Some(match &session {
                Some(sess) => {
                    let snap = sess.snapshot();
                    cursor.last = snap["sequence"].as_u64().unwrap_or(0);
                    json!({"type":"snapshot","snapshot":snap})
                }
                None => json!({"type":"closed","threadId":key.0,"terminalId":key.1}),
            });
            cursor.session = session;
        }
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_reads_is_decoded_once() {
        let bytes = "añ€😀".as_bytes();
        for cut in 1..bytes.len() {
            let mut d = Utf8Stream::default();
            let out = d.push(&bytes[..cut]) + &d.push(&bytes[cut..]) + &d.finish();
            assert_eq!(out, "añ€😀", "cut at {cut}");
        }
        let mut d = Utf8Stream::default();
        assert_eq!(d.push(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(d.push(&[0xe2, 0x82]), "");
        assert_eq!(d.finish(), "\u{FFFD}");
    }

    #[test]
    fn history_is_bounded_by_bytes_and_lines() {
        let mut h = History::new(100, 1000);
        for i in 0..50 {
            h.append(&format!("line {i}\n"));
        }
        assert!(h.len() <= 100);
        assert!(h.value().ends_with("line 49\n"));
        let mut h = History::new(1 << 20, 3);
        h.append("a\nb\nc\nd\ne");
        assert_eq!(h.value(), "c\nd\ne");
        h.append("é".repeat(10).as_str());
        let mut h = History::new(7, 100);
        h.append("ééééé");
        assert!(h.len() <= 7 && h.value().chars().all(|c| c == 'é'));
    }

    #[test]
    fn history_drops_terminal_queries_but_keeps_styling() {
        let (v, p) = sanitize_history("", "a\u{1b}[6nb\u{1b}[31mred\u{1b}[0m\u{1b}]11;?\u{7}c");
        assert_eq!((v.as_str(), p.as_str()), ("ab\u{1b}[31mred\u{1b}[0mc", ""));
        let (v, p) = sanitize_history("", "x\u{1b}[");
        assert_eq!((v.as_str(), p.as_str()), ("x", "\u{1b}["));
        let (v, p) = sanitize_history(&p, "6ny");
        assert_eq!((v.as_str(), p.as_str()), ("y", ""));
    }

    #[test]
    fn labels_follow_t3_numbering() {
        assert_eq!(terminal_label("term-2"), "Terminal 2");
        assert_eq!(terminal_label("terminal-10"), "Terminal 10");
        assert_eq!(terminal_label("build"), "build");
        assert_eq!(
            terminal_label("term-3-0123abcd-0123-0123-0123-0123456789ab"),
            "Terminal 3"
        );
    }
}
