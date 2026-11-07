//! GitHub star operations and the credential/quota policy shared by their reads and writes.
use crate::{
    native_goal::trim,
    store::{now_ms, State},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[cfg(test)]
use crate::checkpoint::recovery_test_support as test_support;

fn valid_host(host: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(||regex::Regex::new(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*(?::[0-9]{1,5})?$").unwrap()).is_match(host)
}

fn environment_token(host: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    if !valid_host(host) {
        return None;
    }
    let names = if host == "github.com" || host.ends_with(".ghe.com") {
        ["GH_TOKEN", "GITHUB_TOKEN"]
    } else if get("GH_HOST").is_some_and(|h| trim(&h).to_lowercase() == host) {
        ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    } else {
        return None;
    };
    names
        .into_iter()
        .filter_map(get)
        .map(|v| trim(&v).to_owned())
        .find(|v| !v.is_empty())
}

fn starred_path(repository: &str) -> Option<String> {
    let segments = repository.split('/').collect::<Vec<_>>();
    if segments.len() != 2 || segments.iter().any(|s| matches!(*s, "" | "." | "..")) {
        return None;
    }
    let encode = |text: &str| {
        let mut out = String::new();
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                out.push(byte as char);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{byte:02X}");
            }
        }
        out
    };
    Some(format!(
        "user/starred/{}/{}",
        encode(segments[0]),
        encode(segments[1])
    ))
}

fn api_root(host: &str) -> String {
    if host == "github.com" {
        "https://api.github.com".into()
    } else if host.ends_with(".ghe.com") {
        format!("https://api.{host}")
    } else {
        format!("https://{host}/api/v3")
    }
}

fn header_number(text: &str) -> Option<f64> {
    let text = trim(text);
    let value = if text.is_empty() {
        0.0
    } else if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()? as f64
    } else if let Some(binary) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        u64::from_str_radix(binary, 2).ok()? as f64
    } else if let Some(octal) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
        u64::from_str_radix(octal, 8).ok()? as f64
    } else {
        text.parse::<f64>().ok()?
    };
    value.is_finite().then_some(value)
}

fn retry_after(text: &str, now: i64) -> Option<i64> {
    let text = trim(text);
    if !text.is_empty() && text.bytes().all(|v| v.is_ascii_digit()) {
        let at = now as f64 + text.parse::<f64>().ok()? * 1000.0;
        return (at.is_finite() && at.fract() == 0.0 && at.abs() <= 9_007_199_254_740_991.0)
            .then_some(at as i64);
    }
    chrono::DateTime::parse_from_rfc2822(text)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(text))
        .ok()
        .map(|v| v.timestamp_millis())
        .filter(|v| *v > now)
}

struct CachedCredential {
    expires: Instant,
    token: Option<String>,
}

enum CredentialFailure {
    SignedOut,
    Missing,
    Transient,
}

#[derive(Default)]
struct Quota {
    limit: f64,
    remaining: f64,
    reset_at: i64,
    retry_at: i64,
    attempts: u32,
    generation: u64,
}

pub(crate) struct GitHub {
    pub(crate) source_credentials: crate::source_credentials_runtime::Services,
    credentials: tokio::sync::Mutex<BTreeMap<String, CachedCredential>>,
    credential_gates: Mutex<BTreeMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    quota: Mutex<BTreeMap<String, Quota>>,
    http_slots: tokio::sync::Semaphore,
    cli_slots: tokio::sync::Semaphore,
    client: OnceLock<reqwest::Client>,
    #[cfg(test)]
    fixture_api_root: Mutex<Option<String>>,
    #[cfg(test)]
    fixture_cli: Mutex<Option<std::path::PathBuf>>,
}

impl Default for GitHub {
    fn default() -> Self {
        Self {
            source_credentials: Default::default(),
            credentials: Default::default(),
            credential_gates: Default::default(),
            quota: Default::default(),
            http_slots: tokio::sync::Semaphore::new(8),
            cli_slots: tokio::sync::Semaphore::new(4),
            client: OnceLock::new(),
            #[cfg(test)]
            fixture_api_root: Mutex::new(None),
            #[cfg(test)]
            fixture_cli: Mutex::new(None),
        }
    }
}

