//! Bounded provider failure records and terminal cards, pinned to nightly 4262.
use regex::{Captures, Regex};
use serde_json::{json, Value};
use std::sync::OnceLock;

const DEFAULT: &str = "Provider turn failed.";
const HANDOFF_BUDGET: &str = "The request leaves too little context for a provider handoff. Shorten the request or use a larger-context model; the current request has not been truncated.";
const HANDOFF_BACKGROUND: &str = "The target provider still has background work running. Wait for it to finish or stop it before retrying the handoff.";

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '\u{212a}' | '\u{017f}')
}
fn before_word(text: &str, start: usize) -> bool {
    text[..start].chars().next_back().is_some_and(word)
}

fn redactors() -> &'static [Regex; 5] {
    static PATTERNS: OnceLock<[Regex; 5]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        // ECMAScript whitespace excludes U+0085 and includes U+FEFF.
        let ws = r"\x09-\x0d\x20\u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
        let key = r"[a-z0-9_-]*(?:access[_-]?token|api[_-]?key|authorization|credential|password|secret|token)";
        [
            format!(r#"(?i)https?://[^{ws}<>"']+"#),
            format!(r"(?i)(Bearer|Basic)[{ws}]+[^{ws},;]+"),
            format!(r#"(?i)(["']{key}["'][{ws}]*:[{ws}]*["'])[^"']*(["'])"#),
            format!(r#"(?i)({key}[{ws}]*[:=][{ws}]*)(?:"[^"]*"|'[^']*'|[^{ws},;]+)"#),
            r"sk-[A-Za-z0-9_-]{16,}".into(),
        ]
        .map(|pattern| Regex::new(&pattern).unwrap())
    })
}

fn redact(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\0'..='\u{0008}' | '\u{000b}'..='\u{000c}' | '\u{000e}'..='\u{001f}' | '\u{007f}') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let patterns = redactors();
    let urls = patterns[0].replace_all(&cleaned, |caps: &Captures<'_>| {
        let found = caps.get(0).unwrap();
        if before_word(&cleaned, found.start()) {
            return found.as_str().to_owned();
        }
        let candidate = found
            .as_str()
            .trim_end_matches([')', ',', '.', ';', '!', '?']);
        let suffix = &found.as_str()[candidate.len()..];
        match url::Url::parse(candidate) {
            Ok(mut url) => {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                format!("{url}{suffix}")
            }
            Err(_) => "[REDACTED_URL]".into(),
        }
    });
    let auth = patterns[1].replace_all(&urls, |caps: &Captures<'_>| {
        if before_word(&urls, caps.get(0).unwrap().start()) {
            caps[0].to_owned()
        } else {
            format!("{} [REDACTED]", &caps[1])
        }
    });
    let quoted = patterns[2].replace_all(&auth, "${1}[REDACTED]${2}");
    let fields = patterns[3].replace_all(&quoted, |caps: &Captures<'_>| {
        let start = caps.get(0).unwrap().start();
        if before_word(&quoted, start) || quoted[..start].ends_with('-') {
            caps[0].to_owned()
        } else {
            format!("{}[REDACTED]", &caps[1])
        }
    });
    let keys = patterns[4].replace_all(&fields, |caps: &Captures<'_>| {
        let found = caps.get(0).unwrap();
        // This upstream expression has /gu, without case folding: its word
        // boundaries exclude the Kelvin sign and long s accepted by /giu.
        let ascii_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        if fields[..found.start()]
            .chars()
            .next_back()
            .is_some_and(ascii_word)
        {
            return found.as_str().to_owned();
        }
        let mut end = found.end();
        while end > found.start() + 3 {
            let left = fields[..end].chars().next_back().unwrap();
            let right = fields[end..].chars().next();
            if ascii_word(left) != right.is_some_and(ascii_word) {
                break;
            }
            end -= left.len_utf8();
        }
        if end < found.start() + 19 {
            found.as_str().to_owned()
        } else {
            format!("[REDACTED]{}", &fields[end..found.end()])
        }
    });
    crate::native_goal::trim(&keys).into()
}

fn bounded(text: &str, limit: usize) -> String {
    let text = redact(text);
    if text.encode_utf16().count() <= limit {
        return text;
    }
    let mut count = 0;
    let mut output = String::new();
    for c in text.chars() {
        if count + c.len_utf16() >= limit {
            break;
        }
        count += c.len_utf16();
        output.push(c);
    }
    output.push('…');
    output
}

