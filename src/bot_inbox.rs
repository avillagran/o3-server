//! Persistent mailboxes for external bots ("bot inbox").
//!
//! MCP is pull-only: the server cannot wake an external client such as a ChatGPT connector.
//! Agents running in O3 threads therefore drop messages into a durable per-bot mailbox with
//! `t3_bot_inbox_post`, which never starts a provider turn. The bot long-polls with
//! `t3_bot_inbox_wait`, acknowledges with `t3_bot_inbox_ack`, and answers with
//! `t3_bot_inbox_reply`, which both records the answer and delivers it to the originating
//! thread through the ordinary `t3_thread_send` path.
//!
//! Access model (see `Authority::inbox_caller`):
//! - native agent credentials may post (only as their own thread) and read their own thread's
//!   messages;
//! - credentials issued with `--bot-id` are bound to exactly that mailbox;
//! - other external credentials need `orchestration:operate` and may address any mailbox.
use crate::{
    events,
    mcp_access::{Authority, InboxCaller},
    mcp_tools::{definition, failure},
    store::{iso, now_ms, random_id, State},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

type Reply = Result<Value, Value>;

const BOT_ID_PATTERN: &str = "^[a-z0-9][a-z0-9._-]{0,63}$";
const BODY_MAX_UNITS: usize = 20_000;
/// Undelivered `to_bot` messages one mailbox may hold before posting is refused.
const MAX_PENDING: i64 = 200;
/// Rows retained per mailbox; the oldest acknowledged rows are dropped first.
const MAX_RETAINED: i64 = 1_000;
const MAX_BOTS: i64 = 64;
const ACKED_TTL_MS: i64 = 14 * 24 * 60 * 60 * 1000;
const DEFAULT_WAIT_MS: f64 = 25_000.0;
const MAX_WAIT_MS: f64 = 50_000.0;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const COLUMNS: &str = "seq,id,bot_id,direction,thread_id,environment_id,body,correlation_id,reply_to,created_at,acked_at";

pub(crate) fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS bot_inbox(
           seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, bot_id TEXT NOT NULL,
           direction TEXT NOT NULL CHECK(direction IN ('to_bot','from_bot')), thread_id TEXT NOT NULL,
           environment_id TEXT NOT NULL, body TEXT NOT NULL, correlation_id TEXT, reply_to TEXT,
           request_key TEXT, created_at INTEGER NOT NULL, acked_at INTEGER);
         CREATE INDEX IF NOT EXISTS bot_inbox_bot ON bot_inbox(bot_id, seq);
         CREATE UNIQUE INDEX IF NOT EXISTS bot_inbox_request ON bot_inbox(bot_id, direction, request_key) WHERE request_key IS NOT NULL;
         CREATE INDEX IF NOT EXISTS bot_inbox_pending ON bot_inbox(bot_id, seq) WHERE direction='to_bot' AND acked_at IS NULL;",
    )
    .map_err(|e| e.to_string())
}

/// Mailbox names are stable lowercase tokens such as `dot`.
pub fn valid_bot_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
        })
}

