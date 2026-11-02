//! On-demand scan of canonical turns. No transcript reads, cache, or write-path work.
use crate::store::{iso, now_ms, State};
use chrono::{DateTime, NaiveDate};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn summary(s: &State, p: &Value, since: &str, until: &str, zone: &str) -> Result<Value, String> {
    let started = std::time::Instant::now();
    let tz: chrono_tz::Tz = zone.parse().map_err(|_| "invalid timeZone")?;
    let lo = NaiveDate::parse_from_str(since, "%Y-%m-%d").map_err(|_| "invalid sinceDay")?;
    let hi = NaiveDate::parse_from_str(until, "%Y-%m-%d").map_err(|_| "invalid untilDay")?;
    if hi < lo || (hi-lo).num_days() > 3660 { return Err("usage window must be at most 3661 days".into()); }
    let bound = |key: &str| -> Result<Option<DateTime<chrono::FixedOffset>>, String> {
        p.get(key).map(|v| DateTime::parse_from_rfc3339(v.as_str().unwrap_or("")).map_err(|_| format!("invalid {key}"))).transpose()
    };
    let (start, end) = (bound("sinceTime")?, bound("untilTime")?);
    let rates = crate::usage_rates::load(s).unwrap_or(Value::Null);
    let db = s.reader.lock().unwrap();
    // Select only accounting and small identity fields, never turn content. The primary
    // key's kind prefix restricts the scan to turns even on large message histories.
    let mut q = db.prepare("SELECT json_object('usage',json_extract(t.body,'$.tokenUsage'),'at',coalesce(json_extract(t.body,'$.tokenUsage.updatedAt'),json_extract(t.body,'$.completedAt'),json_extract(t.body,'$.startedAt')),'driver',coalesce(json_extract(t.body,'$.driver'),json_extract(pt.body,'$.driver')),'model',coalesce(json_extract(t.body,'$.model'),json_extract(ps.body,'$.model'),'unknown'),'session',coalesce(json_extract(t.body,'$.providerThreadId'),t.thread_id)) FROM records t LEFT JOIN records pt ON pt.kind='providerThreads' AND pt.id=json_extract(t.body,'$.providerThreadId') LEFT JOIN records ps ON ps.kind='providerSessions' AND ps.id=json_extract(pt.body,'$.providerSessionId') WHERE t.kind='providerTurns' AND json_type(t.body,'$.tokenUsage')='object'").map_err(|e|e.to_string())?;
    let mut buckets: BTreeMap<(String,String,String), Value> = BTreeMap::new();
    let mut sessions: BTreeSet<(String,String,String,String)> = BTreeSet::new();
    let mut providers = BTreeSet::new();
    for row in q.query_map([], |r|r.get::<_,String>(0)).map_err(|e|e.to_string())? {
        let v: Value = serde_json::from_str(&row.map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        let Ok(at) = DateTime::parse_from_rfc3339(v["at"].as_str().unwrap_or("")) else { continue; };
        let day = at.with_timezone(&tz).date_naive();
        if day < lo || day > hi || start.is_some_and(|s|at<s) || end.is_some_and(|e|at>=e) { continue; }
        let provider = match v["driver"].as_str().unwrap_or("unknown") { "claudeAgent" => "claude", v => v }.to_owned();
        providers.insert(provider.clone());
        let key = (day.to_string(), provider, v["model"].as_str().unwrap_or("unknown").to_owned());
        let b = buckets.entry(key.clone()).or_insert_with(||json!({"day":key.0,"provider":key.1,"model":key.2,"totals":{"uncachedInputTokens":0,"cachedInputTokens":0,"cacheCreationTokens":0,"outputTokens":0,"reasoningTokens":0},"costUsd":0.0,"cacheSavingsUsd":0.0,"records":0,"unpricedRecords":0,"sessions":0}));
        let u = &v["usage"];
        let num = |k:&str|u[k].as_u64().unwrap_or(0);
        let cached = num("cachedInputTokens");
        let reasoning = num("reasoningOutputTokens");
        // Codex input/output include their cached/reasoning subsets. Claude's stored
        // input already includes cache creation, but excludes cache reads.
        let created = num("cacheCreationTokens");
        let input = if key.1 == "codex" {num("inputTokens").saturating_sub(cached)} else {num("inputTokens").saturating_sub(created)};
        for (k,n) in [("uncachedInputTokens",input),("cachedInputTokens",cached),("cacheCreationTokens",created),("outputTokens",num("outputTokens").saturating_sub(reasoning)),("reasoningTokens",reasoning)] {
            b["totals"][k] = json!(b["totals"][k].as_u64().unwrap().saturating_add(n));
        }
        b["records"] = json!(b["records"].as_u64().unwrap()+1);
        let rate = &rates["document"][&key.2];
        let price = |k: &str| rate[k].as_f64().filter(|n|n.is_finite() && *n>=0.0);
        let estimate = (|| Some(input as f64 * price("input_cost_per_token")? + num("outputTokens") as f64 * price("output_cost_per_token")? + if cached>0 {cached as f64 * price("cache_read_input_token_cost")?} else {0.0} + if created>0 {created as f64 * price("cache_creation_input_token_cost")?} else {0.0}))();
        if let Some(cost) = u["cost"].as_f64().filter(|n| n.is_finite() && *n>=0.0).or(estimate) {
            b["costUsd"] = json!(b["costUsd"].as_f64().unwrap()+cost);
        } else { b["unpricedRecords"] = json!(b["unpricedRecords"].as_u64().unwrap()+1); }
        if sessions.insert((key.0,key.1,key.2,v["session"].as_str().unwrap_or("").into())) { b["sessions"] = json!(b["sessions"].as_u64().unwrap()+1); }
        if buckets.len()>20000 || sessions.len()>100000 {return Err("usage query exceeds aggregation limit; select a shorter window".into());}
    }
    Ok(json!({"contractVersion":6,"readAt":iso(now_ms()),"timeZone":zone,"sinceDay":since,"untilDay":until,"buckets":buckets.into_values().collect::<Vec<_>>(),"sources":providers.into_iter().map(|provider|json!({"fingerprint":{"provider":provider},"status":"ok","message":"Stored provider turns only; records without accounting are unavailable","refreshing":false})).collect::<Vec<_>>(),"pricing":crate::usage_rates::pricing(s),"scanDurationMs":started.elapsed().as_millis()}))
}
