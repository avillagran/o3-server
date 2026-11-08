//! Differential cloud service observations from the whole original Effect layer.
#[path = "support/sandbox.rs"]
mod sandbox;
use o3_server::{
    cloud_speech::{
        self as cloud, Body, Endpoint, Error, Part, Provider, Settings, Snapshot, Task, Text,
        Transport, TransportError,
    },
    speech_request::TranscriptionOptions,
    speech_worker::{NodeRuntime, SealedFile},
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};
struct State {
    credentials: Value,
    resolved: Option<String>,
    config: Value,
    events: Vec<Value>,
    requests: Vec<Value>,
    catalog: usize,
}
struct Fixture(Arc<Mutex<State>>);
impl Settings for Fixture {
    fn check(&self) -> Result<(), Error> {
        Ok(())
    }
    fn read(&self) -> Task<'_, Result<Snapshot, Error>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state.events.push(json!({"kind":"settings-read"}));
            Ok(Snapshot {
                credentials: state.credentials.clone(),
                secret_providers: json!({}),
            })
        })
    }
    fn patch<'a>(&'a self, patch: Value) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            state
                .events
                .push(json!({"kind":"settings-write","patch":patch}));
            for (key, value) in patch["cloudSpeechCredentials"].as_object().unwrap() {
                state.credentials[key] = value.clone();
            }
            Ok(())
        })
    }
}
impl cloud::Secrets for Fixture {
    fn read<'a>(
        &'a self,
        _: &'a Value,
        reference: &'a o3_server::secret_resolver::SecretReference,
    ) -> Task<'a, Option<cloud::ResolvedSecret>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            let references = json!([reference.to_value()]);
            state
                .events
                .push(json!({"kind":"invalidate","references":references}));
            state
                .events
                .push(json!({"kind":"secret-read","references":references}));
            if let Some(next) = state.config.get("afterSecretRead").cloned() {
                state.credentials = next;
            }
            state.resolved.clone().map(|value| cloud::ResolvedSecret {
                value,
                identity: None,
            })
        })
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
impl Transport for Fixture {
    fn execute(
        &self,
        request: cloud::Request,
    ) -> Task<'_, Result<cloud::Response, TransportError>> {
        Box::pin(async move {
            let mut record = json!({"url":request.endpoint.url(),"method":if matches!(request.endpoint,Endpoint::Catalog){"GET"}else{"POST"},"redirect":"error","headers":{}});
            if let Some(key) = request.bearer {
                record["headers"]["authorization"] = json!(format!("Bearer {key}"));
            }
            match request.body {
                Body::Empty => {}
                Body::Json(value) => {
                    record["headers"]["content-type"] = json!("application/json");
                    record["json"] = value;
                }
                Body::Multipart(parts) => {
                    let mut form = vec![];
                    for part in parts {
                        form.push(match part{Part::Field{key,value}=>json!([key,value]),Part::File{key,name,mime,bytes}=>json!([key,{"name":name,"type":mime,"bytes":bytes.len(),"hex":hex(&bytes)}])});
                    }
                    record["form"] = json!(form);
                }
            }
            let mut state = self.0.lock().unwrap();
            state.requests.push(record);
            let (status, body) = if matches!(request.endpoint, Endpoint::Catalog) {
                state.catalog += 1;
                if let Some(next) = state.config.get("afterCatalog").cloned() {
                    state.credentials = next;
                }
                (state.config["catalogStatus"].as_u64().unwrap_or(200) as u16,state.config.get("catalog").cloned().unwrap_or_else(||json!({"data":[{"id":"fixture/transcribe","name":" Fixture ASR "},{"id":"other/model"},{"id":"openrouter/auto"},{"id":"bad id"},{"id":"fixture/blank","name":"   "}]})))
            } else {
                if state.config["rejectFetch"] == true {
                    return Err(TransportError);
                }
                (
                    state.config["status"].as_u64().unwrap_or(200) as u16,
                    state
                        .config
                        .get("reply")
                        .cloned()
                        .unwrap_or_else(|| json!({"text":"  um, oh three  "})),
                )
            };
            Ok(cloud::Response {
                status,
                bytes: serde_json::to_vec(&body).unwrap(),
            })
        })
    }
}
fn recorded(label: &str, result: Result<Value, Error>, observed: &mut Vec<Value>) {
    observed.push(match result{Ok(value)=>json!({"label":label,"ok":true,"value":value}),Err(cause)=>{let mut value=json!({"label":label,"ok":false,"tag":cause.encoded()["_tag"],"provider":null,"reason":null,"message":cause.message()});if let Error::Cloud{provider,reason}=cause{value["provider"]=json!(provider.as_str());value["reason"]=json!(reason.as_str());}value}});
}
#[tokio::test]
async fn actual_text_owner_matches_all_whole_cloud_service_traces_without_paid_network() {
    sandbox::assert_isolated();
    let directory = std::env::temp_dir().join(format!(
        "cloud-service-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let path = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
    let node = NodeRuntime::capture(
        SealedFile::admit(
            &path,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&path).unwrap().len(),
        )
        .unwrap(),
        &directory.join("node"),
    )
    .unwrap();
    let native = Arc::new(cloud::text::NativeText::new(node, directory).unwrap());
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/cloud-speech-4314.json")).unwrap();
    assert_eq!(corpus["cases"].as_array().unwrap().len(), 72);
    let pcm = unhex(corpus["pcmHex"].as_str().unwrap());
    for row in corpus["cases"].as_array().unwrap() {
        let config = &row["input"];
        let state = Arc::new(Mutex::new(State {
            credentials: config.get("credentials").cloned().unwrap_or(json!({})),
            resolved: config["resolved"].as_str().map(str::to_owned),
            config: config.clone(),
            events: vec![],
            requests: vec![],
            catalog: 0,
        }));
        let settings = Arc::new(Fixture(state.clone()));
        let service = cloud::Service::new(
            Arc::new(Fixture(state.clone())),
            Arc::new(Fixture(state.clone())),
            native.clone(),
        );
        let call = service.bind(settings);
        let mut actual = vec![];
        let commands = row["commands"].as_array().unwrap();
        let mut command = 0;
        for expected in row["observations"].as_array().unwrap() {
            let label = expected["label"].as_str().unwrap();
            let result = match label {
                "empty" | "configured" | "cleared" => call.configured().await,
                "save" => call
                    .set_key(Provider::OpenAi, Some(json!("fixture-chosen")))
                    .await
                    .map(|()| Value::Null),
                "clear" => call
                    .set_key(Provider::OpenAi, None)
                    .await
                    .map(|()| Value::Null),
                "set-reference" => call
                    .set_key(
                        Provider::OpenAi,
                        Some(json!({"provider":"fixture-vault","reference":"fixture/voice/key"})),
                    )
                    .await
                    .map(|()| Value::Null),
                "first" | "second" => call
                    .models()
                    .await
                    .map(|models| serde_json::to_value(models).unwrap()),
                "transcribe" | "unlocked" | "locked-again" => {
                    if label == "unlocked" {
                        state.lock().unwrap().resolved = Some("fixture-unlocked".into());
                    } else if label == "locked-again" {
                        state.lock().unwrap().resolved = None;
                    }
                    let request = &commands[command];
                    command += 1;
                    call.transcribe(
                        Provider::parse(request["provider"].as_str().unwrap()).unwrap(),
                        unhex(request["pcmHex"].as_str().unwrap()),
                        TranscriptionOptions::decode(request["options"].clone()).unwrap(),
                        request["model"].as_str().map(str::to_owned),
                    )
                    .await
                    .map(Value::String)
                }
                _ => panic!("Unrecognized original observation {label}"),
            };
            recorded(label, result, &mut actual);
        }
        let state = state.lock().unwrap();
        assert_eq!(json!(actual), row["observations"], "{}", row["name"]);
        assert_eq!(json!(state.events), row["events"], "{}", row["name"]);
        assert_eq!(json!(state.requests), row["requests"], "{}", row["name"]);
        assert_eq!(state.credentials, row["credentials"], "{}", row["name"]);
        assert_eq!(state.catalog, row["catalogFetches"], "{}", row["name"]);
    }
    assert_eq!(pcm.len(), 24);
    native.shutdown().await.unwrap();
    native.shutdown().await.unwrap();
}

#[path = "cloud/http.rs"]
mod http;