fn string(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
fn bot_id() -> Value {
    json!({"type":"string","minLength":1,"maxLength":64,"pattern":BOT_ID_PATTERN})
}
fn write(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut tool = definition(name, description, properties, required);
    tool["annotations"] = json!({"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":true});
    tool
}

pub(crate) fn catalog() -> Vec<Value> {
    let cursor = json!({"type":"integer","minimum":0});
    let limit = json!({"type":"integer","minimum":1,"maximum":MAX_LIMIT});
    vec![
        write(
            "t3_bot_inbox_post",
            "Leave a message for an external bot (for example one reached over MCP) in its durable mailbox. This never starts a turn in any thread. The bot picks it up with t3_bot_inbox_wait and answers in this thread. Inside a thread the message is attributed to that thread; external operators pass threadId.",
            json!({"botId":bot_id(),"body":string(BODY_MAX_UNITS),"correlationId":string(256),"threadId":string(256),"clientRequestId":string(256)}),
            &["botId", "body"],
        ),
        definition(
            "t3_bot_inbox_wait",
            "Long-poll a bot mailbox for undelivered messages addressed to the bot. Returns as soon as one exists, or empty after timeoutMs (default 25000, at most 50000). Messages stay pending until acknowledged; pass afterCursor (the previous nextCursor) to skip ones already seen.",
            json!({"botId":bot_id(),"afterCursor":cursor.clone(),"timeoutMs":{"type":"number"},"limit":limit.clone()}),
            &[],
        ),
        definition(
            "t3_bot_inbox_read",
            "Read mailbox messages without waiting, oldest first. direction filters to_bot, from_bot or any (default). Inside a thread only that thread's messages are visible.",
            json!({"botId":bot_id(),"afterCursor":cursor,"direction":{"type":"string","enum":["to_bot","from_bot","any"]},"pendingOnly":{"type":"boolean"},"limit":limit}),
            &[],
        ),
        write(
            "t3_bot_inbox_ack",
            "Confirm receipt of to_bot messages by id so they are no longer delivered by t3_bot_inbox_wait.",
            json!({"botId":bot_id(),"messageIds":{"type":"array","minItems":1,"maxItems":100,"items":string(256)}}),
            &["messageIds"],
        ),
        write(
            "t3_bot_inbox_reply",
            "Answer a mailbox message: records the reply as from_bot, acknowledges the original, and delivers the text to the thread that posted it (same delivery as t3_thread_send, so it wakes that thread's agent). Reuse clientRequestId for retries.",
            json!({"botId":bot_id(),"messageId":string(256),"body":string(BODY_MAX_UNITS),"mode":{"type":"string","enum":["auto","queue","steer"]},"clientRequestId":string(256)}),
            &["messageId", "body"],
        ),
    ]
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "t3_bot_inbox_post"
            | "t3_bot_inbox_wait"
            | "t3_bot_inbox_read"
            | "t3_bot_inbox_ack"
            | "t3_bot_inbox_reply"
    )
}

fn denied(message: &str) -> Value {
    failure("capability_denied", message)
}
fn internal<E>(_: E) -> Value {
    failure(
        "orchestration_error",
        "Unable to complete the bot inbox operation.",
    )
}

/// Who may call what, and which mailbox they address.
fn authorize(
    s: &State,
    authority: &Authority,
    name: &str,
    p: &Value,
) -> Result<(InboxCaller, String), Value> {
    let operating = matches!(name, "t3_bot_inbox_post" | "t3_bot_inbox_reply");
    let caller = authority.inbox_caller(&s.reader.lock().unwrap(), operating)?;
    let asked = p["botId"].as_str().map(str::trim);
    let bot = match (&caller, asked) {
        (InboxCaller::Bot(bound), Some(asked)) if asked != bound => {
            return Err(denied("This credential is bound to a different bot."))
        }
        (InboxCaller::Bot(bound), _) => bound.clone(),
        (_, Some(asked)) if valid_bot_id(asked) => asked.to_owned(),
        _ => {
            return Err(failure(
                "target_required",
                "Pass botId: the mailbox to use.",
            ))
        }
    };
    let allowed = match (&caller, name) {
        (InboxCaller::Native(_), "t3_bot_inbox_post" | "t3_bot_inbox_read") => true,
        (InboxCaller::Native(_), _) => false,
        (InboxCaller::Bot(_), "t3_bot_inbox_post") => false,
        (InboxCaller::Bot(_) | InboxCaller::Operator, _) => true,
    };
    if !allowed {
        return Err(denied("This credential cannot use that bot inbox tool."));
    }
    Ok((caller, bot))
}

