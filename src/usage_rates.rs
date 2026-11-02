//! On-demand public rate refresh. No network or cache load at startup.
use crate::store::{iso, now_ms, random_id, State};
use serde_json::{json, Value};
use std::io::Read;
const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_AGE_MS: i64 = 24 * 3600 * 1000;

fn load(s: &State) -> Option<Value> {
    let file = std::fs::File::open(s.dir.join("usage-rates.json")).ok()?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn describe(cache: Option<&Value>, failed: bool) -> Value {
    let Some(cache) = cache else {
        return json!({"status":"unavailable","source":"none","fetchedAt":null,"knownModels":0});
    };
    let at = cache["fetchedAtMs"].as_i64().unwrap_or(0);
    let count = cache["document"].as_object().map_or(0, |d| d.len());
    json!({"status":if failed || now_ms().saturating_sub(at)>MAX_AGE_MS {"stale"} else {"fresh"},"source":URL,"fetchedAt":iso(at),"knownModels":count})
}
pub fn pricing(s: &State) -> Value {
    describe(
        load(s).as_ref(),
        s.streams
            .ops
            .usage_rate_failure
            .load(std::sync::atomic::Ordering::Relaxed),
    )
}

async fn fetch() -> Result<Value, String> {
    let url = std::env::var("O3_USAGE_RATES_URL").unwrap_or_else(|_| URL.into());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("rate document too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err("rate document too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let document: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let prices = document
        .as_object()
        .ok_or("rate document must be an object")?
        .iter()
        .filter(|(_, v)| {
            ["input_cost_per_token", "output_cost_per_token"]
                .iter()
                .all(|k| v[*k].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<serde_json::Map<_, _>>();
    if prices.is_empty() {
        return Err("no usable model prices".into());
    }
    Ok(json!({"fetchedAtMs":now_ms(),"document":prices}))
}
pub async fn refresh(s: &State) -> Value {
    let _refresh = s.streams.ops.usage_rate_refresh.lock().await;
    s.streams
        .ops
        .usage_rate_failure
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let old = load(s);
    let Ok(cache) = fetch().await else {
        return describe(old.as_ref(), true);
    };
    let Ok(id) = random_id() else {
        return describe(old.as_ref(), true);
    };
    let path = s.dir.join(format!("usage-rates-{id}.part"));
    let result = (|| -> Result<(), std::io::Error> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        serde_json::to_writer(&mut f, &cache)?;
        f.flush()?;
        f.sync_all()?;
        std::fs::rename(&path, s.dir.join("usage-rates.json"))
    })();
    let _ = std::fs::remove_file(path);
    if result.is_err() {
        describe(old.as_ref(), true)
    } else {
        s.streams
            .ops
            .usage_rate_failure
            .store(false, std::sync::atomic::Ordering::Relaxed);
        describe(Some(&cache), false)
    }
}
