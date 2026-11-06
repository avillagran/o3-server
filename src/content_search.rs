//! Nightly content search: bounded work off the async runtime, no persistent index or subprocess.
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

static SEARCH_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

const MAX_FILE: u64 = 1024 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

fn error(p: &Value, failure: &str, detail: impl ToString) -> Value {
    json!({"_tag":"ProjectSearchContentsError","cwd":p["cwd"],"queryLength":p["query"].as_str().map(|q|q.encode_utf16().count()),"limit":p["limit"],"failure":failure,"message":format!("Failed to search workspace contents: {}",detail.to_string())})
}

pub async fn search(p: Value) -> Result<Value, Value> {
    let permit = SEARCH_SLOTS
        .acquire()
        .await
        .map_err(|e| error(&p, "search_index_search_failed", e))?;
    let on_failure = p.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        search_blocking(&p)
    })
    .await
    .map_err(|e| error(&on_failure, "search_index_search_failed", e))?
}

fn search_blocking(p: &Value) -> Result<Value, Value> {
    let invalid = |detail: &str| error(p, "search_index_search_failed", detail);
    let query = p["query"]
        .as_str()
        .filter(|q| !q.is_empty() && q.encode_utf16().count() <= 256)
        .ok_or_else(|| invalid("query must contain 1–256 characters"))?;
    let limit = p["limit"]
        .as_u64()
        .filter(|n| (1..=500).contains(n))
        .ok_or_else(|| invalid("limit must be between 1 and 500"))? as usize;
    for key in ["caseSensitive", "wholeWord", "useRegex"] {
        if !p[key].is_boolean() {
            return Err(invalid("search options must be boolean"));
        }
    }
    let raw = p["cwd"].as_str().unwrap_or("").trim();
    let root = crate::paths::normalize(&crate::paths::expand_home(raw));
    if raw.is_empty() || !root.is_absolute() {
        return Err(error(
            p,
            "workspace_root_not_found",
            "absolute cwd required",
        ));
    }
    match std::fs::metadata(&root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(error(
                p,
                "workspace_root_not_directory",
                "cwd is not a directory",
            ))
        }
        Err(e) => {
            return Err(error(
                p,
                if e.kind() == std::io::ErrorKind::NotFound {
                    "workspace_root_not_found"
                } else {
                    "workspace_root_stat_failed"
                },
                e,
            ))
        }
    }
    let compile = |pattern: &str| {
        RegexBuilder::new(pattern)
            .case_insensitive(p["caseSensitive"] == false)
            .size_limit(2 * 1024 * 1024)
            .build()
    };
    let mut fallback = None;
    let pattern = if p["useRegex"] == true {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let re = match compile(&pattern) {
        Ok(re) => re,
        Err(e) => {
            fallback = Some(e.to_string());
            compile(&regex::escape(query)).map_err(|e| invalid(&e.to_string()))?
        }
    };
    let word = Regex::new(r"[\p{L}\p{M}\p{N}_]").unwrap();
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut bytes = 0;
    // Match the native finder: include dotfiles, apply ignore rules, never follow symlinks.
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .filter_entry(|e| e.depth() == 0 || e.file_name() != ".git")
        .sort_by_file_path(Path::cmp)
        .build();
    'walk: for entry in walker {
        if Instant::now() >= deadline {
            truncated = true;
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                truncated = true;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let Ok(file) = opts.open(entry.path()) else {
            truncated = true;
            continue;
        };
        let Ok(meta) = file.metadata() else {
            truncated = true;
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_FILE {
            truncated = true;
            continue;
        }
        let mut content = Vec::new();
        if file.take(MAX_FILE + 1).read_to_end(&mut content).is_err() {
            truncated = true;
            continue;
        }
        if content.len() as u64 > MAX_FILE {
            truncated = true;
            continue;
        }
        if content.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&content) else {
            continue;
        };
        let rel = entry
            .path()
            .strip_prefix(&root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        let mut file_matches = 0;
        for (line_idx, line) in text.lines().enumerate() {
            if Instant::now() >= deadline {
                truncated = true;
                break 'walk;
            }
            if line.len() > MAX_LINE {
                truncated = true;
                continue;
            }
            let mut ranges = Vec::new();
            let (mut offset, mut units) = (0, 0);
            for m in re.find_iter(line) {
                if m.is_empty() {
                    continue;
                }
                if p["wholeWord"] == true {
                    let is_word = |c: Option<char>| {
                        c.is_some_and(|c| word.is_match(c.encode_utf8(&mut [0; 4])))
                    };
                    if (is_word(line[..m.start()].chars().next_back())
                        && is_word(m.as_str().chars().next()))
                        || (is_word(line[m.end()..].chars().next())
                            && is_word(m.as_str().chars().next_back()))
                    {
                        continue;
                    }
                }
                units += line[offset..m.start()].encode_utf16().count();
                let start = units;
                units += m.as_str().encode_utf16().count();
                offset = m.end();
                ranges.push(json!({"start":start,"end":units}));
            }
            if ranges.is_empty() {
                continue;
            }
            if matches.len() == limit {
                truncated = true;
                break 'walk;
            }
            let item =
                json!({"path":rel,"lineNumber":line_idx+1,"lineContent":line,"matchRanges":ranges});
            bytes += item.to_string().len();
            if bytes > MAX_RESPONSE {
                truncated = true;
                break 'walk;
            }
            matches.push(item);
            file_matches += 1;
            if file_matches == 100 {
                break;
            }
        }
    }
    let mut result = json!({"matches":matches,"truncated":truncated});
    if let Some(e) = fallback {
        result["regexFallbackError"] = json!(e);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn typed_errors_and_contract_validation() {
        let mut p = json!({"cwd":"/does-not-exist-o3-content-search","query":"x","limit":1,"caseSensitive":true,"wholeWord":false,"useRegex":false});
        let err = search(p.clone()).await.unwrap_err();
        assert_eq!(err["_tag"], "ProjectSearchContentsError");
        assert_eq!(err["failure"], "workspace_root_not_found");
        for (key, value) in [
            ("limit", json!(0)),
            ("limit", json!(501)),
            ("query", json!("")),
            ("query", json!("x".repeat(257))),
            ("caseSensitive", Value::Null),
        ] {
            let old = p[key].clone();
            p[key] = value;
            let err = search(p.clone()).await.unwrap_err();
            assert_eq!(err["failure"], "search_index_search_failed");
            p[key] = old;
        }
    }
}