pub(crate) async fn call(s: Arc<State>, authority: Authority, name: &str, p: Value) -> Reply {
    let (caller, bot) = {
        let s = s.clone();
        let authority = authority.clone();
        let name = name.to_owned();
        let p = p.clone();
        tokio::task::spawn_blocking(move || authorize(&s, &authority, &name, &p))
            .await
            .map_err(internal)??
    };
    if name == "t3_bot_inbox_wait" {
        return wait(s, authority, caller, bot, p).await;
    }
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || match name.as_str() {
        "t3_bot_inbox_post" => post(&s, &caller, &bot, &p),
        "t3_bot_inbox_read" => read(&s, &caller, &bot, &p),
        "t3_bot_inbox_ack" => ack(&s, &bot, &p),
        _ => reply(&s, &authority, &bot, &p),
    })
    .await
    .map_err(internal)?
}

fn message(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let acked: Option<i64> = r.get(10)?;
    Ok(json!({
        "cursor": r.get::<_, i64>(0)?,
        "id": r.get::<_, String>(1)?,
        "botId": r.get::<_, String>(2)?,
        "direction": r.get::<_, String>(3)?,
        "threadId": r.get::<_, String>(4)?,
        "environmentId": r.get::<_, String>(5)?,
        "body": r.get::<_, String>(6)?,
        "correlationId": r.get::<_, Option<String>>(7)?,
        "replyTo": r.get::<_, Option<String>>(8)?,
        "createdAt": iso(r.get::<_, i64>(9)?),
        "ackedAt": acked.map(iso),
    }))
}

fn fetch(
    db: &Connection,
    bot: &str,
    direction: Option<&str>,
    thread: Option<&str>,
    after: i64,
    pending_only: bool,
    limit: usize,
) -> Result<Vec<Value>, Value> {
    let sql = format!(
        "SELECT {COLUMNS} FROM bot_inbox WHERE bot_id=?1 AND seq>?2 AND (?3 IS NULL OR direction=?3) AND (?4 IS NULL OR thread_id=?4) AND (?5=0 OR (direction='to_bot' AND acked_at IS NULL)) ORDER BY seq LIMIT ?6"
    );
    let mut q = db.prepare(&sql).map_err(internal)?;
    let rows = q
        .query_map(
            params![
                bot,
                after,
                direction,
                thread,
                pending_only as i64,
                limit as i64
            ],
            message,
        )
        .map_err(internal)?;
    rows.collect::<Result<_, _>>().map_err(internal)
}

fn limit(p: &Value) -> usize {
    p["limit"]
        .as_u64()
        .map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT))
}
fn after(p: &Value) -> i64 {
    p["afterCursor"].as_i64().unwrap_or(0).max(0)
}

fn collect(rows: Vec<Value>, after: i64) -> Value {
    let next = rows
        .iter()
        .filter_map(|m| m["cursor"].as_i64())
        .max()
        .unwrap_or(after);
    json!({"messages":rows,"nextCursor":next})
}

fn post(s: &State, caller: &InboxCaller, bot: &str, p: &Value) -> Reply {
    let thread = match caller {
        InboxCaller::Native(thread) => thread.clone(),
        _ => p["threadId"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                failure(
                    "target_required",
                    "Pass threadId: the thread the bot should answer in.",
                )
            })?
            .to_owned(),
    };
    {
        let db = s.reader.lock().unwrap();
        events::get(&db, "thread", &thread)
            .ok()
            .filter(|t| t["deletedAt"].is_null())
            .ok_or_else(|| failure("thread_not_found", "The thread was not found."))?;
    }
    let sent = insert(
        s,
        bot,
        "to_bot",
        &thread,
        p["body"].as_str().unwrap_or(""),
        p["correlationId"].as_str().map(str::trim),
        None,
        p["clientRequestId"].as_str().map(str::trim),
    )?;
    Ok(
        json!({"messageId":sent["id"],"cursor":sent["cursor"],"botId":bot,"threadId":thread,"correlationId":sent["correlationId"],"status":"queued"}),
    )
}

