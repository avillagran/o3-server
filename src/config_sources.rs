//! Subscription-owned quota polling; no timer or network work without an opted-in reader.
use crate::store::{iso, now_ms, State};
use serde_json::{json, Value};
use std::{process::Stdio, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct Sources {
    receiver: tokio::sync::mpsc::Receiver<Value>,
    task: tokio::task::JoinHandle<()>,
    last: std::collections::BTreeMap<String, Value>,
    replay: std::collections::VecDeque<Value>,
}
impl Drop for Sources {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Sources {
    pub fn next(&mut self) -> Option<Value> {
        if let Ok(event) = self.receiver.try_recv() {
            self.replay.retain(|old| old["type"] != event["type"]);
            self.last.insert(
                event["type"].as_str().unwrap_or("").to_owned(),
                event.clone(),
            );
            return Some(event);
        }
        self.replay.pop_front()
    }
    pub fn replay(&mut self) {
        self.replay = self.last.values().cloned().collect();
    }
    pub fn start(
        state: Arc<State>,
        wake: Arc<tokio::sync::Notify>,
        usage: bool,
        themes: bool,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let task = tokio::spawn(async move {
            let mut changes = state.changed.subscribe();
            let mut last_themes = Value::Null;
            let mut next_usage = tokio::time::Instant::now();
            let mut last_settings = Value::Null;
            loop {
                if themes {
                    let current = read_themes(&state);
                    if current != last_themes {
                        last_themes = current.clone();
                        if sender.send(json!({"version":1,"type":"environmentThemesUpdated","payload":{"themes":current}})).await.is_err() { return; }
                        wake.notify_one();
                    }
                }
                let settings = crate::settings::read(&state).unwrap_or(json!({}));
                if usage
                    && (tokio::time::Instant::now() >= next_usage
                        || settings["usageLimitSources"] != last_settings["usageLimitSources"]
                        || settings["providerInstances"] != last_settings["providerInstances"])
                {
                    let provider = state.provider.lock().unwrap().clone();
                    if let Some(provider) = provider {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(20),
                            provider.poll_usage(),
                        )
                        .await;
                        state.notify();
                    }
                    let sources = refresh(&settings).await;
                    if crate::settings::read(&state).ok().is_some_and(|current| {
                        ["usageLimitSources", "providerInstances"]
                            .iter()
                            .any(|key| current[*key] != settings[*key])
                    }) {
                        continue;
                    }
                    if sender.send(json!({"version":1,"type":"usageLimitSourcesUpdated","payload":{"sources":sources}})).await.is_err() { break; }
                    wake.notify_one();
                }
                let interval = settings["backgroundActivity"]["overrides"]
                    ["providerHealthRefreshInterval"]
                    .as_u64()
                    .or_else(|| settings["providerHealthRefreshInterval"].as_u64())
                    .unwrap_or(300_000);
                next_usage =
                    if next_usage <= tokio::time::Instant::now() || settings != last_settings {
                        tokio::time::Instant::now()
                            + std::time::Duration::from_millis(if interval == 0 {
                                u64::MAX / 2
                            } else {
                                interval.max(1000)
                            })
                    } else {
                        next_usage
                    };
                last_settings = settings.clone();
                // Disabled periodic refresh still reacts to configuration changes.
                let timer = tokio::time::sleep_until(next_usage);
                tokio::pin!(timer);
                loop {
                    tokio::select! {
                        _ = &mut timer => break,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)), if themes => break,
                        r = changes.changed() => {
                            if r.is_err() { return; }
                            if crate::settings::read(&state).ok().is_some_and(|s| ["usageLimitSources","providerInstances","backgroundActivity"].iter().any(|key| s[*key] != settings[*key])) { break; }
                        }
                    }
                }
            }
        });
        Self {
            receiver,
            task,
            last: Default::default(),
            replay: Default::default(),
        }
    }
}
async fn management(config: &Value, path: &str, body: Option<Value>) -> Result<Value, ()> {
    let url = format!(
        "{}/v0/management/{path}",
        config["url"].as_str().ok_or(())?.trim_end_matches('/')
    );
    let key = config["managementKey"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(())?;
    request(&url, key, &[], body).await
}
pub(crate) async fn request(
    url: &str,
    key: &str,
    headers: &[&str],
    body: Option<Value>,
) -> Result<Value, ()> {
    let mut command = tokio::process::Command::new("curl");
    command
        .args([
            "--disable",
            "--proto",
            "=http,https",
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "15",
            "--max-filesize",
            "1048576",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ())?;
    let mut input = format!(
        "url = {}\nheader = {}\n",
        json!(url),
        json!(format!("Authorization: Bearer {key}"))
    );
    for header in headers {
        input.push_str(&format!("header = {}\n", json!(header)));
    }
    if let Some(body) = body {
        input.push_str(&format!(
            "header = \"Content-Type: application/json\"\ndata = {}\n",
            json!(body.to_string())
        ));
    }
    child
        .stdin
        .take()
        .ok_or(())?
        .write_all(input.as_bytes())
        .await
        .map_err(|_| ())?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or(())?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ())?;
    if bytes.len() > 1_048_576 || !child.wait().await.map_err(|_| ())?.success() {
        return Err(());
    }
    serde_json::from_slice(&bytes).map_err(|_| ())
}
async fn account(config: &Value, account: &Value) -> Value {
    let codex = account["provider"] == "codex";
    let checked = iso(now_ms());
    let mut out = json!({"id":account["id"],"driver":if codex {"codex"} else {"claudeAgent"},"email":account["email"],"workspaceId":account["id_token"]["chatgpt_account_id"]});
    let header = if codex {
        json!({"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","OpenAI-Beta":"codex-1","Originator":"Codex Desktop","Chatgpt-Account-Id":account["id_token"]["chatgpt_account_id"]})
    } else {
        json!({"Authorization":"Bearer $TOKEN$","anthropic-beta":"oauth-2025-04-20","User-Agent":"claude-code/2.1.273"})
    };
    let response = management(config,"api-call",Some(json!({"auth_index":account["auth_index"],"method":"GET","url":if codex {"https://chatgpt.com/backend-api/wham/usage"} else {"https://api.anthropic.com/api/oauth/usage"},"header":header}))).await;
    let usage = response
        .ok()
        .filter(|r| {
            r["status_code"]
                .as_u64()
                .is_some_and(|s| (200..300).contains(&s))
        })
        .and_then(|r| serde_json::from_str::<Value>(r["body"].as_str()?).ok());
    let Some(usage) = usage else {
        out["usageLimits"] = json!({"checkedAt":checked,"windows":[],"unavailable":{"reason":"probeFailed","message":"The hub could not read this account's usage."}});
        return out;
    };
    let mut windows = Vec::new();
    for (key, id, label, kind) in if codex {
        [
            ("primary_window", "primary", "Session", "session"),
            ("secondary_window", "secondary", "Weekly", "weekly"),
        ]
    } else {
        [
            ("five_hour", "five_hour", "Session", "session"),
            ("seven_day", "seven_day", "Weekly", "weekly"),
        ]
    } {
        let w = if codex {
            &usage["rate_limit"][key]
        } else {
            &usage[key]
        };
        let used = w[if codex { "used_percent" } else { "utilization" }].as_f64();
        if let Some(used) = used.filter(|v| v.is_finite()) {
            let mut window =
                json!({"id":id,"label":label,"kind":kind,"usedPercent":used.clamp(0.,100.)});
            if codex {
                if let Some(reset) = w["reset_at"].as_i64() {
                    window["resetsAt"] = json!(iso(reset.saturating_mul(1000)));
                }
                if let Some(seconds) = w["limit_window_seconds"].as_u64() {
                    window["windowDurationMins"] = json!(seconds / 60);
                }
            } else if w["resets_at"].is_string() {
                window["resetsAt"] = w["resets_at"].clone();
            }
            windows.push(window);
        }
    }
    if !codex {
        for limit in usage["limits"].as_array().into_iter().flatten() {
            if limit["kind"] != "weekly_scoped" {
                continue;
            }
            if let (Some(name), Some(used)) = (
                limit["scope"]["model"]["display_name"].as_str(),
                limit["percent"].as_f64(),
            ) {
                let id = format!(
                    "seven_day_{}",
                    name.to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect::<String>()
                );
                windows.push(json!({"id":id,"label":format!("Weekly · {name}"),"kind":"weekly","usedPercent":used.clamp(0.,100.),"resetsAt":limit["resets_at"],"windowDurationMins":10080}));
            }
        }
    }
    out["plan"] = if codex {
        usage["plan_type"].clone()
    } else {
        json!("Claude Subscription")
    };
    out["usageLimits"] = json!({"checkedAt":checked,"windows":windows});
    if codex {
        if let Ok(response) = management(config,"api-call",Some(json!({"auth_index":account["auth_index"],"method":"GET","url":"https://chatgpt.com/backend-api/wham/rate-limit-reset-credits","header":{"Authorization":"Bearer $TOKEN$","Content-Type":"application/json","OpenAI-Beta":"codex-1","Originator":"Codex Desktop"}}))).await {
            if response["status_code"] == 200 {
                if let Some(body) = response["body"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                    let now = time::OffsetDateTime::now_utc();
                    let mut credits:Vec<_> = body["credits"].as_array().into_iter().flatten().filter(|c| c["status"]=="available" && c["reset_type"]=="codex_rate_limits" && c["expires_at"].as_str().and_then(|s|time::OffsetDateTime::parse(s,&time::format_description::well_known::Rfc3339).ok()).is_some_and(|t| t>now)).collect();
                    credits.sort_by_key(|c|c["expires_at"].as_str());
                    out["usageLimits"]["resetCredits"] = json!({"availableCount":credits.len()});
                    if let Some(first) = credits.first() { out["usageLimits"]["resetCredits"]["nextExpiresAt"] = first["expires_at"].clone(); }
                }
            }
        }
    }

    out
}
async fn refresh(settings: &Value) -> Vec<Value> {
    let mut sources = Vec::new();
    for (id, config) in settings["usageLimitSources"]
        .as_object()
        .into_iter()
        .flatten()
        .take(512)
    {
        if config["enabled"] == false {
            continue;
        }
        let label = config["label"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                url::Url::parse(config["url"].as_str().unwrap_or(""))
                    .ok()
                    .and_then(|url| {
                        url.host_str().map(|host| {
                            if let Some(port) = url.port() {
                                format!("{host}:{port}")
                            } else {
                                host.to_owned()
                            }
                        })
                    })
                    .unwrap_or_else(|| id.to_owned())
            });
        let mut source = json!({"id":id,"kind":"cliproxy","label":label,"checkedAt":iso(now_ms()),"accounts":[]});
        match management(config, "auth-files", None).await {
            Ok(files) => {
                let mut accounts = Vec::new();
                for a in files["files"].as_array().into_iter().flatten().take(4096) {
                    if a["disabled"] != true
                        && matches!(a["provider"].as_str(), Some("codex" | "claude"))
                    {
                        accounts.push(account(config, a).await);
                    }
                }
                source["accounts"] = json!(accounts);
            }
            Err(()) => source["error"] = json!("Could not read usage source."),
        }
        sources.push(source);
    }
    sources
}

