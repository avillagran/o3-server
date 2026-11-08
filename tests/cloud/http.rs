//! Actual authenticated server and provider HTTP with private credentials and pure-text Node.
use super::sandbox;
use o3_server::{
    cloud_speech::{self as cloud, Text},
    secret_resolver::{OnePassword, SecretDriver, SecretResolver},
    speech_worker::{NodeRuntime, SealedFile},
    store::State,
};
use serde_json::{json, Value};
use std::{
    fs,
    future::Future,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
struct Gate {
    entered: tokio::sync::Notify,
    released: tokio::sync::Notify,
}
struct Vault {
    gate: Mutex<Option<Arc<Gate>>>,
    value: Mutex<Option<String>>,
    calls: Mutex<Vec<Value>>,
}
impl SecretDriver for Vault {
    fn kind(&self) -> &str {
        "one_password"
    }
    fn decode_config(&self, value: &Value) -> Option<Value> {
        OnePassword::system().decode_config(value)
    }
    fn is_reference(&self, value: &str) -> bool {
        OnePassword::system().is_reference(value)
    }
    fn read<'a>(
        &'a self,
        config: Value,
        references: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<Option<String>>> + Send + 'a>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(json!({"config":config,"references":references}));
            let gate = self.gate.lock().unwrap().clone();
            let value = self.value.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.entered.notify_one();
                gate.released.notified().await;
            }
            references.iter().map(|_| value.clone()).collect()
        })
    }
}
fn directory() -> PathBuf {
    let value = std::env::temp_dir().join(format!(
        "cloud-http-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new().mode(0o700).create(&value).unwrap();
    value
}
struct Reply {
    status: u16,
    body: Value,
}
async fn request(
    state: Arc<State>,
    token: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
    content: &str,
) -> Reply {
    let (client, server) = tokio::io::duplex(128 * 1024);
    let task = tokio::spawn(o3_server::serve_io(server, state));
    let (mut read, mut write) = tokio::io::split(client);
    write.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: {content}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
    write.write_all(&body).await.unwrap();
    let mut bytes = vec![];
    tokio::time::timeout(Duration::from_secs(30), read.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    task.await.unwrap().unwrap();
    let split = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap();
    let status = std::str::from_utf8(&bytes[..split])
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = &bytes[split + 4..];
    Reply {
        status,
        body: if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap()
        },
    }
}
async fn json_request(
    state: Arc<State>,
    token: &str,
    method: &str,
    path: &str,
    value: Value,
) -> Reply {
    request(
        state,
        token,
        method,
        path,
        if method == "GET" {
            vec![]
        } else {
            serde_json::to_vec(&value).unwrap()
        },
        "application/json",
    )
    .await
}
async fn provider_request(listener: &TcpListener) -> (TcpStream, String, Vec<u8>) {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .expect("Only the armed provider-request phase has this witness")
        .unwrap();
    let mut bytes = vec![];
    let mut block = [0u8; 4096];
    let split = loop {
        let count = socket.read(&mut block).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&block[..count]);
        if let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break split;
        }
        assert!(bytes.len() < 64 * 1024);
    };
    let head = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 1024 * 1024);
    let mut body = bytes.split_off(split + 4);
    while body.len() < length {
        let count = socket.read(&mut block).await.unwrap();
        assert!(count > 0);
        body.extend_from_slice(&block[..count]);
    }
    assert_eq!(body.len(), length);
    (socket, head, body)
}
fn pcm() -> Vec<u8> {
    let options = json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechCustomWords":[{"term":"T3 Code","aliases":["oh three"]},{"term":"line\nterm","aliases":[]}],"speechRemoveFillerWords":true,"speechSilenceFiltering":false});
    let options = serde_json::to_vec(&options).unwrap();
    let mut body = (options.len() as u32).to_le_bytes().to_vec();
    body.extend(options);
    body.extend([0.0f32.to_le_bytes(), 0.5f32.to_le_bytes()].concat());
    body
}
fn assert_worker_stack() {
    let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    assert_eq!(
        unsafe { libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) },
        0
    );
    let mut attributes = unsafe { attributes.assume_init() };
    let mut address = std::ptr::null_mut();
    let mut bytes = 0;
    assert_eq!(
        unsafe { libc::pthread_attr_getstack(&attributes, &mut address, &mut bytes) },
        0
    );
    assert_eq!(unsafe { libc::pthread_attr_destroy(&mut attributes) }, 0);
    assert_eq!(
        bytes,
        512 * 1024,
        "Actual cloud HTTP task must run on the production stack budget"
    );
}
#[test]
fn private_settings_named_vault_and_actual_provider_http_preserve_authority_and_payloads() {
    sandbox::assert_isolated();
    const NAME:&str="http::private_settings_named_vault_and_actual_provider_http_preserve_authority_and_payloads";
    if std::env::var("O3_CLOUD_HTTP_CHILD").as_deref() != Ok(NAME) {
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--test-threads=1", "--nocapture"])
            .env("O3_CLOUD_HTTP_CHILD", NAME)
            .status()
            .unwrap()
            .success());
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {tokio::spawn(async {
        assert_worker_stack();

    sandbox::assert_isolated();
    let root = directory();
    let state = Arc::new(State::open(&root).unwrap());
    let path = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
    let node = NodeRuntime::capture(
        SealedFile::admit(
            &path,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&path).unwrap().len(),
        )
        .unwrap(),
        &root.join("node"),
    )
    .unwrap();
    assert!(root.join("node").is_file());
    let native = Arc::new(cloud::text::NativeText::new(node, root.clone()).unwrap());
    native.prepare().await.unwrap();
    let vault = Arc::new(Vault {
        gate:Mutex::new(None),
        value: Mutex::new(Some("fixture-vault-key".into())),
        calls: Mutex::new(vec![]),
    });
    let resolver = Arc::new(SecretResolver::new(vec![vault.clone()]));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http = Arc::new(cloud::Http::loopback_peer(address).unwrap());
    let weak = Arc::downgrade(&state);
    let provider_source = Arc::new(move || {
        weak.upgrade()
            .and_then(|state| {
                let _guard = state.settings_lock.lock().ok()?;
                let settings = o3_server::settings::read(&state).ok()?;
                o3_server::settings_secrets::materialize_profile(&state.dir, &settings).ok()
            })
            .map(|settings| settings["secretProviders"].clone())
            .unwrap_or(json!({}))
    });
    let service = cloud::Service::new(
        http,
        Arc::new(cloud::Resolver::new(resolver.clone(), provider_source)),
        native,
    );
    o3_server::speech_state::install_cloud(&state, service).unwrap();
    let token = |scopes: &[&str], subject: &str| {
        state
            .issue_session(
                subject,
                &scopes
                    .iter()
                    .map(|scope| scope.to_string())
                    .collect::<Vec<_>>(),
                3_600_000,
                "private-cloud-http",
            )
            .unwrap()
    };
    let operator = token(&["orchestration:operate"], "fixture-operator");
    let writer = token(
        &["orchestration:operate", "settings:write"],
        "fixture-writer",
    );
    let reader = token(&["orchestration:read"], "fixture-reader");
    let denied = json_request(
        state.clone(),
        &reader,
        "GET",
        "/api/voice/cloud/keys",
        Value::Null,
    )
    .await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.body["requiredPermission"], "orchestration:operate");
    let denied = json_request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/openai/key",
        json!({"key":"fixture-blocked"}),
    )
    .await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.body["requiredPermission"], "settings:write");
    let invalid = json_request(
        state.clone(),
        &reader,
        "POST",
        "/api/voice/cloud/openai/key",
        json!({"key":"https://fixture.invalid"}),
    )
    .await;
    assert_eq!(invalid.status, 400);
    assert_eq!(
        json_request(
            state.clone(),
            &writer,
            "POST",
            "/api/voice/cloud/openai/key",
            json!({"key":" fixture-openai-key "})
        )
        .await
        .status,
        204
    );
    assert!(!fs::read_to_string(root.join("settings.json"))
        .unwrap()
        .contains("fixture-openai-key"));
    let requests = Arc::new(Mutex::new(vec![]));
    let captured = requests.clone();
    let (arm, armed) = tokio::sync::oneshot::channel();
    let(peer_stop,peer_stopped)=tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        armed.await.unwrap();
        for _ in 0..7 {
            let (mut socket, head, body) = provider_request(&listener).await;
            let is_catalog = head.starts_with("GET /api/v1/models?");
            let response = if is_catalog {
                json!({"data":[{"id":"fixture/transcribe","name":" Fixture ASR "}]})
            } else {
                json!({"text":" um, oh three "})
            };
            captured.lock().unwrap().push((head, body));
            let body = serde_json::to_vec(&response).unwrap();
            socket.write_all(format!("HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }
        tokio::select!{_=peer_stopped=>{},_=listener.accept()=>panic!("Rejected vault/authority operations must not open another provider connection")}
    });
    arm.send(()).unwrap();
    let first = request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/openai/transcribe",
        pcm(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body, json!({"text":"T3 Code"}));
    assert_eq!(
        json_request(
            state.clone(),
            &writer,
            "POST",
            "/api/voice/cloud/xai/key",
            json!({"key":"fixture-xai-key"})
        )
        .await
        .status,
        204
    );
    let second = request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/xai/transcribe",
        pcm(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(second.status, 200);
    assert_eq!(
        json_request(
            state.clone(),
            &writer,
            "POST",
            "/api/voice/cloud/openrouter/key",
            json!({"key":"fixture-router-key"})
        )
        .await
        .status,
        204
    );
    let models = json_request(
        state.clone(),
        &operator,
        "GET",
        "/api/voice/cloud/openrouter/models",
        Value::Null,
    )
    .await;
    assert_eq!(models.status, 200);
    assert_eq!(models.body["models"][0]["name"], "Fixture ASR");
    let router = request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/openrouter/transcribe?model=fixture/transcribe",
        pcm(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(router.status, 200);
    o3_server::settings::update(&state,&json!({"patch":{"secretProviders":{"vault":{"kind":"one_password","label":"Fixture Vault","config":{"account":"fixture.1password.com"}}}}})).unwrap();
    assert_eq!(
        json_request(
            state.clone(),
            &writer,
            "POST",
            "/api/voice/cloud/openai/key",
            json!({"key":{"provider":"vault","reference":"op://Fixture/Voice/key"}})
        )
        .await
        .status,
        204
    );
    // The unchanged public resolver read path still shares its original cache.
    let reference=o3_server::secret_resolver::SecretReference::decode(&json!({"provider":"vault","reference":"op://Fixture/Voice/key"})).unwrap();
    let providers=o3_server::settings::read(&state).unwrap()["secretProviders"].clone();
    assert_eq!(resolver.read(&providers,std::slice::from_ref(&reference)).await,vec![Some("fixture-vault-key".into())]);
    assert_eq!(resolver.read(&providers,std::slice::from_ref(&reference)).await,vec![Some("fixture-vault-key".into())]);
    assert_eq!(vault.calls.lock().unwrap().len(),1);
    let named = request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/openai/transcribe",
        pcm(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(named.status, 200);
    *vault.value.lock().unwrap() = None;
    let locked = request(
        state.clone(),
        &operator,
        "POST",
        "/api/voice/cloud/openai/transcribe",
        pcm(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(locked.status, 422);
    assert_eq!(
        locked.body,
        json!({"_tag":"CloudSpeechError","provider":"openai","reason":"unreadable-key"})
    );
    assert_eq!(vault.calls.lock().unwrap().len(), 3);
    // A label and an unknown config property do not change the decoded authority.
    *vault.value.lock().unwrap() = Some("fixture-label-stable-key".into());
    let stable_gate = Arc::new(Gate { entered: tokio::sync::Notify::new(), released: tokio::sync::Notify::new() });
    *vault.gate.lock().unwrap() = Some(stable_gate.clone());
    let pending_state = state.clone(); let pending_token = operator.clone();
    let stable = tokio::spawn(async move { request(pending_state, &pending_token, "POST", "/api/voice/cloud/openai/transcribe", pcm(), "application/octet-stream").await });
    tokio::time::timeout(Duration::from_secs(10), stable_gate.entered.notified()).await.expect("Prepared lookup witness");
    o3_server::settings::update(&state, &json!({"patch":{"secretProviders":{"vault":{"kind":"one_password","label":"Label only","config":{"account":"fixture.1password.com","ignored":"not-authority"}}}}})).unwrap();
    stable_gate.released.notify_one();
    assert_eq!(stable.await.unwrap().status, 200);
    *vault.gate.lock().unwrap() = None;
    // This is an intentional O3 security deviation from the measured native race.
    // The provider account changes while the real resolver owns the old decoded plan.
    *vault.value.lock().unwrap()=Some("fixture-race-old-key".into());
    let gate=Arc::new(Gate{entered:tokio::sync::Notify::new(),released:tokio::sync::Notify::new()});
    *vault.gate.lock().unwrap()=Some(gate.clone());
    let pending_state=state.clone();let pending_token=operator.clone();
    let pending=tokio::spawn(async move{request(pending_state,&pending_token,"POST","/api/voice/cloud/openai/transcribe",pcm(),"application/octet-stream").await});
    tokio::time::timeout(Duration::from_secs(10),gate.entered.notified()).await.expect("Prepared owner must reach only the vault lookup witness");
    o3_server::settings::update(&state,&json!({"patch":{"secretProviders":{"vault":{"kind":"one_password","label":"Renamed fixture vault","config":{"account":"new-account","ignored":"not-authority"}}}}})).unwrap();
    assert_eq!(o3_server::settings::read(&state).unwrap()["cloudSpeechCredentials"]["openai"],reference.to_value());
    gate.released.notify_one();
    let changed=pending.await.unwrap();assert_eq!(changed.status,422);assert_eq!(changed.body,json!({"_tag":"CloudSpeechError","provider":"openai","reason":"key-changed"}));
    *vault.gate.lock().unwrap()=None;
    // The next operation reads the new account before lookup and can use its fresh key.
    *vault.value.lock().unwrap() = Some("fixture-new-account-key".into());
    let fresh = request(state.clone(), &operator, "POST", "/api/voice/cloud/openai/transcribe", pcm(), "application/octet-stream").await;
    assert_eq!(fresh.status, 200);
    assert_eq!(vault.calls.lock().unwrap().last().unwrap()["config"]["account"], "new-account");
    peer_stop.send(()).unwrap();peer.await.unwrap();
    let keys = json_request(
        state.clone(),
        &operator,
        "GET",
        "/api/voice/cloud/keys",
        Value::Null,
    )
    .await;
    assert_eq!(
        keys.body,
        json!({"openai":true,"xai":true,"openrouter":true})
    );
    {
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 7);
    assert!(requests[0]
        .0
        .to_ascii_lowercase()
        .contains("authorization: bearer fixture-openai-key"));
    assert!(requests[1]
        .0
        .to_ascii_lowercase()
        .contains("authorization: bearer fixture-xai-key"));
    assert!(!requests[2]
        .0
        .to_ascii_lowercase()
        .contains("authorization:"));
    assert!(requests[3]
        .0
        .to_ascii_lowercase()
        .contains("authorization: bearer fixture-router-key"));
    assert!(requests[4]
        .0
        .to_ascii_lowercase()
        .contains("authorization: bearer fixture-vault-key"));
    assert!(requests[5].0.to_ascii_lowercase().contains("authorization: bearer fixture-label-stable-key"));
    assert!(requests[6].0.to_ascii_lowercase().contains("authorization: bearer fixture-new-account-key"));
    assert!(requests[0].0.starts_with("POST /v1/audio/transcriptions "));
    assert!(requests[1].0.starts_with("POST /v1/stt "));
    assert!(requests[2]
        .0
        .contains("output_modalities=transcription&zdr=true"));
    let xai = String::from_utf8_lossy(&requests[1].1);
    assert!(xai.contains("line\r\nterm"));
    assert!(xai.find("name=\"keyterm\"").unwrap() < xai.find("name=\"file\"").unwrap());
    assert!(xai.contains("Content-Type: audio/wav"));
    let router: Value = serde_json::from_slice(&requests[3].1).unwrap();
    assert_eq!(router["provider"], json!({"zdr":true}));
    assert!(router.get("keywords").is_none());
    }
    state.begin_shutdown();
    o3_server::speech_state::shutdown(&state).await.unwrap();
    }).await.unwrap();});
}
