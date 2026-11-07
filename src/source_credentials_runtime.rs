//! Physical discovery transports for the pinned source-control credential policy.
use crate::{proc, source_credentials::*, store::State};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};

pub(crate) struct Services {
    pub(crate) cafe: CafeCredentials,
    slots: tokio::sync::Semaphore,
    client: OnceLock<reqwest::Client>,
    started: Instant,
    #[cfg(test)]
    endpoints: std::sync::Mutex<Option<(String, String)>>,
    #[cfg(test)]
    executable: std::sync::Mutex<Option<String>>,
}
impl Default for Services {
    fn default() -> Self {
        Self {
            cafe: Default::default(),
            slots: tokio::sync::Semaphore::new(4),
            client: OnceLock::new(),
            started: Instant::now(),
            #[cfg(test)]
            endpoints: Default::default(),
            #[cfg(test)]
            executable: Default::default(),
        }
    }
}
struct Physical<'a> {
    state: &'a State,
}
impl Context for Physical<'_> {
    fn now_ms(&self) -> u64 {
        self.state
            .forge
            .source_credentials
            .started
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
    fn cwd(&self) -> PathBuf {
        self.state.dir.clone()
    }
    fn environment(&self) -> Value {
        let mut out = serde_json::Map::new();
        for key in [
            "CAFE_HOST",
            "CAFE_TOKEN",
            "T3CODE_BITBUCKET_ACCESS_TOKEN",
            "T3CODE_BITBUCKET_EMAIL",
            "T3CODE_BITBUCKET_API_TOKEN",
            "T3CODE_BITBUCKET_API_BASE_URL",
        ] {
            if let Ok(value) = std::env::var(key) {
                out.insert(key.into(), json!(value));
            }
        }
        // Retain the older O3 alias only when the native variable is absent.
        if !out.contains_key("T3CODE_BITBUCKET_ACCESS_TOKEN") {
            if let Ok(value) = std::env::var("BITBUCKET_ACCESS_TOKEN") {
                out.insert("T3CODE_BITBUCKET_ACCESS_TOKEN".into(), json!(value));
            }
        }
        Value::Object(out)
    }
    fn settings<'a>(
        &'a self,
        kind: &'static str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, ()>> + Send + 'a>> {
        Box::pin(async move {
            let mut settings = crate::settings::read_materialized(self.state).map_err(|_| ())?;
            let fields = crate::settings_secrets::resolve_fields(
                &self.state.secret_resolver,
                &settings["secretProviders"],
                "sourceControlHosts",
                Some(kind),
                &settings["sourceControlHosts"][kind],
            )
            .await;
            settings["sourceControlHosts"][kind] = fields;
            Ok(settings)
        })
    }
    fn process<'a>(
        &'a self,
        r: Request,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, ProcessFailure>> + Send + 'a>,
    > {
        Box::pin(async move {
            let command = r.command.to_owned();
            #[cfg(test)]
            let command = self
                .state
                .forge
                .source_credentials
                .executable
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(command);
            let args = r.args.iter().map(String::as_str).collect::<Vec<_>>();
            let env = r
                .env
                .iter()
                .map(|(k, v)| (k.as_str(), Path::new(v)))
                .collect::<Vec<_>>();
            match proc::run_with_input(
                &command,
                &r.cwd,
                &args,
                proc::Opts {
                    timeout: Duration::from_millis(r.timeout_ms),
                    cap: 65536,
                    allow_truncate: false,
                },
                &env,
                r.stdin.as_bytes(),
            )
            .await
            {
                Ok(out) if out.ok() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
                Ok(_) => Err(ProcessFailure::Exit),
                Err(error) if error.spawn_kind == Some(std::io::ErrorKind::NotFound) => {
                    Err(ProcessFailure::Missing)
                }
                Err(_) => Err(ProcessFailure::Other),
            }
        })
    }
    fn bitbucket_url(&self) -> String {
        #[cfg(test)]
        if let Some((_, url)) = self
            .state
            .forge
            .source_credentials
            .endpoints
            .lock()
            .unwrap()
            .clone()
        {
            return url;
        }
        format!(
            "{}/user",
            self.environment()["T3CODE_BITBUCKET_API_BASE_URL"]
                .as_str()
                .unwrap_or("https://api.bitbucket.org/2.0")
                .trim_end_matches('/')
        )
    }
    fn http<'a>(
        &'a self,
        url: String,
        credential: Option<BitbucketCredential>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>>
    {
        Box::pin(async move {
            let services = &self.state.forge.source_credentials;
            let _slot = services.slots.acquire().await.map_err(|_| ())?;
            let parsed = url::Url::parse(&url).map_err(|_| ())?;
            if !matches!(parsed.scheme(), "http" | "https")
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(());
            }
            if services.client.get().is_none() {
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(|_| ())?;
                let _ = services.client.set(client);
            }
            let mut request = services
                .client
                .get()
                .ok_or(())?
                .get(parsed)
                .header("Accept", "application/json")
                .header("User-Agent", "t3code");
            if let Some(credential) = credential {
                request = request.header("Authorization", credential.authorization());
            }
            let mut response = request.send().await.map_err(|_| ())?;
            let status = response.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                if body.len() + chunk.len() > 8 * 1024 * 1024 {
                    return Err(());
                }
                body.extend_from_slice(&chunk);
            }
            Ok((status, body))
        })
    }
}
pub(crate) async fn auth(state: &State, kind: &str) -> Option<Value> {
    let context = Physical { state };
    if kind == "bitbucket" {
        return Some(bitbucket_probe(&context).await);
    }
    let services = &state.forge.source_credentials;
    let unknown = |status: &str, detail: Option<String>| json!({"status":status,"host":{"_tag":"Some","value":"git.cafe"},"account":{"_tag":"None"},"detail":detail.map(|v|json!({"_tag":"Some","value":v})).unwrap_or(json!({"_tag":"None"}))});
    Some(match services.cafe.get(&context, "git.cafe").await {
        Err(CredentialError::Unresolved(message)) => unknown("unauthenticated", Some(message)),
        Err(_) => return None,
        Ok(credential) => {
            let endpoint = "https://git.cafe/api/auth/principal".to_owned();
            #[cfg(test)]
            let endpoint = services
                .endpoints
                .lock()
                .unwrap()
                .clone()
                .map(|v| v.0)
                .unwrap_or(endpoint);
            match context
                .http(
                    endpoint,
                    Some(BitbucketCredential::Access(credential.token.clone())),
                )
                .await
            {
                Ok((401, _)) => {
                    services.cafe.invalidate("git.cafe");
                    unknown("unauthenticated", None)
                }
                Ok((status, bytes)) if (200..300).contains(&status) => {
                    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
                    let principal: Value = serde_json::from_str(&String::from_utf8_lossy(bytes))
                        .unwrap_or(Value::Null);
                    let mut auth = unknown("unknown", None);
                    if principal["actorId"].as_str().is_some_and(|v| !v.is_empty()) {
                        if let Some(handle) = principal["handle"]
                            .as_str()
                            .filter(|v| !v.is_empty() && v.len() <= 200 && *v != credential.token)
                        {
                            auth["status"] = json!("authenticated");
                            auth["account"] = json!({"_tag":"Some","value":handle});
                        }
                    }
                    auth
                }
                _ => unknown("unknown", None),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
    };
    #[test]
    fn actual_rpc_uses_native_credentials_private_cli_and_refusal_boundaries() {
        crate::test_sandbox::assert_isolated();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let headers = Arc::new(Mutex::new(Vec::new()));
        let captured = headers.clone();
        let http = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    assert_eq!(socket.read(&mut byte).unwrap(), 1);
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 16384);
                }
                let request = String::from_utf8(bytes).unwrap();
                let body = if request.starts_with("GET /cafe ") {
                    r#"{"actorId":"fixture-id","handle":"cafe-reader"}"#
                } else {
                    r#"{"username":"bb-reader"}"#
                };
                captured.lock().unwrap().push(request);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let server = crate::checkpoint::recovery_test_support::start(|state| {
            let script = state.dir.join("synthetic-cafe.py");
            std::fs::write(
                &script,
                r#"#!/usr/bin/python3
import os,sys
assert sys.argv[1:]==['auth','http','credential','--host','https://git.cafe/api','get']
assert sys.stdin.read()=='protocol=https\nhost=git.cafe\n\n'
assert os.environ['CAFE_NO_UPDATE_CHECK']=='1'
with open('credential-calls','a') as f:f.write('x')
print('password=fixture-cli-token')
"#,
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            *state.forge.source_credentials.executable.lock().unwrap() =
                Some(script.to_string_lossy().into_owned());
            *state.forge.source_credentials.endpoints.lock().unwrap() =
                Some((format!("{base}/cafe"), format!("{base}/user")));
        });
        let client = server.connect();
        client.rpc("server.updateSettings",json!({"patch":{"sourceControlHosts":{"gitcafe":{"token":""},"bitbucket":{"email":"fixture@example.test","apiToken":"fixture-api-token"}}}})).unwrap();
        for _ in 0..2 {
            let reply = client
                .rpc("server.discoverSourceControl", json!({}))
                .unwrap();
            for (kind, account) in [("gitcafe", "cafe-reader"), ("bitbucket", "bb-reader")] {
                let provider = reply["sourceControlProviders"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["kind"] == kind)
                    .unwrap();
                assert_eq!(provider["auth"]["status"], "authenticated", "{provider}");
                assert_eq!(provider["auth"]["account"]["value"], account);
            }
            assert!(!reply.to_string().contains("fixture-cli-token"));
            assert!(!reply.to_string().contains("fixture-api-token"));
        }
        http.join().unwrap();
        assert_eq!(
            std::fs::read(server.dir.join("credential-calls")).unwrap(),
            b"x"
        );
        let requests = headers.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|v| v.contains("Bearer fixture-cli-token"))
                .count(),
            2
        );
        use base64::Engine;
        let basic = base64::engine::general_purpose::STANDARD
            .encode("fixture@example.test:fixture-api-token");
        assert_eq!(
            requests
                .iter()
                .filter(|v| v.contains(&format!("Basic {basic}")))
                .count(),
            2
        );
        drop(requests);
        let reference =
            json!({"provider":"missing-private-vault","reference":"op://Fixture/item/token"});
        client.rpc("server.updateSettings",json!({"patch":{"sourceControlHosts":{"gitcafe":{"token":reference},"bitbucket":{"accessToken":reference}}}})).unwrap();
        let reply = client
            .rpc("server.discoverSourceControl", json!({}))
            .unwrap();
        for kind in ["gitcafe", "bitbucket"] {
            let provider = reply["sourceControlProviders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["kind"] == kind)
                .unwrap();
            assert_eq!(provider["auth"]["status"], "unauthenticated", "{provider}");
            assert!(provider["auth"]["detail"]["value"]
                .as_str()
                .unwrap()
                .contains("Could not read the credential"));
        }
        assert_eq!(
            std::fs::read(server.dir.join("credential-calls")).unwrap(),
            b"x"
        );
        assert!(!crate::settings::get(&server.state)
            .unwrap()
            .to_string()
            .contains("fixture-api-token"));
    }
}
