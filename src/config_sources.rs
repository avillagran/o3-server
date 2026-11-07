//! Subscription-owned quota polling; no timer or network work without an opted-in reader.
use crate::store::{iso, now_ms, State};
use serde_json::{json, Value};
use std::{process::Stdio, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
pub(crate) struct UsageState {
    refresh: tokio::sync::Mutex<()>,
    snapshot: std::sync::Mutex<Vec<Value>>,
    version: std::sync::atomic::AtomicU64,
    private_generation: std::sync::atomic::AtomicU64,
}

pub(crate) fn private_settings_changed(state: &State) {
    state
        .streams
        .ops
        .usage_sources
        .private_generation
        .fetch_add(1, std::sync::atomic::Ordering::Release);
}

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
            let mut last_usage_version = 0;
            let mut last_private_generation = 0;
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
                let private_generation = state
                    .streams
                    .ops
                    .usage_sources
                    .private_generation
                    .load(std::sync::atomic::Ordering::Acquire);
                if usage
                    && (private_generation != last_private_generation
                        || tokio::time::Instant::now() >= next_usage
                        || settings["usageLimitSources"] != last_settings["usageLimitSources"]
                        || settings["providerInstances"] != last_settings["providerInstances"]
                        || state
                            .streams
                            .ops
                            .usage_sources
                            .version
                            .load(std::sync::atomic::Ordering::Acquire)
                            != last_usage_version)
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
                    let version = state
                        .streams
                        .ops
                        .usage_sources
                        .version
                        .load(std::sync::atomic::Ordering::Acquire);
                    if version == last_usage_version
                        || settings != last_settings
                        || private_generation != last_private_generation
                    {
                        refresh(&state).await;
                    }
                    let sources = {
                        let current = state.streams.ops.usage_sources.snapshot.lock().unwrap();
                        last_usage_version = state
                            .streams
                            .ops
                            .usage_sources
                            .version
                            .load(std::sync::atomic::Ordering::Acquire);
                        current.clone()
                    };
                    if state
                        .streams
                        .ops
                        .usage_sources
                        .private_generation
                        .load(std::sync::atomic::Ordering::Acquire)
                        != private_generation
                        || crate::settings::read(&state).ok().is_some_and(|current| {
                            ["usageLimitSources", "providerInstances"]
                                .iter()
                                .any(|key| current[*key] != settings[*key])
                        })
                    {
                        continue;
                    }
                    last_private_generation = private_generation;
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
                            if state.streams.ops.usage_sources.private_generation.load(std::sync::atomic::Ordering::Acquire) != last_private_generation || state.streams.ops.usage_sources.version.load(std::sync::atomic::Ordering::Acquire) != last_usage_version || crate::settings::read(&state).ok().is_some_and(|s| ["usageLimitSources","providerInstances","backgroundActivity"].iter().any(|key| s[*key] != settings[*key])) { break; }
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
    let checked = iso(now_ms());
    let base = config["url"].as_str().unwrap_or("");
    let key = config["managementKey"].as_str().unwrap_or("");
    let usage = crate::cliproxy::usage(base, key, account).await.ok();
    let available = if account["provider"] == "codex" && usage.is_some() {
        crate::cliproxy::credits(base, key, account, now_ms())
            .await
            .ok()
    } else {
        None
    };
    crate::cliproxy_projection::project(account, usage.as_ref(), available.as_deref(), &checked)
}

pub(crate) async fn refresh(state: &State) -> Vec<Value> {
    let service = &state.streams.ops.usage_sources;
    let _refresh = service.refresh.lock().await;
    let settings = crate::settings::read_materialized(state)
        .ok()
        .unwrap_or_else(|| json!({}));
    let sources = read_sources(state, &settings).await;
    let mut previous = service.snapshot.lock().unwrap();
    if *previous != sources {
        *previous = sources.clone();
        service
            .version
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        state.notify();
    }
    sources
}