impl GitHub {
    async fn cli_token(
        &self,
        cwd: &Path,
        host: &str,
        account: Option<&str>,
    ) -> Result<String, CredentialFailure> {
        #[cfg(test)]
        let fixture = self.fixture_cli.lock().unwrap().clone();
        #[cfg(test)]
        let binary = fixture.as_deref().unwrap_or_else(|| Path::new("gh"));
        #[cfg(not(test))]
        let binary = Path::new("gh");
        if crate::executable::resolve(binary).is_none() {
            return Err(CredentialFailure::Missing);
        }
        let _permit = self
            .cli_slots
            .acquire()
            .await
            .map_err(|_| CredentialFailure::Transient)?;
        let mut args = vec!["auth", "token", "--hostname", host];
        if let Some(account) = account {
            args.extend(["--user", account]);
        }
        let output = crate::proc::run(
            binary.to_str().ok_or(CredentialFailure::Missing)?,
            cwd,
            &args,
            crate::proc::Opts {
                timeout: Duration::from_secs(10),
                cap: 64 * 1024,
                allow_truncate: false,
            },
            &[
                ("GH_DEBUG", Path::new("")),
                ("GH_PROMPT_DISABLED", Path::new("1")),
            ],
        )
        .await
        .map_err(|_| CredentialFailure::Transient)?;
        if !output.ok() {
            return Err(CredentialFailure::SignedOut);
        }
        let token =
            trim(std::str::from_utf8(&output.stdout).map_err(|_| CredentialFailure::Transient)?)
                .to_owned();
        if token.is_empty() {
            Err(CredentialFailure::SignedOut)
        } else {
            Ok(token)
        }
    }