fn category(tag: &str) -> Option<(&'static str, &'static str)> {
    Some(match tag {
        "ProviderAdapterTurnStartError" => (
            "The provider could not start this turn",
            "Retry the turn; if it keeps failing, check the provider setup and server logs.",
        ),
        "ProviderAdapterEventStreamError" => (
            "The provider event stream closed unexpectedly",
            "Retry the turn; if it keeps failing, check the provider and server logs.",
        ),
        "ProviderAdapterOpenSessionError" => (
            "The provider session could not be opened",
            "Check that the provider is installed and signed in, then retry the turn.",
        ),
        "ProviderAdapterResumeThreadError" => (
            "The provider conversation could not be resumed",
            "Retry the turn; if it keeps failing, check the provider and server logs.",
        ),
        _ => return None,
    })
}

fn curated(cause: &Value) -> (Option<String>, Option<String>) {
    static VALIDATION: OnceLock<Regex> = OnceLock::new();
    let validation = VALIDATION.get_or_init(|| {
        Regex::new(r"^goal objective must (?:not be empty|be at most [0-9]{1,6} characters)$")
            .unwrap()
    });
    let mut cause = cause;
    let mut wrapper = None;
    let mut reason = None;
    let describe = |wrapper: Option<(&str, &str)>, reason: Option<&str>| {
        wrapper.map(|(summary, guidance)| {
            if let Some(reason) = reason {
                format!("{summary}: {reason}. {guidance}")
            } else {
                format!("{summary}. {guidance}")
            }
        })
    };
    for _ in 0..16 {
        if !cause.is_object() {
            break;
        }
        let tag = cause["_tag"].as_str().unwrap_or("");
        let known = match tag {
            "ContextHandoffBudgetError" => Some(HANDOFF_BUDGET.into()),
            "ContextHandoffBackgroundWorkError" => Some(HANDOFF_BACKGROUND.into()),
            "AcpModelSelectionError" | "ClaudeBackgroundWorkBlocksQueryReplacementError" | "ClaudeAppendSystemPromptFileError" | "ClaudeAppendSystemPromptMissingValueError" | "ClaudeSettingsLaunchArgError" | "ClaudeMcpRegistrationTimeoutError" | "ClaudeCodeRootRefusalError" | "CursorModelUnavailableError" | "CodexGoalObjectiveTooLongError" | "ProviderSetupError" | "ProviderAdapterThreadInUseError" => {
                return (cause["message"].as_str().map(str::to_owned), describe(wrapper, reason));
            }
            "CodexAppServerRequestError" => {
                cause["message"].as_str().filter(|message| cause["method"] == "thread/goal/set" && validation.is_match(message)).map(|message| format!("Codex rejected the goal: {message}."))
            }
            "AcpProcessExitedError" => {
                let signal = cause["signal"].as_str().filter(|signal| signal.strip_prefix("SIG").is_some_and(|suffix| !suffix.is_empty() && suffix.len() <= 16 && suffix.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())));
                let status = if let Some(signal) = signal {
                    format!(" after signal {signal}")
                } else if let Some(code) = cause["code"].as_f64().filter(|n| n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0) {
                    format!(" with code {code:.0}")
                } else { String::new() };
                let prefix = wrapper.map(|(summary, _)| format!("{summary}: the")).unwrap_or_else(|| "The".into());
                Some(format!("{prefix} provider process exited{status}. Check the server logs for the provider's output, then retry the turn."))
            }
            "ContextHandoffDeliveryUncertainError" => Some("T3 could not confirm whether conversation history reached the provider. Retry the turn to recover the session.".into()),
            "ProviderAdapterProtocolError" => {
                if wrapper.is_some() && reason.is_none() {
                    reason = match cause["reason"].as_str() {
                        Some("previous_turn_active") => Some("the previous turn is still running"),
                        Some("attachment_unreadable") => Some("an attachment could not be read"),
                        _ => None,
                    };
                }
                None
            }
            _ => None,
        };
        if known.is_some() {
            return (known, describe(wrapper, reason));
        }
        if let Some(category) = category(tag) {
            wrapper = Some(category);
            reason = None;
        }
        cause = &cause["cause"];
    }
    (None, describe(wrapper, reason))
}