fn read_themes(state: &State) -> Value {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let mut paths: Vec<_> = std::fs::read_dir(state.dir.join("themes"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut out = Vec::new();
    let mut total = 0;
    for path in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .take(32)
    {
        let id = path.file_stem().unwrap_or_default().to_string_lossy();
        if id.is_empty()
            || id.len() > 48
            || !id.as_bytes()[0].is_ascii_alphanumeric()
            || matches!(id.as_ref(), "system" | "light" | "dark")
            || !id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            continue;
        }
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        else {
            continue;
        };
        if !file
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() <= 32768)
        {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(32769).read_to_end(&mut bytes).is_err() {
            continue;
        }
        total += bytes.len();
        if total > 196608 {
            break;
        }
        let Ok(mut theme) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if !theme["name"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 48)
            || !matches!(theme["appearance"].as_str(), Some("light" | "dark"))
        {
            continue;
        }
        if !(theme["canvas"].is_string() && theme["accent"].is_string())
            && theme["colors"].as_object().is_none_or(|o| o.is_empty())
        {
            continue;
        }
        theme.as_object_mut().unwrap().retain(|k, _| {
            matches!(
                k.as_str(),
                "version" | "name" | "appearance" | "canvas" | "accent" | "colors" | "variants"
            )
        });
        theme["id"] = json!(id);
        out.push(theme);
    }
    json!(out)
}
