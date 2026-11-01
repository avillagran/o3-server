//! Transient command output (`orchestration.subscribeCommandOutput`), kept apart from the transcript.
//!
//! A provider reports a command with `CommandEvent`s. Output chunks only update a bounded tail held
//! here; the turn item is persisted when the command starts and again when it finishes. Subscribers
//! hold an absolute offset into the tail, so a slow viewer is sent a `replace` of the tail instead of
//! a queue of appends. Progress folding handles CR, backspace and bounded CSI sequences;
//! arbitrary multi-line terminal emulation is deliberately outside this surface.
use crate::{
    events::{append, get, put},
    store::{iso, now_ms, State},
    terminal::{bump, Notifier},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

const TAIL_BYTES: usize = 64 * 1024;
const MAX_APPEND_BYTES: u64 = 16 * 1024;
const MAX_LIVE: usize = 64;
const MAX_FINISHED: usize = 16;
const MAX_FINAL_BYTES: usize = 4 * TAIL_BYTES;

type Key = (String, String);

pub enum CommandEvent {
    Started {
        item_id: String,
        input: String,
    },
    Output {
        item_id: String,
        chunk: String,
    },
    Finished {
        item_id: String,
        exit_code: Option<i32>,
        output: Option<String>,
    },
}

#[derive(Clone, Default)]
struct Tail {
    text: String,
    start: u64,
    total: u64,
    truncated: bool,
    cursor: usize,
    escape: String,
    rewritten: u64,
}
impl Tail {
    fn push(&mut self, chunk: &str) {
        let plain = self.escape.is_empty()
            && !chunk.chars().any(|c| c.is_control() && c != '\n')
            && self.cursor == self.text.rsplit('\n').next().unwrap_or("").chars().count();
        if plain {
            self.text.push_str(chunk);
            self.cursor = self.text.rsplit('\n').next().unwrap_or("").chars().count();
            self.total += chunk.len() as u64;
        } else {
            self.rewritten += 1;
            let start = self.text.rfind('\n').map_or(0, |n| n + 1);
            let mut line: Vec<char> = self.text[start..].chars().collect();
            self.text.truncate(start);
            for c in chunk.chars() {
                if !self.escape.is_empty() {
                    self.escape.push(c);
                    if self.escape.len() > 64 {
                        self.escape.clear();
                        continue;
                    }
                    if self.escape.starts_with("\x1b[")
                        && ('@'..='~').contains(&c)
                        && self.escape.len() > 2
                    {
                        let args = &self.escape[2..self.escape.len() - 1];
                        let n = args.parse::<usize>().unwrap_or(1).clamp(1, TAIL_BYTES);
                        match c {
                            'G' => self.cursor = n - 1,
                            'C' => self.cursor = self.cursor.saturating_add(n).min(TAIL_BYTES),
                            'D' => self.cursor = self.cursor.saturating_sub(n),
                            'K' if args == "2" => line.clear(),
                            'K' if args.is_empty() || args == "0" => line.truncate(self.cursor),
                            'K' if args == "1" => {
                                for ch in line.iter_mut().take(self.cursor + 1) {
                                    *ch = ' ';
                                }
                            }
                            _ => {}
                        }
                        self.escape.clear();
                    } else if self.escape.starts_with("\x1b]") {
                        if c == '\x07' || self.escape.ends_with("\x1b\\") {
                            self.escape.clear();
                        }
                    } else if self.escape.len() == 2 && c != '[' && c != ']' {
                        self.escape.clear();
                    }
                    continue;
                }
                match c {
                    '\x1b' => self.escape.push(c),
                    '\r' => self.cursor = 0,
                    '\x08' => self.cursor = self.cursor.saturating_sub(1),
                    '\n' => {
                        self.text.extend(line.drain(..));
                        self.text.push('\n');
                        self.cursor = 0;
                        if self.text.len() > 2 * TAIL_BYTES {
                            self.trim();
                        }
                    }
                    c if !c.is_control() || c == '\t' => {
                        let end = if c == '\t' {
                            (self.cursor / 8 + 1) * 8
                        } else {
                            self.cursor + 1
                        };
                        while self.cursor < end {
                            let value = if c == '\t' { ' ' } else { c };
                            while line.len() < self.cursor {
                                line.push(' ');
                            }
                            if self.cursor < line.len() {
                                line[self.cursor] = value;
                            } else {
                                line.push(value);
                            }
                            self.cursor += 1;
                        }
                        if line.len() > TAIL_BYTES {
                            let cut = line.len() - TAIL_BYTES / 2;
                            line.drain(..cut);
                            self.cursor = self.cursor.saturating_sub(cut);
                            self.truncated = true;
                        }
                    }
                    _ => {}
                }
            }
            self.text.extend(line);
            self.total += chunk.len() as u64 + self.text.len() as u64;
            self.start = self.total - self.text.len() as u64;
        }
        self.trim();
    }
    fn trim(&mut self) {
        if self.text.len() > TAIL_BYTES {
            let mut cut = self.text.len() - TAIL_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            if let Some(nl) = self.text[cut..].find('\n') {
                if cut + nl + 1 < self.text.len() {
                    cut += nl + 1;
                }
            }
            let old_line = self.text.rsplit('\n').next().unwrap_or("").chars().count();
            self.text.drain(..cut);
            self.start += cut as u64;
            self.truncated = true;
            let new_line = self.text.rsplit('\n').next().unwrap_or("").chars().count();
            self.cursor = self
                .cursor
                .saturating_sub(old_line.saturating_sub(new_line));
        }
    }
}

#[derive(Default)]
struct Tails {
    live: HashMap<Key, Tail>,
    order: VecDeque<Key>,
    finished: HashMap<Key, Tail>,
    finished_order: VecDeque<Key>,
}

pub struct Hub {
    tails: Mutex<Tails>,
    notify: Notifier,
}

impl Hub {
    pub fn new(notify: Notifier) -> Self {
        Self {
            tails: Mutex::default(),
            notify,
        }
    }
    pub fn append(&self, thread: &str, item: &str, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let key = (thread.to_owned(), item.to_owned());
        {
            let mut t = self.tails.lock().unwrap();
            if !t.live.contains_key(&key) {
                t.order.push_back(key.clone());
                if t.order.len() > MAX_LIVE {
                    if let Some(old) = t.order.pop_front() {
                        t.live.remove(&old);
                    }
                }
            }
            t.live.entry(key).or_default().push(chunk);
        }
        bump(&self.notify);
    }
    /// Keeps the tail briefly for settlements that carry no output of their own.
    pub fn finish(&self, thread: &str, item: &str) {
        let key = (thread.to_owned(), item.to_owned());
        let mut t = self.tails.lock().unwrap();
        if let Some(tail) = t.live.remove(&key) {
            t.order.retain(|k| *k != key);
            t.finished_order.push_back(key.clone());
            t.finished.insert(key, tail);
            if t.finished_order.len() > MAX_FINISHED {
                if let Some(old) = t.finished_order.pop_front() {
                    t.finished.remove(&old);
                }
            }
        }
        drop(t);
        bump(&self.notify);
    }
    fn tail(&self, key: &Key) -> Option<Tail> {
        let t = self.tails.lock().unwrap();
        t.live.get(key).or_else(|| t.finished.get(key)).cloned()
    }
}

fn active(status: &str) -> bool {
    matches!(status, "pending" | "running" | "waiting")
}

/// Last `MAX_FINAL_BYTES` of a persisted output, cut after a newline.
fn persisted(output: Option<&str>) -> (String, bool) {
    let Some(out) = output else {
        return (String::new(), false);
    };
    if out.chars().any(|c| matches!(c, '\r' | '\x1b' | '\x08')) {
        let mut folded = Tail::default();
        folded.push(out);
        return (folded.text, folded.truncated);
    }
    if out.len() <= MAX_FINAL_BYTES {
        return (out.to_owned(), false);
    }
    let mut cut = out.len() - MAX_FINAL_BYTES;
    while !out.is_char_boundary(cut) {
        cut += 1;
    }
    if let Some(nl) = out[cut..].find('\n') {
        if cut + nl + 1 < out.len() {
            cut += nl + 1;
        }
    }
    (out[cut..].to_owned(), true)
}

pub struct Cursor {
    key: Key,
    sent: u64,
    replace: bool,
    done: bool,
    waiting: bool,
    rewritten: u64,
}

impl Cursor {
    pub fn is_full(&self) -> bool {
        self.waiting
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn next(&mut self, s: &State) -> crate::terminal::Step {
        use crate::terminal::Step;
        if self.waiting {
            return Step::Idle;
        }
        if self.done {
            return Step::Done;
        }
        let item = {
            let db = s.db.lock().unwrap();
            get(&db, "turnItems", &self.key.1).ok()
        };
        let Some(item) = item.filter(|i| i["type"] == "command_execution") else {
            return Step::Done;
        };
        let tail = s.streams.commands.tail(&self.key);
        let frame = if !active(item["status"].as_str().unwrap_or("")) {
            self.done = true;
            match (&tail, item["output"].as_str()) {
                (Some(t), None) => {
                    json!({"kind":"replace","text":t.text,"truncated":t.truncated,"running":false})
                }
                (_, out) => {
                    let (text, truncated) = persisted(out);
                    json!({"kind":"replace","text":text,"truncated":truncated,"running":false})
                }
            }
        } else if let Some(t) = tail {
            let behind = self.sent < t.start || t.total - self.sent > MAX_APPEND_BYTES;
            if self.replace || behind || self.rewritten != t.rewritten {
                self.rewritten = t.rewritten;
                self.replace = false;
                self.sent = t.total;
                json!({"kind":"replace","text":t.text,"truncated":t.truncated,"running":true})
            } else if t.total > self.sent {
                let from = (self.sent - t.start) as usize;
                self.sent = t.total;
                json!({"kind":"append","text":t.text[from..],"truncated":t.truncated,"running":true})
            } else {
                return Step::Idle;
            }
        } else if self.replace {
            self.replace = false;
            let (text, truncated) = persisted(item["output"].as_str());
            json!({"kind":"replace","text":text,"truncated":truncated,"running":true})
        } else {
            return Step::Idle;
        };
        self.waiting = true;
        Step::Values(vec![frame])
    }
}

pub fn open_stream(s: &State, p: &Value) -> Result<Cursor, Value> {
    let (Some(thread), Some(item)) = (p["threadId"].as_str(), p["itemId"].as_str()) else {
        return Err(json!({"_tag":"RpcInvalidPayload","message":"threadId and itemId required"}));
    };
    let found = {
        let db = s.db.lock().unwrap();
        get(&db, "turnItems", item)
            .ok()
            .filter(|i| i["threadId"] == thread && i["type"] == "command_execution")
    };
    if found.is_none() {
        return Err(
            json!({"_tag":"OrchestrationV2CommandOutputError","threadId":thread,"message":"No such command."}),
        );
    }
    Ok(Cursor {
        key: (thread.into(), item.into()),
        sent: 0,
        replace: true,
        done: false,
        waiting: false,
        rewritten: 0,
    })
}

/// Applies a provider's command event; called by the run supervisor for each one.
pub fn persist(s: &State, tid: &str, rid: &str, event: CommandEvent) -> Result<(), String> {
    match event {
        CommandEvent::Output { item_id, chunk } => {
            s.streams.commands.append(tid, &item_id, &chunk);
            return Ok(());
        }
        CommandEvent::Started { item_id, input } => {
            let mut db = s.db.lock().unwrap();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let thread = get(&tx, "thread", tid)?;
            let at = iso(now_ms());
            let ordinal: i64 = tx
                .query_row(
                    "SELECT count(*) FROM records WHERE kind='turnItems' AND thread_id=?",
                    [tid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let mut item = json!({"id":item_id,"threadId":tid,"runId":rid,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"running","title":null,"startedAt":at,"completedAt":null,"updatedAt":at,"type":"command_execution","input":input});
            if let Ok(mut native) = get(&tx, "turnItems", &item_id) {
                native["input"] = item["input"].clone();
                native["status"] = json!("running");
                item = native;
            }
            store_item(&tx, tid, &item)?;
            append(&tx, tid, "turn-item.updated", item, &thread)?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        CommandEvent::Finished {
            item_id,
            exit_code,
            output,
        } => {
            {
                let mut db = s.db.lock().unwrap();
                let tx = db.transaction().map_err(|e| e.to_string())?;
                let thread = get(&tx, "thread", tid)?;
                let mut item = get(&tx, "turnItems", &item_id)?;
                let at = iso(now_ms());
                let tail = s.streams.commands.tail(&(tid.to_owned(), item_id.clone()));
                let final_output = output.or_else(|| tail.map(|t| t.text));
                item["status"] = json!(if exit_code.unwrap_or(0) == 0 {
                    "completed"
                } else {
                    "failed"
                });
                item["completedAt"] = json!(at);
                item["updatedAt"] = json!(at);
                if let Some(code) = exit_code {
                    item["exitCode"] = json!(code);
                }
                if let Some(out) = final_output {
                    item["output"] = json!(persisted(Some(&out)).0);
                }
                store_item(&tx, tid, &item)?;
                append(&tx, tid, "turn-item.updated", item, &thread)?;
                tx.commit().map_err(|e| e.to_string())?;
            }
            s.streams.commands.finish(tid, &item_id);
        }
    }
    s.notify();
    Ok(())
}

fn store_item(db: &rusqlite::Connection, tid: &str, item: &Value) -> Result<(), String> {
    let id = item["id"].as_str().ok_or("item id required")?;
    put(db, "turnItems", id, tid, item)?;
    put(
        db,
        "visibleTurnItems",
        id,
        tid,
        &json!({"position":item["ordinal"],"visibility":"local","sourceThreadId":tid,"sourceItemId":id,"item":item}),
    )
}

/// A provider that runs one scripted command: it prints, waits for an approval answer, prints
/// more and finishes. For tests and demos; the executable installs no provider.
pub struct ScriptedCommandProvider;
impl crate::provider::Provider for ScriptedCommandProvider {
    fn execute<'a>(
        &'a self,
        _input: crate::provider::RunInput,
        output: tokio::sync::mpsc::Sender<crate::provider::ProviderEvent>,
        mut control: tokio::sync::mpsc::Receiver<crate::provider::ProviderControl>,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        use crate::provider::{ProviderControl, ProviderEvent::*};
        Box::pin(async move {
            let item_id = "cmd-1".to_string();
            let send = |e| async {
                output
                    .send(e)
                    .await
                    .map_err(|_| "receiver closed".to_string())
            };
            send(Started).await?;
            send(Command(CommandEvent::Started {
                item_id: item_id.clone(),
                input: "echo hello; echo world".into(),
            }))
            .await?;
            send(Command(CommandEvent::Output {
                item_id: item_id.clone(),
                chunk: "hello\n".into(),
            }))
            .await?;
            send(ApprovalRequest {
                request_id: "approval-1".into(),
                command: "echo".into(),
            })
            .await?;
            tokio::select! {
                c = control.recv() => if !matches!(c, Some(ProviderControl::Respond { .. })) { return Ok(()) },
                _ = cancel.changed() => return Ok(()),
            }
            send(Command(CommandEvent::Output {
                item_id: item_id.clone(),
                chunk: "world\n".into(),
            }))
            .await?;
            send(Command(CommandEvent::Finished {
                item_id,
                exit_code: Some(0),
                output: None,
            }))
            .await?;
            send(Completed).await
        })
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    #[test]
    fn progress_rewrites_split_controls_and_bounds_unicode() {
        let mut t = Tail::default();
        t.push("step 100%");
        t.push("\rstep 2\x1b[");
        t.push("K\nnext\x08!");
        assert_eq!(t.text, "step 2\nnex!");
        assert!(t.rewritten > 0);
        t.push(&"é".repeat(TAIL_BYTES));
        assert!(t.text.len() <= TAIL_BYTES);
        assert!(t.truncated);
        t.push(&format!("\r{}", "x".repeat(1024 * 1024)));
        assert!(t.text.len() <= TAIL_BYTES);
    }
}