pub(crate) fn make(input: &Value) -> Value {
    let (known, category) = curated(&input["cause"]);
    let message = bounded(
        input["message"]
            .as_str()
            .or(known.as_deref())
            .or(category.as_deref())
            .unwrap_or(DEFAULT),
        4096,
    );
    let code = input["code"]
        .as_str()
        .or_else(|| input["cause"]["code"].as_str())
        .map(|code| bounded(code, 128))
        .filter(|code| !code.is_empty());
    let mut failure = json!({"class":input["class"].as_str().unwrap_or("unknown"),"message":if message.is_empty(){DEFAULT}else{&message},"code":code,"retryable":input["retryable"].as_bool()});
    if input["class"] == "usage_limit" {
        if let Some(raw) = input["resetAt"].as_str() {
            let instant = chrono::DateTime::parse_from_rfc3339(raw)
                .map(|d| d.with_timezone(&chrono::Utc))
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                        .ok()
                        .and_then(|date| date.and_hms_opt(0, 0, 0))
                        .map(|date| date.and_utc())
                });
            if let Some(instant) = instant {
                failure["resetAt"] =
                    json!(instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
            }
        }
    }
    failure
}

pub(crate) fn turn_item(input: &Value) -> Value {
    let driver = input["driver"].as_str().unwrap_or("");
    let native = format!(
        "terminal-failure:{}",
        input["providerTurnId"].as_str().unwrap_or("")
    );
    let mut item = json!({
        "id":format!("turn-item:provider:{}:native-item:{}",crate::webhook::encode_component(driver),crate::webhook::encode_component(&native)),
        "threadId":input["threadId"],"runId":input["runId"],"nodeId":input["nodeId"],"providerThreadId":input["providerThreadId"],"providerTurnId":input["providerTurnId"],
        "nativeItemRef":null,"parentItemId":null,"ordinal":input["itemOrdinal"],"status":"failed",
        "title":if input["failure"]["class"]=="usage_limit" {"Usage limit reached"}else{"Provider error"},
        "startedAt":input.get("retryStartedAt").unwrap_or(&input["occurredAt"]),"completedAt":input["occurredAt"],"updatedAt":input["occurredAt"],"type":"error","failure":input["failure"]
    });
    if let Some(retry) = input.get("retry") {
        item["retry"] = retry.clone();
    }
    item
}