async fn read_sources(state: &State, settings: &Value) -> Vec<Value> {
    let mut sources = Vec::new();
    for (id, config) in settings["usageLimitSources"]
        .as_object()
        .into_iter()
        .flatten()
        .take(512)
    {
        let Ok(config) = crate::managed_settings_codec::decode_usage_config(config.clone()) else {
            continue;
        };
        if config["enabled"] == false {
            continue;
        }
        let config = crate::settings_secrets::resolve_fields(
            state.secret_resolver().as_ref(),
            &settings["secretProviders"],
            "usage",
            None,
            &config,
        )
        .await;
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
                    .unwrap_or_else(|| {
                        let fallback = crate::native_goal::trim(id);
                        if fallback.is_empty() {
                            "Usage limit source".into()
                        } else {
                            fallback.into()
                        }
                    })
            });
        let mut source = json!({"id":id,"kind":"cliproxy","label":label,"checkedAt":iso(now_ms()),"accounts":[]});
        if config["managementKey"]
            .as_str()
            .is_none_or(|key| key.is_empty())
        {
            source["error"] = json!(if config["managementKey"].is_object() {
                "Could not read the management key from its secret provider."
            } else {
                "No management key configured."
            });
            sources.push(source);
            continue;
        }
        match crate::cliproxy::list_accounts(
            config["url"].as_str().unwrap_or(""),
            config["managementKey"].as_str().unwrap_or(""),
        )
        .await
        {
            Ok(files) => {
                use futures_util::StreamExt;
                let files = files
                    .into_iter()
                    .filter(|file: &Value| {
                        file["disabled"] != true
                            && matches!(file["provider"].as_str(), Some("codex" | "claude"))
                    })
                    .take(4096)
                    .collect::<Vec<_>>();
                let accounts = futures_util::stream::iter(files)
                    .map(|file: Value| {
                        let config = config.clone();
                        async move { account(&config, &file).await }
                    })
                    .buffered(4)
                    .collect::<Vec<_>>()
                    .await;
                source["accounts"] = json!(accounts);
            }
            Err(failure) => source["error"] = json!(failure.detail()),
        }
        sources.push(source);
    }
    sources
}

pub(crate) async fn consume_reset_credit(state: &State, input: &Value) -> Result<Value, Value> {
    let input = crate::cliproxy::decode_reset_input(input)
        .map_err(|_| json!({"_tag":"RpcInvalidPayload","message":"Invalid reset-credit input."}))?;
    if let Some(instance_id) = input["instanceId"].as_str() {
        let settings = crate::settings::read(state).unwrap_or_else(|_| json!({}));
        let instance = settings["providerInstances"].get(instance_id);
        let detail = match instance {
            None => "Provider instance not found.",
            Some(instance) if instance["enabled"] == false => "This provider is disabled.",
            Some(_) => "This provider does not bank reset credits.",
        };
        return Err(
            json!({"_tag":"ProviderSetupError","instanceId":instance_id,"operation":"consume-reset-credit","detail":detail}),
        );
    }
    let error = |detail: &str| json!({"_tag":"UsageLimitSourceError","detail":detail});
    let service = &state.streams.ops.usage_sources;
    // A stale concurrent refresh must not resurrect credit state after redemption.
    let _refresh = service.refresh.lock().await;
    let settings = crate::settings::read_materialized(state)
        .map_err(|_| error("Could not read hub settings."))?;
    let source_id = input["sourceId"].as_str().unwrap();
    let raw = settings["usageLimitSources"]
        .get(source_id)
        .ok_or_else(|| error("The usage limit source is missing or disabled."))?;
    let config = crate::managed_settings_codec::decode_usage_config(raw.clone())
        .map_err(|_| error("Could not read hub settings."))?;
    if config["enabled"] == false {
        return Err(error("The usage limit source is missing or disabled."));
    }
    let resolved = crate::settings_secrets::resolve_fields(
        state.secret_resolver().as_ref(),
        &settings["secretProviders"],
        "usage",
        None,
        &config,
    )
    .await;
    let key = resolved["managementKey"].as_str().unwrap_or("");
    if key.is_empty() {
        return Err(error(if config["managementKey"].is_string() {
            "No management key configured."
        } else {
            "Could not read the management key from its secret provider."
        }));
    }
    let result = crate::cliproxy::consume(
        resolved["url"].as_str().unwrap(),
        key,
        input["accountId"].as_str().unwrap(),
        input["creditId"].as_str().unwrap(),
    )
    .await
    .map_err(|failure| error(&failure.detail()))?;
    let mut selected = settings.clone();
    selected["usageLimitSources"] = json!({source_id:raw});
    let mut refreshed = read_sources(state, &selected).await;
    if let Some(snapshot) = refreshed.pop() {
        let mut previous = service.snapshot.lock().unwrap();
        let mut changed = false;
        for source in previous
            .iter_mut()
            .filter(|source| source["id"] == source_id)
        {
            if *source != snapshot {
                *source = snapshot.clone();
                changed = true;
            }
        }
        if changed {
            service
                .version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            state.notify();
        }
    }
    Ok(result)
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

#[cfg(test)]
#[path = "../tests/support/config_sources_secrets.rs"]
mod secret_tests;
