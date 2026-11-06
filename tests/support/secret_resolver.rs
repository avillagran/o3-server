use super::*;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

fn reference(value: &Value) -> SecretReference {
    SecretReference::decode(value).unwrap()
}
fn fixtures() -> Value {
    serde_json::from_str(include_str!("../fixtures/secret-resolver-4275.json")).unwrap()
}

#[test]
fn native_reference_config_and_syntax_oracles() {
    let fixture = fixtures();
    let driver = OnePassword::system();
    for c in fixture["referenceCases"].as_array().unwrap() {
        let result = SecretReference::decode(&c["input"]);
        if c["invalid"] == true {
            assert!(result.is_none(), "reference {c}")
        } else {
            assert_eq!(
                result.map(|v| v.to_value()),
                Some(c["decoded"].clone()),
                "reference {c}"
            )
        }
    }
    for c in fixture["accountCases"].as_array().unwrap() {
        assert_eq!(
            driver.decode_config(&c["input"]),
            c.get("decoded").cloned(),
            "account {c}"
        )
    }
    for c in fixture["syntaxCases"].as_array().unwrap() {
        assert_eq!(
            driver.is_reference(c["reference"].as_str().unwrap()),
            c["valid"].as_bool().unwrap(),
            "syntax {c}"
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Call {
    kind: String,
    account: String,
    references: Vec<String>,
}
struct FakeDriver {
    kind: String,
    calls: Arc<Mutex<Vec<Call>>>,
    responses: Arc<Mutex<Value>>,
    gate: Option<Arc<Gate>>,
}
impl SecretDriver for FakeDriver {
    fn kind(&self) -> &str {
        &self.kind
    }
    fn decode_config(&self, value: &Value) -> Option<Value> {
        Some(json!({"account":value.get("account")?.as_str()?}))
    }
    fn is_reference(&self, reference: &str) -> bool {
        reference.starts_with("fake://")
    }
    fn read<'a>(&'a self, config: Value, references: Vec<String>) -> ReadFuture<'a> {
        Box::pin(async move {
            if let Some(gate) = &self.gate {
                if references.iter().any(|r| r == "fake://unlock") {
                    gate.started.fetch_add(1, Ordering::SeqCst);
                    gate.start.notify_one();
                    gate.release.notified().await;
                }
            }
            let account = config["account"].as_str().unwrap().to_owned();
            self.calls.lock().unwrap().push(Call {
                kind: self.kind.clone(),
                account: account.clone(),
                references: references.clone(),
            });
            references
                .iter()
                .map(|r| {
                    self.responses
                        .lock()
                        .unwrap()
                        .get(format!("{account}|{r}"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        })
    }
}
#[derive(Default)]
struct Gate {
    started: AtomicUsize,
    start: Notify,
    release: Notify,
}
type FakeResolver = (
    Arc<SecretResolver>,
    Arc<Mutex<Vec<Call>>>,
    Arc<Mutex<Value>>,
);
fn fake_resolver(gate: Option<Arc<Gate>>) -> FakeResolver {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let responses = Arc::new(Mutex::new(json!({})));
    let drivers = ["fake", "other"]
        .into_iter()
        .map(|kind| {
            Arc::new(FakeDriver {
                kind: kind.into(),
                calls: calls.clone(),
                responses: responses.clone(),
                gate: gate.clone(),
            }) as Arc<dyn SecretDriver>
        })
        .collect();
    (Arc::new(SecretResolver::new(drivers)), calls, responses)
}
#[tokio::test]
async fn native_resolver_sequence_oracles() {
    for c in fixtures()["coreCases"].as_array().unwrap() {
        let (resolver, calls, responses) = fake_resolver(None);
        let mut providers = c["providers"].clone();
        for event in c["events"].as_array().unwrap() {
            if let Some(v) = event.get("providers") {
                providers = v.clone()
            }
            if let Some(v) = event.get("responses") {
                *responses.lock().unwrap() = v.clone()
            }
            if let Some(v) = event.get("read") {
                let refs = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(reference)
                    .collect::<Vec<_>>();
                assert_eq!(
                    serde_json::to_value(resolver.read(&providers, &refs).await).unwrap(),
                    event["result"],
                    "{} read {event}",
                    c["label"]
                )
            }
            if let Some(v) = event.get("invalidate") {
                if v.is_null() {
                    resolver.invalidate(None).await
                } else {
                    let refs = v
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(reference)
                        .collect::<Vec<_>>();
                    resolver.invalidate(Some(&refs)).await
                }
            }
            if let Some(label) = event["label"].as_str() {
                assert_eq!(
                    SecretResolver::label(&providers, label),
                    event["result"].as_str().unwrap()
                )
            }
            assert_eq!(
                serde_json::to_value(calls.lock().unwrap().clone()).unwrap(),
                event["calls"],
                "{} calls {event}",
                c["label"]
            );
        }
    }
}

#[tokio::test]
async fn cached_hits_bypass_unlock_and_concurrent_misses_deduplicate() {
    let gate = Arc::new(Gate::default());
    let (resolver, calls, responses) = fake_resolver(Some(gate.clone()));
    *responses.lock().unwrap() = json!({"a|fake://cached":"cached","a|fake://unlock":"unlocked"});
    let providers = json!({"personal":{"kind":"fake","config":{"account":"a"}}});
    let cached = reference(&json!({"provider":"personal","reference":"fake://cached"}));
    let unlock = reference(&json!({"provider":"personal","reference":"fake://unlock"}));
    assert_eq!(
        resolver
            .read(&providers, std::slice::from_ref(&cached))
            .await,
        vec![Some("cached".into())]
    );
    let r = resolver.clone();
    let p = providers.clone();
    let u = unlock.clone();
    let first = tokio::spawn(async move { r.read(&p, &[u]).await });
    gate.start.notified().await;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(100),
            resolver.read(&providers, &[cached])
        )
        .await
        .unwrap(),
        vec![Some("cached".into())]
    );
    let r = resolver.clone();
    let p = providers.clone();
    let u = unlock.clone();
    let second = tokio::spawn(async move { r.read(&p, &[u.clone(), u]).await });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    gate.release.notify_one();
    assert_eq!(first.await.unwrap(), vec![Some("unlocked".into())]);
    assert_eq!(
        second.await.unwrap(),
        vec![Some("unlocked".into()), Some("unlocked".into())]
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(gate.started.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn invalidation_waits_for_unlock_then_forgets_result() {
    let gate = Arc::new(Gate::default());
    let (resolver, _, responses) = fake_resolver(Some(gate.clone()));
    *responses.lock().unwrap() = json!({"a|fake://unlock":"unlocked"});
    let providers = json!({"p":{"kind":"fake","config":{"account":"a"}}});
    let reference = reference(&json!({"provider":"p","reference":"fake://unlock"}));
    let r = resolver.clone();
    let p = providers.clone();
    let refs = vec![reference.clone()];
    let first = tokio::spawn(async move { r.read(&p, &refs).await });
    gate.start.notified().await;
    let r = resolver.clone();
    let invalidate = tokio::spawn(async move { r.invalidate(None).await });
    tokio::task::yield_now().await;
    assert!(!invalidate.is_finished());
    gate.release.notify_one();
    first.await.unwrap();
    invalidate.await.unwrap();
    let r = resolver.clone();
    let read = tokio::spawn(async move { r.read(&providers, &[reference]).await });
    gate.start.notified().await;
    assert_eq!(gate.started.load(Ordering::SeqCst), 2);
    gate.release.notify_one();
    read.await.unwrap();
}

struct PrivateOp {
    directory: PathBuf,
    binary: PathBuf,
}
impl PrivateOp {
    fn new(answers: &Value) -> Self {
        crate::test_sandbox::assert_isolated();
        let directory = std::env::temp_dir().join(format!("o3-secret-op-{}", uuid().unwrap()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let binary = directory.join("op");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/input/secret-op.py"),
            &binary,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            directory.join("case.json"),
            serde_json::to_vec(answers).unwrap(),
        )
        .unwrap();
        Self { directory, binary }
    }
    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
impl Drop for PrivateOp {
    fn drop(&mut self) {
        for file in ["op", "case.json", "calls.jsonl"] {
            let _ = std::fs::remove_file(self.directory.join(file));
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[tokio::test]
async fn native_one_password_process_traces() {
    for case in fixtures()["driverCases"].as_array().unwrap() {
        let fixture = PrivateOp::new(&case["answers"]);
        let driver = if case["answers"]["virtualTimeout"] == true {
            OnePassword::with_timeout(fixture.binary.clone(), Duration::from_secs(1))
        } else {
            OnePassword::with_binary(fixture.binary.clone())
        };
        let refs: Vec<String> = serde_json::from_value(case["references"].clone()).unwrap();
        let result = driver.read(case["config"].clone(), refs).await;
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            case["result"],
            "{} result",
            case["label"]
        );
        let calls = fixture.calls();
        let actual: Vec<Value> = calls
            .iter()
            .map(|call| {
                Value::Array(
                    call["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|arg| {
                            let template = arg.as_str().is_some_and(|value| {
                                let path = std::path::Path::new(value);
                                path.parent()
                                    .is_some_and(|parent| parent == std::env::temp_dir())
                                    && path.file_name().and_then(|name| name.to_str()).is_some_and(
                                        |name| {
                                            let Some(id) = name.strip_prefix("t3code-op-inject-")
                                            else {
                                                return false;
                                            };
                                            id.len() == 36
                                                && id.bytes().enumerate().all(|(index, byte)| {
                                                    if matches!(index, 8 | 13 | 18 | 23) {
                                                        byte == b'-'
                                                    } else {
                                                        byte.is_ascii_hexdigit()
                                                    }
                                                })
                                        },
                                    )
                            });
                            if template {
                                json!("TEMPLATE")
                            } else {
                                arg.clone()
                            }
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(json!(actual), case["calls"], "{} calls", case["label"]);
        let templates: Vec<_> = calls
            .iter()
            .filter_map(|v| v.get("template").cloned())
            .collect();
        assert_eq!(
            json!(templates),
            case["templates"],
            "{} templates",
            case["label"]
        );
        for call in calls {
            if let Some(mode) = call.get("templateMode") {
                assert_eq!(mode, 384);
                let path = call["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find_map(|v| {
                        v.as_str()
                            .filter(|s| s.starts_with("/tmp/t3code-op-inject-"))
                    })
                    .unwrap();
                assert!(
                    !std::path::Path::new(path).exists(),
                    "scoped template was retained"
                );
            }
        }
    }
}
#[tokio::test]
async fn batch_timeout_kills_process_removes_template_and_does_not_fallback() {
    let fixture = PrivateOp::new(
        &json!({"inject":{"hang":true},"read":{"op://a/b/c":{"stdout":"must not read"}}}),
    );
    let driver = OnePassword::with_timeout(fixture.binary.clone(), Duration::from_secs(1));
    let refs = vec!["op://a/b/c".to_owned(), "op://a/b/d".to_owned()];
    assert_eq!(
        driver.read(json!({"account":"work"}), refs).await,
        vec![None, None]
    );
    let calls = fixture.calls();
    assert_eq!(calls.len(), 1);
    let pid = calls[0]["pid"].as_i64().unwrap() as i32;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("timed-out op remained alive");
    let args = calls[0]["args"].as_array().unwrap();
    let path = args[args.iter().position(|v| v == "-i").unwrap() + 1]
        .as_str()
        .unwrap();
    assert!(!std::path::Path::new(path).exists());
}

#[tokio::test]
async fn single_timeout_and_missing_binary_fail_without_reading_host_tools() {
    let reference = "op://a/b/c".to_owned();
    let fixture = PrivateOp::new(&json!({"read":{(reference.clone()):{"hang":true}}}));
    let driver = OnePassword::with_timeout(fixture.binary.clone(), Duration::from_secs(1));
    assert_eq!(
        driver
            .read(json!({"account":"work"}), vec![reference.clone()])
            .await,
        vec![None]
    );
    assert_eq!(fixture.calls().len(), 1);
    let missing = OnePassword::with_binary(fixture.directory.join("missing-op"));
    assert_eq!(
        missing
            .read(json!({"account":"work"}), vec![reference])
            .await,
        vec![None]
    );
    assert_eq!(fixture.calls().len(), 1);
}
#[tokio::test]
async fn cancelling_batch_cleans_template_and_child() {
    let fixture = PrivateOp::new(&json!({"inject":{"hang":true}}));
    let driver = Arc::new(OnePassword::with_binary(fixture.binary.clone()));
    let d = driver.clone();
    let task = tokio::spawn(async move {
        d.read(
            json!({"account":"work"}),
            vec!["op://a/b/c".into(), "op://a/b/d".into()],
        )
        .await
    });
    let calls = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let calls = fixture.calls();
            if !calls.is_empty() {
                break calls;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let pid = calls[0]["pid"].as_i64().unwrap() as i32;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if unsafe { libc::kill(pid, 0) } != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled op remained alive");
    let args = calls[0]["args"].as_array().unwrap();
    let path = args[args.iter().position(|v| v == "-i").unwrap() + 1]
        .as_str()
        .unwrap();
    assert!(!std::path::Path::new(path).exists());
}
#[tokio::test]
async fn queued_reads_keep_their_own_configuration_snapshot() {
    let gate = Arc::new(Gate::default());
    let (resolver, calls, responses) = fake_resolver(Some(gate.clone()));
    *responses.lock().unwrap() = json!({"a|fake://unlock":"a","b|fake://unlock":"b"});
    let reference = reference(&json!({"provider":"p","reference":"fake://unlock"}));
    let providers_a = json!({"p":{"kind":"fake","config":{"account":"a"}}});
    let providers_b = json!({"p":{"kind":"fake","config":{"account":"b"}}});
    let r = resolver.clone();
    let p = providers_a.clone();
    let refs = vec![reference.clone()];
    let first = tokio::spawn(async move { r.read(&p, &refs).await });
    gate.start.notified().await;
    let r = resolver.clone();
    let refs = vec![reference.clone()];
    let second = tokio::spawn(async move { r.read(&providers_b, &refs).await });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    gate.release.notify_one();
    assert_eq!(first.await.unwrap(), vec![Some("a".into())]);
    gate.start.notified().await;
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(100),
            resolver.read(&providers_a, &[reference])
        )
        .await
        .unwrap(),
        vec![Some("a".into())]
    );
    gate.release.notify_one();
    assert_eq!(second.await.unwrap(), vec![Some("b".into())]);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.account.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[tokio::test]
async fn cancelling_resolver_unlock_releases_semaphore_without_caching() {
    let gate = Arc::new(Gate::default());
    let (resolver, calls, responses) = fake_resolver(Some(gate.clone()));
    *responses.lock().unwrap() = json!({"a|fake://unlock":"value"});
    let reference = reference(&json!({"provider":"p","reference":"fake://unlock"}));
    let providers = json!({"p":{"kind":"fake","config":{"account":"a"}}});
    let r = resolver.clone();
    let p = providers.clone();
    let refs = vec![reference.clone()];
    let first = tokio::spawn(async move { r.read(&p, &refs).await });
    gate.start.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_millis(100), resolver.invalidate(None))
        .await
        .unwrap();
    let r = resolver.clone();
    let second = tokio::spawn(async move { r.read(&providers, &[reference]).await });
    gate.start.notified().await;
    assert_eq!(gate.started.load(Ordering::SeqCst), 2);
    gate.release.notify_one();
    assert_eq!(second.await.unwrap(), vec![Some("value".into())]);
    assert_eq!(calls.lock().unwrap().len(), 1);
}
