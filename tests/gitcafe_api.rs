#[path = "support/sandbox.rs"]
mod test_sandbox;
pub use o3_server::source_credentials;
#[path = "../src/gitcafe_api.rs"]
pub mod gitcafe_api;
use gitcafe_api::*;
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, sync::Mutex};
struct Fake {
    input: Value,
    calls: Mutex<Vec<Value>>,
    reads: Mutex<Vec<String>>,
    invalidations: Mutex<Vec<String>>,
    position: Mutex<usize>,
}
impl Context for Fake {
    fn credential<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        source_credentials::CafeCredential,
                        source_credentials::CredentialError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.reads.lock().unwrap().push(host.into());
            match self.input["credentialFailure"].as_str(){
            Some("missing")=>Err(source_credentials::CredentialError::Missing(host.into())),Some("signed-out")=>Err(source_credentials::CredentialError::SignedOut(host.into())),Some("unresolved")=>Err(source_credentials::CredentialError::Unresolved("Could not read the credential from the secret provider \"Private vault\". Unlock or fix it in Settings, then try again.".into())),Some(_)=>Err(source_credentials::CredentialError::Failed(host.into())),None=>Ok(source_credentials::CafeCredential{host:host.into(),token:"fixture-api-token".into(),source:"cafe"})}
        })
    }
    fn invalidate<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.invalidations.lock().unwrap().push(host.into());
        })
    }
    fn send<'a>(
        &'a self,
        r: Outgoing,
    ) -> Pin<Box<dyn Future<Output = Result<Response, ()>> + Send + 'a>> {
        Box::pin(async move {
            let mut headers = json!({"accept":"application/json","user-agent":"t3code","authorization":"<redacted>"});
            let body = r.body.map(|value| {
                headers["content-type"] = json!("application/json");
                let body = serde_json::to_string(&value).unwrap();
                headers["content-length"] = json!(body.len().to_string());
                body
            });
            self.calls.lock().unwrap().push(json!({"method":r.method.as_str(),"url":url::Url::parse(&r.url).unwrap().to_string(),"headers":headers,"authorization":format!("Bearer {}",r.token),"body":body}));
            let position = {
                let mut index = self.position.lock().unwrap();
                let n = *index;
                *index += 1;
                n
            };
            let value = self.input["responses"]
                .as_array()
                .and_then(|v| v.get(position))
                .cloned()
                .unwrap_or(json!({"status":200,"text":"fixture-success"}));
            if value["stall"] == true {
                return std::future::pending().await;
            }
            if value["failure"] == true {
                return Err(());
            }
            let status = value["status"].as_u64().unwrap_or(200) as u16;
            Ok(Response {
                status,
                location: value["location"].as_str().map(str::to_owned),
                text: if matches!(status, 204 | 205 | 304) {
                    String::new()
                } else {
                    value["text"]
                        .as_str()
                        .unwrap_or("")
                        .strip_prefix('\u{feff}')
                        .unwrap_or(value["text"].as_str().unwrap_or(""))
                        .into()
                },
            })
        })
    }
}
fn units(value: &Value, key: &str) -> Vec<u16> {
    value[key]
        .as_str()
        .map(|text| text.encode_utf16().collect())
        .unwrap_or_else(|| {
            value[format!("{key}Utf16")]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u16)
                .collect()
        })
}
#[tokio::test]
async fn whole_native_request_policy_matches_results_http_and_invalidations() {
    test_sandbox::assert_isolated();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/gitcafe-api-4288.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 478);
    for case in fixture["cases"].as_array().unwrap() {
        let fake = Fake {
            input: case["input"].clone(),
            calls: Default::default(),
            reads: Default::default(),
            invalidations: Default::default(),
            position: Default::default(),
        };
        let input: Request =
            serde_json::from_value(case["input"].get("request").cloned().unwrap_or(
                json!({"host":"git.cafe","operation":"fixture","path":"/auth/principal"}),
            ))
            .unwrap();
        match request(&fake, &input).await {
            Ok(value) => assert_eq!(json!(value), case["result"]["value"], "{}", case["label"]),
            Err(error) => {
                let expected = &case["result"]["error"];
                assert!(
                    expected.is_object(),
                    "{} unexpected {error:?}",
                    case["label"]
                );
                assert_eq!(error.host, expected["host"]);
                assert_eq!(error.operation, expected["operation"]);
                assert_eq!(json!(error.status), expected["status"]);
                assert_eq!(error.reason, expected["reason"]);
                assert_eq!(
                    error.detail_units(),
                    units(expected, "detail"),
                    "{} detail",
                    case["label"]
                );
                assert_eq!(
                    error.message_units(),
                    units(expected, "message"),
                    "{} message",
                    case["label"]
                );
                assert!(!format!("{error:?}").contains("fixture-api-token"));
            }
        }
        assert_eq!(
            json!(*fake.calls.lock().unwrap()),
            case["calls"],
            "{} calls",
            case["label"]
        );
        assert_eq!(
            json!(*fake.reads.lock().unwrap()),
            case["credentialReads"],
            "{} reads",
            case["label"]
        );
        assert_eq!(
            json!(*fake.invalidations.lock().unwrap()),
            case["invalidations"],
            "{} invalidations",
            case["label"]
        );
    }
}