/// One write transaction: retention, admission and the insert. A repeated request key returns
/// the message already stored.
#[allow(clippy::too_many_arguments)]
fn insert(
    s: &State,
    bot: &str,
    direction: &str,
    thread: &str,
    body: &str,
    correlation: Option<&str>,
    reply_to: Option<&str>,
    request_key: Option<&str>,
) -> Result<Value, Value> {
    s.wait_for_wal_budget();
    let id = random_id().map_err(internal)?;
    let now = now_ms();
    let stored = {
        let db = s.db.lock().unwrap();
        let tx = db.unchecked_transaction().map_err(internal)?;
        if let Some(key) = request_key {
            let prior = tx
                .query_row(
                    &format!("SELECT {COLUMNS} FROM bot_inbox WHERE bot_id=?1 AND direction=?2 AND request_key=?3"),
                    params![bot, direction, key],
                    message,
                )
                .optional()
                .map_err(internal)?;
            if let Some(prior) = prior {
                return Ok(prior);
            }
        }
        tx.execute(
            "DELETE FROM bot_inbox WHERE acked_at IS NOT NULL AND acked_at<?1",
            [now - ACKED_TTL_MS],
        )
        .map_err(internal)?;
        let (known, total, pending): (i64, i64, i64) = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM bot_inbox WHERE bot_id=?1), (SELECT count(*) FROM bot_inbox WHERE bot_id=?1), (SELECT count(*) FROM bot_inbox WHERE bot_id=?1 AND direction='to_bot' AND acked_at IS NULL)",
                [bot],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(internal)?;
        if known == 0 {
            let bots: i64 = tx
                .query_row("SELECT count(DISTINCT bot_id) FROM bot_inbox", [], |r| {
                    r.get(0)
                })
                .map_err(internal)?;
            if bots >= MAX_BOTS {
                return Err(failure("inbox_limit", "Too many bot mailboxes exist."));
            }
        }
        if direction == "to_bot" && pending >= MAX_PENDING {
            return Err(failure(
                "inbox_full",
                "The bot has too many undelivered messages; wait for it to acknowledge some.",
            ));
        }
        if total >= MAX_RETAINED {
            // Acknowledged and bot-authored rows go first; undelivered requests are kept.
            tx.execute(
                "DELETE FROM bot_inbox WHERE seq IN (SELECT seq FROM bot_inbox WHERE bot_id=?1 AND NOT (direction='to_bot' AND acked_at IS NULL) ORDER BY seq LIMIT ?2)",
                params![bot, total - MAX_RETAINED + 1],
            )
            .map_err(internal)?;
        }
        tx.execute(
            "INSERT INTO bot_inbox(id,bot_id,direction,thread_id,environment_id,body,correlation_id,reply_to,request_key,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![id, bot, direction, thread, s.environment_id, body, correlation, reply_to, request_key, now],
        )
        .map_err(internal)?;
        let stored = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM bot_inbox WHERE id=?1"),
                [&id],
                message,
            )
            .map_err(internal)?;
        tx.commit().map_err(internal)?;
        stored
    };
    s.notify();
    Ok(stored)
}

fn read(s: &State, caller: &InboxCaller, bot: &str, p: &Value) -> Reply {
    let direction = match p["direction"].as_str() {
        Some("to_bot") => Some("to_bot"),
        Some("from_bot") => Some("from_bot"),
        _ => None,
    };
    let thread = match caller {
        InboxCaller::Native(thread) => Some(thread.as_str()),
        _ => None,
    };
    let after = after(p);
    let rows = fetch(
        &s.reader.lock().unwrap(),
        bot,
        direction,
        thread,
        after,
        p["pendingOnly"].as_bool().unwrap_or(false),
        limit(p),
    )?;
    let mut out = collect(rows, after);
    out["botId"] = json!(bot);
    Ok(out)
}