    async fn credential(&self, s: &State, host: &str) -> Option<String> {
        // Native settings read/materialization failures behave like absent saved credentials.
        let settings = crate::settings::read_materialized(s)
            .ok()
            .unwrap_or_else(|| json!({}));
        let providers = &settings["secretProviders"];
        let resolved = crate::settings_secrets::resolve_fields(
            s.secret_resolver().as_ref(),
            providers,
            "hosts",
            Some("github"),
            &settings["sourceControlHosts"]["github"],
        )
        .await;
        let github = crate::managed_settings_codec::read_source_host_settings("github", resolved)?;
        let choice = &github["hosts"][host];
        if choice["enabled"] == false {
            return None;
        }
        // A valid unreadable reference refuses fallback to another account.
        let saved =
            crate::settings_secrets::require_literal(&github["tokens"][host], providers).ok()?;
        if !trim(&saved).is_empty() {
            return Some(trim(&saved).into());
        }
        // App refresh is a separate driver; preserve the configured account boundary.
        if github["signIns"][host]
            .as_str()
            .is_some_and(|value| !trim(value).is_empty())
        {
            return None;
        }
        let account = choice["account"]
            .as_str()
            .map(trim)
            .filter(|v| !v.is_empty());
        let key = format!("{host}\0{}", account.unwrap_or(""));
        let gate = {
            let mut gates = self.credential_gates.lock().unwrap();
            gates.retain(|_, v| v.strong_count() > 0);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key.clone(), std::sync::Arc::downgrade(&gate));
                gate
            }
        };
        let _lookup = gate.lock().await;
        {
            let cache = self.credentials.lock().await;
            if let Some(cached) = cache.get(&key).filter(|c| c.expires > Instant::now()) {
                return cached.token.clone();
            }
        }
        let token = if let Some(token) = environment_token(host, |name| std::env::var(name).ok()) {
            Some(token)
        } else {
            let result = match self.cli_token(&s.dir, host, account).await {
                Err(CredentialFailure::SignedOut) if account.is_some() => {
                    self.cli_token(&s.dir, host, None).await
                }
                result => result,
            };
            match result {
                Ok(token) => Some(token),
                Err(CredentialFailure::Transient) => return None,
                Err(_) => None,
            }
        };
        let mut cache = self.credentials.lock().await;
        cache.retain(|_, v| v.expires > Instant::now());
        if cache.len() >= 32 {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, v)| v.expires)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            key,
            CachedCredential {
                expires: Instant::now()
                    + Duration::from_secs(if token.is_some() { 300 } else { 10 }),
                token: token.clone(),
            },
        );
        token
    }

    async fn send(&self, host: &str, token: &str, url: &str, write: bool) -> bool {
        let _permit = match self.http_slots.acquire().await {
            Ok(v) => v,
            Err(_) => return false,
        };
        let fingerprint = format!("{host}:{:x}", Sha256::digest(token.as_bytes()));
        let now = now_ms();
        let lease = {
            let mut quotas = self.quota.lock().unwrap();
            let quota = quotas.entry(fingerprint.clone()).or_default();
            if quota.retry_at > now || (quota.reset_at > now && quota.remaining < quota.limit * 0.1)
            {
                return false;
            }
            quota.generation
        };
        if self.client.get().is_none() {
            let Ok(client) = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
            else {
                return false;
            };
            let _ = self.client.set(client);
        }
        let client = self.client.get().unwrap();
        let method = if write {
            reqwest::Method::PUT
        } else {
            reqwest::Method::GET
        };
        let response = match client
            .request(method, url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "t3code")
            .send()
            .await
        {
            Ok(v) => v,
            Err(_) => return false,
        };
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut response = response;
        let mut body = Vec::new();
        while body.len() < 8 * 1024 * 1024 {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let left = 8 * 1024 * 1024 - body.len();
                    body.extend_from_slice(&chunk[..chunk.len().min(left)]);
                }
                Ok(None) => break,
                Err(_) => return false,
            }
        }
        let number = |key: &str| {
            headers
                .get(key)
                .and_then(|v| v.to_str().ok())
                .and_then(header_number)
        };
        {
            let mut quotas = self.quota.lock().unwrap();
            let quota = quotas.entry(fingerprint).or_default();
            if headers
                .get("x-ratelimit-resource")
                .is_some_and(|v| v == "core")
            {
                if let (Some(limit), Some(remaining), Some(reset)) = (
                    number("x-ratelimit-limit"),
                    number("x-ratelimit-remaining"),
                    number("x-ratelimit-reset"),
                ) {
                    let reset = (reset * 1000.0) as i64;
                    if limit > 0.0 && remaining >= 0.0 && reset >= quota.reset_at {
                        quota.limit = limit;
                        quota.remaining = remaining;
                        quota.reset_at = reset;
                    }
                }
            }
            let limited = status == 429
                || (status == 403
                    && (headers
                        .get("x-ratelimit-remaining")
                        .is_some_and(|v| v == "0")
                        || headers.contains_key("retry-after")
                        || String::from_utf8_lossy(&body)
                            .to_lowercase()
                            .contains("rate limit")));
            if limited {
                let now = now_ms();
                let explicit = headers
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| retry_after(v, now))
                    .or_else(|| {
                        number("x-ratelimit-reset")
                            .map(|v| (v * 1000.0) as i64)
                            .filter(|v| *v > now)
                    });
                if quota.generation > lease {
                    if let Some(retry) = explicit.filter(|v| *v > quota.retry_at) {
                        quota.retry_at = retry;
                    }
                } else {
                    quota.attempts = quota.attempts.saturating_add(1);
                    quota.generation = lease.saturating_add(1);
                    let retry = explicit.unwrap_or_else(|| {
                        now + 30_000_i64
                            .saturating_mul(2_i64.saturating_pow(quota.attempts.saturating_sub(1)))
                            .min(900_000)
                    });
                    quota.retry_at = quota.retry_at.max(retry);
                }
            } else if (200..300).contains(&status)
                && quota.generation == lease
                && quota.retry_at <= now_ms()
            {
                quota.attempts = 0;
                quota.retry_at = 0;
            }
        }
        if status == 401 {
            self.credentials
                .lock()
                .await
                .retain(|key, _| !key.starts_with(&format!("{host}\0")));
        }
        status == 204
    }
}