pub(crate) fn persist(
    db: &rusqlite::Connection,
    tid: &str,
    thread: &Value,
    run: &Value,
    failure: &Value,
    at: &str,
) -> Result<(), String> {
    use crate::events::get;
    use rusqlite::OptionalExtension;
    let Some(provider_thread) = run["providerThreadId"].as_str() else {
        return Ok(());
    };
    let provider = get(db, "providerThreads", provider_thread)?;
    let driver = provider["driver"]
        .as_str()
        .ok_or("Provider thread driver missing")?;
    let attempt_id = run["activeAttemptId"].as_str().unwrap_or("");
    let attempt = get(db, "attempts", attempt_id).unwrap_or_default();
    let latest: Option<String> = db.query_row("SELECT id FROM records WHERE kind='providerTurns' AND thread_id=?1 AND json_extract(body,'$.runAttemptId')=?2 ORDER BY json_extract(body,'$.ordinal') DESC LIMIT 1",rusqlite::params![tid,attempt_id],|row|row.get(0)).optional().map_err(|e|e.to_string())?;
    let turn = latest
        .or_else(|| attempt["providerTurnId"].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            format!(
                "provider-turn:provider:{}:native-turn:{}",
                crate::webhook::encode_component(driver),
                crate::webhook::encode_component(&format!("failed:{attempt_id}"))
            )
        });
    let mut input = json!({"driver":driver,"threadId":tid,"runId":run["id"],"nodeId":run["rootNodeId"],"providerThreadId":provider_thread,"providerTurnId":turn,"itemOrdinal":0,"failure":failure,"occurredAt":at});
    let provisional = turn_item(&input);
    let ordinal = if let Ok(old) = get(db, "turnItems", provisional["id"].as_str().unwrap()) {
        old["ordinal"].clone()
    } else {
        json!(db.query_row("SELECT coalesce(max(json_extract(body,'$.ordinal')),-1)+1 FROM records WHERE kind='turnItems' AND thread_id=?",[tid],|row|row.get::<_,i64>(0)).map_err(|e|e.to_string())?)
    };
    input["itemOrdinal"] = ordinal;
    crate::projection::save_item(db, tid, thread, turn_item(&input))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_native_processes_cannot_terminalize_or_pause_durable_work() {
        use crate::{
            events::{get, put},
            provider::{persist_attempt, ProviderEvent},
            store::State,
        };
        let path = std::env::temp_dir().join(format!(
            "o3-shutdown-receipts-{}",
            crate::store::random_id().unwrap()
        ));
        let state = State::open(&path).unwrap();
        let goal = json!({"objective":"Continue after restart","status":"active","tokensUsed":73,"timeUsedSeconds":17,"tokenBudget":12345});
        {
            let db = state.db.lock().unwrap();
            put(&db,"thread","t","t",&json!({"id":"t","providerInstanceId":"codex","activeProviderThreadId":"p","deletedAt":null})).unwrap();
            put(
                &db,
                "providerThreads",
                "p",
                "t",
                &json!({"id":"p","appThreadId":"t","driver":"codex","goal":goal}),
            )
            .unwrap();
            put(
                &db,
                "runs",
                "r",
                "t",
                &json!({"id":"r","status":"running","ordinal":1,"providerThreadId":"p"}),
            )
            .unwrap();
        }
        state.begin_shutdown();
        for event in [
            ProviderEvent::Failed("Codex process closed".into()),
            ProviderEvent::Interrupted,
            ProviderEvent::Codex(
                json!({"method":"thread/goal/updated","params":{"threadId":"native-root","goal":{"objective":"Continue after restart","status":"paused"}}}),
            ),
        ] {
            persist_attempt(&state, "t", "r", None, event).unwrap();
        }
        let db = state.db.lock().unwrap();
        assert_eq!(get(&db, "runs", "r").unwrap()["status"], "running");
        assert_eq!(get(&db, "providerThreads", "p").unwrap()["goal"], goal);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM records WHERE kind='turnItems'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn executed_upstream_failure_and_card_goldens() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/provider-failures.json")).unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                make(&case["input"]),
                case["expected"],
                "failure case {index}"
            );
        }
        for (index, case) in fixture["cards"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                turn_item(&case["input"]),
                case["expected"],
                "card case {index}"
            );
        }
    }

    #[test]
    fn stale_and_duplicate_terminals_do_not_publish_or_replace_failure_cards() {
        use crate::{
            events::{get, put},
            provider::{persist_attempt, ProviderEvent},
            store::State,
        };
        let path = std::env::temp_dir().join(format!(
            "o3-failure-fencing-{}",
            crate::store::random_id().unwrap()
        ));
        let state = State::open(&path).unwrap();
        {
            let db = state.db.lock().unwrap();
            put(&db,"thread","t","t",&json!({"id":"t","providerInstanceId":"codex","activeProviderThreadId":"p","deletedAt":null})).unwrap();
            put(
                &db,
                "providerThreads",
                "p",
                "t",
                &json!({"id":"p","appThreadId":"t","driver":"codex","providerInstanceId":"codex"}),
            )
            .unwrap();
            put(&db,"runs","r","t",&json!({"id":"r","status":"running","ordinal":1,"providerThreadId":"p","activeAttemptId":"new","rootNodeId":"node-new"})).unwrap();
            put(
                &db,
                "attempts",
                "new",
                "t",
                &json!({"id":"new","runId":"r","status":"running","providerTurnId":null}),
            )
            .unwrap();
        }
        persist_attempt(
            &state,
            "t",
            "r",
            Some("old"),
            ProviderEvent::FailedWithDetails(json!({"message":"Stale failure"})),
        )
        .unwrap();
        assert_eq!(
            state
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM records WHERE kind='turnItems'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        persist_attempt(
            &state,
            "t",
            "r",
            Some("new"),
            ProviderEvent::FailedWithDetails(
                json!({"message":"Current failure","code":"native-code","class":"transport_error"}),
            ),
        )
        .unwrap();
        persist_attempt(
            &state,
            "t",
            "r",
            Some("new"),
            ProviderEvent::FailedWithDetails(json!({"message":"Duplicate replacement"})),
        )
        .unwrap();
        let db = state.db.lock().unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM records WHERE kind='turnItems' AND json_extract(body,'$.type')='error'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(get(&db, "runs", "r").unwrap()["error"], "Current failure");
        let card:String=db.query_row("SELECT body FROM records WHERE kind='turnItems' AND json_extract(body,'$.type')='error'",[],|row|row.get(0)).unwrap();
        let card: Value = serde_json::from_str(&card).unwrap();
        assert_eq!(card["failure"]["code"], "native-code");
        assert_eq!(card["nodeId"], "node-new");
    }
}