fn ack(s: &State, bot: &str, p: &Value) -> Reply {
    let ids: Vec<&str> = p["messageIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .collect();
    s.wait_for_wal_budget();
    let now = now_ms();
    let mut acked = 0;
    {
        let db = s.db.lock().unwrap();
        let tx = db.unchecked_transaction().map_err(internal)?;
        for id in ids {
            acked += tx
                .execute(
                    "UPDATE bot_inbox SET acked_at=?1 WHERE id=?2 AND bot_id=?3 AND direction='to_bot' AND acked_at IS NULL",
                    params![now, id, bot],
                )
                .map_err(internal)?;
        }
        tx.commit().map_err(internal)?;
    }
    Ok(json!({"botId":bot,"acked":acked}))
}

fn reply(s: &Arc<State>, authority: &Authority, bot: &str, p: &Value) -> Reply {
    let id = p["messageId"].as_str().map(str::trim).unwrap_or("");
    let (thread, correlation): (String, Option<String>) = s
        .reader
        .lock()
        .unwrap()
        .query_row(
            "SELECT thread_id,correlation_id FROM bot_inbox WHERE id=?1 AND bot_id=?2 AND direction='to_bot'",
            params![id, bot],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| failure("invalid_request", "The mailbox message was not found."))?;
    let body = p["body"].as_str().unwrap_or("");
    let request_key = p["clientRequestId"].as_str().map(str::trim);
    let header = match &correlation {
        Some(c) => format!("[Reply from bot {bot} · correlationId: {c}]"),
        None => format!("[Reply from bot {bot}]"),
    };
    let mut send = json!({
        "threadId": thread,
        "message": format!("{header}\n\n{body}"),
        "mode": p["mode"].as_str().unwrap_or("auto"),
    });
    // Retries of one reply must not deliver twice.
    if let Some(key) = request_key {
        send["clientRequestId"] = json!(format!("bot-reply:{bot}:{key}"));
    }
    let delivery = crate::mcp_mutations::call(s, authority, "t3_thread_send", &send)?;
    let stored = insert(
        s,
        bot,
        "from_bot",
        &thread,
        body,
        correlation.as_deref(),
        Some(id),
        request_key,
    )?;
    ack(s, bot, &json!({"messageIds":[id]}))?;
    Ok(json!({
        "messageId": stored["id"],
        "cursor": stored["cursor"],
        "repliedTo": id,
        "threadId": thread,
        "correlationId": correlation,
        "delivery": delivery,
    }))
}

async fn wait(
    s: Arc<State>,
    authority: Authority,
    caller: InboxCaller,
    bot: String,
    p: Value,
) -> Reply {
    let Ok(_slot) = s.mcp.bot_waits.clone().try_acquire_owned() else {
        return Err(failure(
            "too_many_waiters",
            "Too many bot inbox waits are already open; retry shortly.",
        ));
    };
    let timeout = p["timeoutMs"]
        .as_f64()
        .unwrap_or(DEFAULT_WAIT_MS)
        .clamp(0.0, MAX_WAIT_MS);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout / 1000.0);
    let after = after(&p);
    let limit = limit(&p);
    let mut changed = s.changed.subscribe();
    loop {
        let rows = {
            let (s, authority, bot, caller) =
                (s.clone(), authority.clone(), bot.clone(), caller.clone());
            tokio::task::spawn_blocking(move || -> Result<Vec<Value>, Value> {
                // Revocation or expiry ends a long poll at its next wake-up.
                let now = authority.inbox_caller(&s.reader.lock().unwrap(), false)?;
                if now != caller {
                    return Err(denied("The MCP credential changed while waiting."));
                }
                fetch(
                    &s.reader.lock().unwrap(),
                    &bot,
                    Some("to_bot"),
                    None,
                    after,
                    true,
                    limit,
                )
            })
            .await
            .map_err(internal)??
        };
        let expired = Instant::now() >= deadline;
        if !rows.is_empty() || expired {
            let mut out = collect(rows, after);
            out["botId"] = json!(bot);
            out["timedOut"] = json!(out["messages"].as_array().is_some_and(Vec::is_empty));
            return Ok(out);
        }
        tokio::select! {
            _ = changed.changed() => {},
            _ = tokio::time::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(5))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::put;
    use std::path::PathBuf;

    fn fixture() -> (Arc<State>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("o3-bot-inbox-{}", random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"enableAgentBrowserAccess":false}"#,
        )
        .unwrap();
        let state = Arc::new(State::open(&dir).unwrap());
        state.native_mcp.listen("127.0.0.1:12345".parse().unwrap());
        {
            let db = state.db.lock().unwrap();
            for id in ["parent", "other"] {
                put(&db, "thread", id, id, &json!({"id":id,"projectId":"p","providerInstanceId":"claudeAgent","runtimeMode":"full-access","interactionMode":"default","providerGeneration":"g"})).unwrap();
            }
            put(
                &db,
                "runs",
                "r",
                "parent",
                &json!({"id":"r","providerInstanceId":"claudeAgent","status":"running"}),
            )
            .unwrap();
        }
        (state, dir)
    }

    #[test]
    fn bot_ids_are_stable_lowercase_tokens() {
        for ok in ["dot", "a", "dot-2", "chat.gpt_1", &"a".repeat(64)] {
            assert!(valid_bot_id(ok), "{ok}");
        }
        for bad in [
            "",
            "Dot",
            "-dot",
            ".x",
            "a b",
            "a/b",
            "dot\n",
            "ñ",
            &"a".repeat(65),
        ] {
            assert!(!valid_bot_id(bad), "{bad:?}");
        }
        let pattern = regex::Regex::new(BOT_ID_PATTERN).unwrap();
        for id in ["dot", "-dot", "Dot", "a_b", "dot\n"] {
            assert_eq!(pattern.is_match(id), valid_bot_id(id), "{id:?}");
        }
    }

    #[tokio::test]
    async fn native_agents_post_as_their_own_thread_and_only_see_their_own_messages() {
        let (state, dir) = fixture();
        let context = state
            .native_mcp
            .obtain(&state, "parent", "claudeAgent", &dir, &Value::Null)
            .await
            .unwrap();
        let token = context.config()["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap()
            .to_owned();
        let authority = Authority::from_native(state.native_mcp.resolve(&token).unwrap());
        let posted = call(
            state.clone(),
            authority.clone(),
            "t3_bot_inbox_post",
            json!({"botId":"dot","body":"need the invoice id","correlationId":"c1","threadId":"other"}),
        )
        .await
        .unwrap();
        assert_eq!(
            posted["threadId"], "parent",
            "an agent cannot post as another thread"
        );
        // A message another thread left for the same bot stays invisible to this agent.
        insert(
            &state, "dot", "to_bot", "other", "private", None, None, None,
        )
        .unwrap();
        let read = call(
            state.clone(),
            authority.clone(),
            "t3_bot_inbox_read",
            json!({"botId":"dot"}),
        )
        .await
        .unwrap();
        let bodies: Vec<_> = read["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["body"].clone())
            .collect();
        assert_eq!(bodies, [json!("need the invoice id")]);
        for tool in [
            "t3_bot_inbox_wait",
            "t3_bot_inbox_ack",
            "t3_bot_inbox_reply",
        ] {
            let denied = call(
                state.clone(),
                authority.clone(),
                tool,
                json!({"botId":"dot","timeoutMs":0,"messageIds":["x"],"messageId":"x","body":"b"}),
            )
            .await
            .unwrap_err();
            assert_eq!(denied["code"], "capability_denied", "{tool}");
        }
        // Posting needs a live turn; reading does not.
        {
            let db = state.db.lock().unwrap();
            put(
                &db,
                "runs",
                "r",
                "parent",
                &json!({"id":"r","providerInstanceId":"claudeAgent","status":"completed"}),
            )
            .unwrap();
        }
        let idle = call(
            state.clone(),
            authority.clone(),
            "t3_bot_inbox_post",
            json!({"botId":"dot","body":"late"}),
        )
        .await
        .unwrap_err();
        assert_eq!(idle["code"], "parent_not_active");
        call(
            state.clone(),
            authority,
            "t3_bot_inbox_read",
            json!({"botId":"dot"}),
        )
        .await
        .unwrap();
        drop(context);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn messages_survive_restart_with_monotonic_cursors() {
        let dir = std::env::temp_dir().join(format!("o3-bot-inbox-{}", random_id().unwrap()));
        let first = {
            let state = State::open(&dir).unwrap();
            let a = insert(&state, "dot", "to_bot", "t", "one", Some("c"), None, None).unwrap();
            let b = insert(&state, "dot", "to_bot", "t", "two", None, None, None).unwrap();
            assert!(b["cursor"].as_i64() > a["cursor"].as_i64());
            a["id"].as_str().unwrap().to_owned()
        };
        let state = State::open(&dir).unwrap();
        ack(&state, "dot", &json!({"messageIds":[first]})).unwrap();
        let pending = fetch(
            &state.reader.lock().unwrap(),
            "dot",
            Some("to_bot"),
            None,
            0,
            true,
            10,
        )
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["body"], "two");
        let c = insert(&state, "dot", "to_bot", "t", "three", None, None, None).unwrap();
        assert!(c["cursor"].as_i64() > pending[0]["cursor"].as_i64());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mailboxes_are_bounded_and_request_keys_deduplicate() {
        let dir = std::env::temp_dir().join(format!("o3-bot-inbox-{}", random_id().unwrap()));
        let state = State::open(&dir).unwrap();
        let again =
            |key| insert(&state, "dot", "to_bot", "t", "same", None, None, Some(key)).unwrap();
        assert_eq!(again("k")["id"], again("k")["id"]);
        {
            let db = state.db.lock().unwrap();
            let tx = db.unchecked_transaction().unwrap();
            for n in 0..MAX_PENDING - 1 {
                tx.execute(
                    "INSERT INTO bot_inbox(id,bot_id,direction,thread_id,environment_id,body,created_at) VALUES(?,?,?,?,?,?,?)",
                    params![format!("p{n}"), "dot", "to_bot", "t", "e", "x", now_ms()],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let full = insert(&state, "dot", "to_bot", "t", "overflow", None, None, None).unwrap_err();
        assert_eq!(full["code"], "inbox_full");
        // The bot's own replies are not blocked by a full request queue.
        insert(&state, "dot", "from_bot", "t", "answer", None, None, None).unwrap();
        // Acknowledged history is trimmed to the retention bound; pending requests are kept.
        ack(&state, "dot", &json!({"messageIds":(0..MAX_PENDING - 1).map(|n| format!("p{n}")).collect::<Vec<_>>()})).unwrap();
        {
            let db = state.db.lock().unwrap();
            let tx = db.unchecked_transaction().unwrap();
            for n in 0..MAX_RETAINED {
                tx.execute(
                    "INSERT INTO bot_inbox(id,bot_id,direction,thread_id,environment_id,body,created_at,acked_at) VALUES(?,?,?,?,?,?,?,?)",
                    params![format!("h{n}"), "dot", "to_bot", "t", "e", "x", now_ms(), now_ms()],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        insert(&state, "dot", "to_bot", "t", "fresh", None, None, None).unwrap();
        let (total, pending): (i64, i64) = state
            .reader
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*), sum(direction='to_bot' AND acked_at IS NULL) FROM bot_inbox WHERE bot_id='dot'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(total <= MAX_RETAINED, "{total}");
        assert_eq!(pending, 2, "the unacknowledged requests survive trimming");
        for n in 0..MAX_BOTS {
            let _ = insert(
                &state,
                &format!("bot{n}"),
                "to_bot",
                "t",
                "x",
                None,
                None,
                None,
            );
        }
        let many = insert(&state, "one-more", "to_bot", "t", "x", None, None, None).unwrap_err();
        assert_eq!(many["code"], "inbox_limit");
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