pub(crate) async fn star(s: &State, p: &Value, write: bool) -> Value {
    if p["provider"] != "github" {
        return json!(false);
    }
    let host = trim(p["host"].as_str().unwrap_or("")).to_lowercase();
    if !valid_host(&host) {
        return json!(false);
    }
    let Some(path) = starred_path(trim(p["repository"].as_str().unwrap_or(""))) else {
        return json!(false);
    };
    let Some(token) = s.forge.credential(s, &host).await else {
        return json!(false);
    };
    let root = api_root(&host);
    #[cfg(test)]
    let root = s
        .forge
        .fixture_api_root
        .lock()
        .unwrap()
        .clone()
        .unwrap_or(root);
    json!(
        s.forge
            .send(&host, &token, &format!("{root}/{path}"), write)
            .await
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forge_boundaries_match_executed_upstream_helpers() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/forge.json")).unwrap();
        for case in fixtures["hosts"].as_array().unwrap() {
            assert_eq!(
                json!(valid_host(case["host"].as_str().unwrap())),
                case["expected"],
                "{case}"
            );
        }
        for case in fixtures["paths"].as_array().unwrap() {
            assert_eq!(
                json!(starred_path(case["repository"].as_str().unwrap())),
                case["expected"],
                "{case}"
            );
        }
        for case in fixtures["urls"].as_array().unwrap() {
            assert_eq!(
                json!(api_root(case["host"].as_str().unwrap())),
                case["expected"],
                "{case}"
            );
        }
        for case in fixtures["environment"].as_array().unwrap() {
            assert_eq!(
                json!(environment_token(case["host"].as_str().unwrap(), |name| {
                    case["env"][name].as_str().map(str::to_owned)
                })),
                case["expected"],
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn native_private_secret_slots_materialize_without_exposing_or_switching_accounts() {
        crate::test_sandbox::assert_isolated();
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "o3-forge-native-secret-{}",
            crate::store::random_id().unwrap()
        ));
        let state = State::open(&path).unwrap();
        let github = GitHub::default();
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/forge.json")).unwrap();
        for case in fixtures["secretNames"].as_array().unwrap() {
            let host = case["input"]["serverHost"].as_str().unwrap();
            std::fs::write(state.dir.join("settings.json"),json!({"sourceControlHosts":{"github":{"tokens":{host:crate::settings::SENTINEL}}}}).to_string()).unwrap();
            let secret = state
                .dir
                .join("secrets")
                .join(format!("{}.bin", case["expected"].as_str().unwrap()));
            std::fs::write(&secret, b" fixture-native-token \n").unwrap();
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                github.credential(&state, host).await,
                Some("fixture-native-token".into())
            );
            assert!(!crate::settings::get(&state)
                .unwrap()
                .to_string()
                .contains("fixture-native-token"));
            assert_eq!(
                crate::settings::get(&state).unwrap()["sourceControlHosts"]["github"]["tokens"]
                    [host],
                crate::settings::SENTINEL
            );
        }
        std::fs::write(state.dir.join("settings.json"),json!({"sourceControlHosts":{"github":{"signIns":{"github.com":"fixture-app-session"}}}}).to_string()).unwrap();
        assert_eq!(
            github.credential(&state, "github.com").await,
            None,
            "a configured app account stays distinct from personal fallback"
        );
    }

    #[test]
    fn managed_forge_lock_cannot_resurrect_hidden_private_credentials() {
        crate::test_sandbox::assert_isolated();
        let path = std::env::temp_dir().join(format!(
            "o3-forge-managed-secret-{}",
            crate::store::random_id().unwrap()
        ));
        let policy = crate::managed_policy::Policy::parse(
            &path.join("machine-policy.json"),
            &json!({"schemaVersion":1,"organization":"Example Corp","settings":{"sourceControlHosts":{"value":{},"locked":true}}}).to_string(),
        ).unwrap();
        // Exercise the consumer before enabling the still-gated Settings family.
        let mut state = State::open(&path).unwrap();
        state.managed_policy = policy;
        let stored = json!({"sourceControlHosts":{"github":{"tokens":{"github.com":crate::settings::SENTINEL},"signIns":{"github.com":"hidden-app-session"}}}}).to_string();
        std::fs::write(state.dir.join("settings.json"), &stored).unwrap();
        use base64::Engine;
        let encode =
            |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes());
        let secret = state.dir.join("secrets").join(format!(
            "source-control-{}-{}-{}.bin",
            encode("github"),
            encode("tokens"),
            encode("github.com")
        ));
        std::fs::write(&secret, b"hidden-personal-token").unwrap();
        let effective = crate::settings_secrets::materialize_profile(
            &state.dir,
            &crate::settings::read(&state).unwrap(),
        )
        .unwrap();
        assert!(effective["sourceControlHosts"]["github"]["tokens"]["github.com"].is_null());
        assert!(effective["sourceControlHosts"]["github"]["signIns"]["github.com"].is_null());
        assert_eq!(
            std::fs::read_to_string(state.dir.join("settings.json")).unwrap(),
            stored
        );
        assert_eq!(std::fs::read(secret).unwrap(), b"hidden-personal-token");
    }

    async fn star_request(listener: &tokio::net::TcpListener) -> (tokio::net::TcpStream, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut head = Vec::new();
            while !head.windows(4).any(|v| v == b"\r\n\r\n") {
                let mut buf = [0u8; 1024];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0 && head.len() + n < 16384);
                head.extend_from_slice(&buf[..n]);
            }
            let head = String::from_utf8(head).unwrap();
            if head.starts_with("GET /user/starred/") || head.starts_with("PUT /user/starred/") {
                return (socket, head);
            }
            // Other library tests discover local HTTP ports with HEAD; those are not forge work.
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
    }

    async fn endpoint(
        answers: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, headers) in answers {
                let (mut socket, head) = star_request(&listener).await;
                requests.push(head);
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n{headers}\r\n").as_bytes()).await.unwrap();
            }
            requests
        });
        (base, task)
    }

    #[tokio::test]
    async fn native_secret_reference_reaches_http_and_unreadable_reference_refuses_fallback() {
        crate::test_sandbox::assert_isolated();
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "o3-forge-op-{}",
            crate::store::random_id().unwrap()
        ));
        let mut state = State::open(&path).unwrap();
        let binary = path.join("op");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/secret-op.py"),
            &binary,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            path.join("case.json"),
            json!({"read":{"op://fixture/item/token":{"stdout":"fixture-resolved-token"}}})
                .to_string(),
        )
        .unwrap();
        state.secret_resolver =
            std::sync::Arc::new(crate::secret_resolver::SecretResolver::new(vec![
                std::sync::Arc::new(crate::secret_resolver::OnePassword::with_binary(binary)),
            ]));
        let settings = json!({"secretProviders":{"vault":{"kind":"one_password","label":"Private fixture","config":{"account":"fixture.1password.test"}}},"sourceControlHosts":{"github":{"tokens":{"fixture.test":{"provider":"vault","reference":"op://fixture/item/token"}}}}});
        std::fs::write(path.join("settings.json"), settings.to_string()).unwrap();
        let github = GitHub::default();
        let token = github.credential(&state, "fixture.test").await.unwrap();
        assert_eq!(token, "fixture-resolved-token");
        let (base, task) = endpoint(vec![(204, String::new())]).await;
        assert!(
            github
                .send(
                    "fixture.test",
                    &token,
                    &format!("{base}/user/starred/o/r"),
                    false
                )
                .await
        );
        assert!(task.await.unwrap()[0]
            .to_lowercase()
            .contains("authorization: bearer fixture-resolved-token"));
        std::fs::write(
            path.join("case.json"),
            json!({"read":{"op://fixture/item/token":{"code":1}}}).to_string(),
        )
        .unwrap();
        state.secret_resolver.invalidate(None).await;
        assert!(
            github.credential(&state, "fixture.test").await.is_none(),
            "an unreadable saved reference cannot use a cached account token"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("settings.json")).unwrap(),
            settings.to_string()
        );
        assert!(!crate::settings::get(&state)
            .unwrap()
            .to_string()
            .contains("fixture-resolved-token"));
    }

    #[tokio::test]
    async fn blank_literal_falls_back_to_private_cli_but_unreadable_reference_does_not() {
        crate::test_sandbox::assert_isolated();
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "o3-forge-cli-{}",
            crate::store::random_id().unwrap()
        ));
        let state = State::open(&directory).unwrap();
        let binary = directory.join("gh");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/secret-gh.py"),
            &binary,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let github = GitHub::default();
        *github.fixture_cli.lock().unwrap() = Some(binary);
        std::fs::write(
            directory.join("settings.json"),
            json!({"sourceControlHosts":{"github":{"tokens":{"fixture.test":"  "}}}}).to_string(),
        )
        .unwrap();
        assert_eq!(
            github.credential(&state, "fixture.test").await,
            Some("fixture-cli-token".into())
        );
        let calls = std::fs::read_to_string(directory.join("gh-calls.jsonl")).unwrap();
        std::fs::write(directory.join("settings.json"),json!({"sourceControlHosts":{"github":{"tokens":{"fixture.test":{"provider":"missing","reference":"op://fixture/item/token"}}}}}).to_string()).unwrap();
        assert!(github.credential(&state, "fixture.test").await.is_none());
        assert_eq!(
            std::fs::read_to_string(directory.join("gh-calls.jsonl")).unwrap(),
            calls
        );
    }

    #[tokio::test]
    async fn only_204_succeeds_and_mutation_uses_put_with_native_headers() {
        for status in [204, 200, 404, 401, 403, 429, 500] {
            let state = GitHub::default();
            let (base, task) = endpoint(vec![(status, String::new())]).await;
            assert_eq!(
                state
                    .send(
                        "fixture.test",
                        "fixture-token",
                        &format!("{base}/user/starred/o/r"),
                        true
                    )
                    .await,
                status == 204
            );
            let requests = task.await.unwrap();
            let request = requests[0].to_lowercase();
            assert!(request.starts_with("put /user/starred/o/r http/1.1\r\n"));
            for header in [
                "authorization: bearer fixture-token",
                "accept: application/vnd.github+json",
                "x-github-api-version: 2022-11-28",
                "user-agent: t3code",
            ] {
                assert!(request.contains(header));
            }
        }
    }

    #[tokio::test]
    async fn reads_respect_quota_reserve_and_credential_boundaries() {
        let reset = now_ms() / 1000 + 60;
        let headers = |remaining| {
            format!("x-ratelimit-resource: core\r\nx-ratelimit-limit: 100\r\nx-ratelimit-remaining: {remaining}\r\nx-ratelimit-reset: {reset}\r\n")
        };
        let (base, task) = endpoint(vec![
            (204, headers(10)),
            (204, headers(9)),
            (204, String::new()),
        ])
        .await;
        let state = GitHub::default();
        let url = format!("{base}/user/starred/o/r");
        assert!(state.send("fixture.test", "account-a", &url, false).await);
        assert!(state.send("fixture.test", "account-a", &url, false).await);
        assert!(!state.send("fixture.test", "account-a", &url, false).await);
        assert!(state.send("fixture.test", "account-b", &url, false).await);
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests
            .iter()
            .all(|r| r.starts_with("GET /user/starred/o/r HTTP/1.1\r\n")));
    }

    #[tokio::test]
    async fn rate_limit_pause_is_not_bypassed_by_stars_or_other_repositories() {
        let (base, task) = endpoint(vec![
            (429, "retry-after: 60\r\n".into()),
            (204, String::new()),
        ])
        .await;
        let state = GitHub::default();
        assert!(
            !state
                .send(
                    "fixture.test",
                    "account-a",
                    &format!("{base}/user/starred/o/a"),
                    false
                )
                .await
        );
        assert!(
            !state
                .send(
                    "fixture.test",
                    "account-a",
                    &format!("{base}/user/starred/o/b"),
                    true
                )
                .await
        );
        assert!(
            state
                .send(
                    "fixture.test",
                    "account-b",
                    &format!("{base}/user/starred/o/b"),
                    true
                )
                .await
        );
        assert_eq!(task.await.unwrap().len(), 2);
    }

    #[test]
    fn compatible_client_reads_and_stars_through_real_http_without_host_credentials() {
        use tokio::io::AsyncWriteExt;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let requests = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = requests.clone();
        let (base,task)=runtime.block_on(async {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base=format!("http://{}",listener.local_addr().unwrap());
            let task=tokio::spawn(async move {
                let mut starred=false;
                loop {
                    let (mut socket,head)=star_request(&listener).await;
                    if head.starts_with("PUT ") {starred=true;}
                    observed.lock().unwrap().push(head);
                    let status=if starred {204} else {404};
                    socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                }
            });
            (base,task)
        });
        let server =
            test_support::start(|state| *state.forge.fixture_api_root.lock().unwrap() = Some(base));
        let client = server.connect();
        client.rpc("server.updateSettings",json!({"patch":{"sourceControlHosts":{"github":{"tokens":{"github.com":"fixture-token"}}}}})).unwrap();
        let input = json!({"provider":"github","host":"GitHub.com","repository":"owner/café ?#"});
        assert_eq!(
            client.rpc("sourceControl.starred", input.clone()).unwrap(),
            false
        );
        assert_eq!(
            client.rpc("sourceControl.star", input.clone()).unwrap(),
            true
        );
        assert_eq!(client.rpc("sourceControl.starred", input).unwrap(), true);
        task.abort();
        assert!(runtime.block_on(task).unwrap_err().is_cancelled());
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert!(requests[0].starts_with("GET /user/starred/owner/caf%C3%A9%20%3F%23 HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("PUT /user/starred/owner/caf%C3%A9%20%3F%23 HTTP/1.1\r\n"));
        assert!(requests.iter().all(|r| r.contains("Bearer fixture-token")));
    }
}
