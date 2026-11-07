//! Whole native credential policies, without host processes, credentials or network.
#[path = "../src/source_credentials.rs"]
mod source_credentials;
use o3_server::settings_secrets;
use serde_json::{json, Value};
use source_credentials::{
    BitbucketCredential, CafeCredentials, Context, CredentialError, ProcessFailure, Request,
};
use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
struct Fixture {
    input: Value,
    settings: Mutex<Value>,
    now: AtomicU64,
    calls: Mutex<Vec<Value>>,
    reads: Mutex<Vec<String>>,
    answers: Mutex<VecDeque<Value>>,
}
impl Fixture {
    fn new(input: Value) -> Self {
        Self {
            settings: Mutex::new(full(input.get("settings").cloned().unwrap_or(json!({})))),
            answers: Mutex::new(
                input["process"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into(),
            ),
            input,
            now: AtomicU64::new(0),
            calls: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
        }
    }
}
fn full(input: Value) -> Value {
    let mut result = json!({"sourceControlHosts":{},"secretProviders":{}});
    for (key, value) in input.as_object().into_iter().flatten() {
        result[key] = value.clone();
    }
    result
}
impl Context for Fixture {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::Relaxed)
    }
    fn environment(&self) -> Value {
        self.input.get("env").cloned().unwrap_or(json!({}))
    }
    fn cwd(&self) -> PathBuf {
        "/private-fixture/workspace".into()
    }
    fn settings<'a>(
        &'a self,
        kind: &'static str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ()>> + Send + 'a>> {
        Box::pin(async move {
            self.reads.lock().unwrap().push(kind.into());
            if self.input["settingsFailure"] == true {
                Err(())
            } else {
                Ok(self.settings.lock().unwrap().clone())
            }
        })
    }
    fn process<'a>(
        &'a self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<String, ProcessFailure>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(json!({"operation":r.operation,"command":r.command,"args":r.args,"cwd":r.cwd,"stdin":r.stdin,"env":r.env,"timeoutMs":r.timeout_ms}));
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(json!({"stdout":"password=private-cli-token\n"}));
            if let Some(delay) = answer["delay"].as_u64() {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            match answer["failure"].as_str() {
                Some("missing") => Err(ProcessFailure::Missing),
                Some("exit") => Err(ProcessFailure::Exit),
                Some(_) => Err(ProcessFailure::Other),
                None => Ok(answer["stdout"].as_str().unwrap_or("").into()),
            }
        })
    }
    fn bitbucket_url(&self) -> String {
        let base = self.input["env"]["T3CODE_BITBUCKET_API_BASE_URL"]
            .as_str()
            .unwrap_or("https://api.bitbucket.org/2.0");
        format!("{}/user", base.trim_end_matches('/'))
    }
    fn http<'a>(
        &'a self,
        url: String,
        credential: Option<BitbucketCredential>,
    ) -> Pin<Box<dyn Future<Output = Result<(u16, Vec<u8>), ()>> + Send + 'a>> {
        Box::pin(async move {
            let mut call = json!({"method":"GET","url":url});
            if let Some(credential) = credential {
                call["authorization"] = json!(credential.authorization());
            }
            self.calls.lock().unwrap().push(call);
            let status = self.input["status"].as_u64().unwrap_or(200) as u16;
            let value = self
                .input
                .get("body")
                .cloned()
                .unwrap_or(json!({"username":"fixture-user"}));
            let bytes = if status == 204 {
                Vec::new()
            } else if let Some(value) = value.as_str() {
                value.as_bytes().to_vec()
            } else {
                value.to_string().into_bytes()
            };
            Ok((status, bytes))
        })
    }
}
fn error(error: CredentialError) -> Value {
    let mut value = json!({"tag":error.tag(),"message":error.message()});
    if let Some(host) = error.host() {
        value["host"] = json!(host);
    }
    json!({"error":value})
}
fn credential_result(value: Result<source_credentials::CafeCredential, CredentialError>) -> Value {
    match value {
        Ok(value) => json!({"value":{"host":value.host,"token":value.token,"source":value.source}}),
        Err(e) => error(e),
    }
}
fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/source-credentials-4288.json")).unwrap()
}
#[tokio::test]
async fn whole_native_gitcafe_cache_and_resolution_methods() {
    let f = fixtures();
    assert_eq!(f["cafeCases"].as_array().unwrap().len(), 58);
    for case in f["cafeCases"].as_array().unwrap() {
        let context = Fixture::new(case["input"].clone());
        let service = CafeCredentials::default();
        let mut results = Vec::new();
        let defaults = vec![json!({"operation":"get","host":"git.cafe"})];
        let steps = case["input"]["steps"].as_array().unwrap_or(&defaults);
        for step in steps {
            context
                .now
                .fetch_add(step["advance"].as_u64().unwrap_or(0), Ordering::Relaxed);
            if let Some(settings) = step.get("settings") {
                *context.settings.lock().unwrap() = full(settings.clone());
            }
            let host = step["host"].as_str().unwrap_or("");
            let result = match step["operation"].as_str().unwrap() {
                "invalidate" => {
                    service.invalidate(host);
                    json!({"value":null})
                }
                "cliEnv" => match service.cli_env(&context, host).await {
                    Ok(v) => json!({"value":v}),
                    Err(e) => error(e),
                },
                "parallel" => {
                    let futures = step["hosts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|h| service.get(&context, h.as_str().unwrap()));
                    let result = futures_util::future::join_all(futures)
                        .await
                        .into_iter()
                        .map(|r| r.unwrap())
                        .map(|v| json!({"host":v.host,"token":v.token,"source":v.source}))
                        .collect::<Vec<_>>();
                    json!({"value":result})
                }
                _ => credential_result(service.get(&context, host).await),
            };
            results.push(json!({"step":step,"result":result,"calls":context.calls.lock().unwrap().len(),"reads":context.reads.lock().unwrap().len()}));
        }
        assert_eq!(json!(results), case["result"], "{}", case["label"]);
        assert_eq!(
            json!(*context.calls.lock().unwrap()),
            case["calls"],
            "{} requests",
            case["label"]
        );
        assert_eq!(
            json!(*context.reads.lock().unwrap()),
            case["reads"],
            "{} reads",
            case["label"]
        );
    }
}
#[tokio::test]
async fn whole_native_bitbucket_probe_and_header_precedence() {
    let f = fixtures();
    assert_eq!(f["bitbucketCases"].as_array().unwrap().len(), 80);
    for case in f["bitbucketCases"].as_array().unwrap() {
        let context = Fixture::new(case["input"].clone());
        let result = source_credentials::bitbucket_probe(&context).await;
        assert_eq!(json!({"value":result}), case["result"], "{}", case["label"]);
        assert_eq!(
            json!(*context.calls.lock().unwrap()),
            case["calls"],
            "{} requests",
            case["label"]
        );
        assert_eq!(
            json!(*context.reads.lock().unwrap()),
            case["reads"],
            "{} reads",
            case["label"]
        );
    }
}
