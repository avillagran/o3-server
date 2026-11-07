//! Focused service acceptance; all filesystem effects below are explicit in-memory fixtures.
#[path = "support/sandbox.rs"]
mod sandbox;
use candidate::{Engine, Error, Loaded, Settings, Snapshot, Task, Unload};
use o3_server::{speech_catalog, speech_model_store, speech_service as candidate, speech_worker};
use serde_json::{json, Value};
use speech_model_store::{Backend, Body, FileInfo, Response, Store, Transfer, Transport};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
#[derive(Default)]
struct Memory {
    ready: Mutex<HashSet<String>>,
    requests: AtomicU64,
    started: tokio::sync::Notify,
}
impl Backend for Memory {
    fn directory(&self) -> &Path {
        Path::new("/fixture/models")
    }
    fn info(&self, path: &Path) -> Option<FileInfo> {
        speech_catalog::all()
            .iter()
            .find(|model| {
                path.file_name() == Some(std::ffi::OsStr::new(model.filename()))
                    && self.ready.lock().unwrap().contains(model.id())
            })
            .map(|model| FileInfo {
                regular: true,
                size: model.size(),
                mtime_ms: Some(1000),
                device: 1,
                inode: Some(2),
            })
    }
    fn read_marker(&self, _: &Path) -> Option<String> {
        None
    }
    fn write_marker(&self, _: &Path, _: &str) -> Result<(), speech_model_store::Error> {
        Ok(())
    }
    fn digest<'a>(&'a self, path: &'a Path) -> speech_model_store::Task<'a, Option<String>> {
        Box::pin(async move {
            speech_catalog::all()
                .iter()
                .find(|model| {
                    path.file_name() == Some(std::ffi::OsStr::new(model.filename()))
                        && self.ready.lock().unwrap().contains(model.id())
                })
                .map(|model| model.sha256().into())
        })
    }
    fn stage<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
    ) -> speech_model_store::Task<'a, Result<Box<dyn Transfer>, speech_model_store::Error>> {
        Box::pin(async { Ok(Box::new(Stage) as Box<dyn Transfer>) })
    }
    fn remove<'a>(
        &'a self,
        model: &'a speech_catalog::Model,
    ) -> speech_model_store::Task<'a, Result<(), speech_model_store::Error>> {
        Box::pin(async move {
            self.ready.lock().unwrap().remove(model.id());
            Ok(())
        })
    }
}
struct Stage;
impl Transfer for Stage {
    fn path(&self) -> &Path {
        Path::new("/fixture/owned.part")
    }
    fn offset(&self) -> u64 {
        0
    }
    fn restart(&mut self) -> speech_model_store::Task<'_, Result<(), speech_model_store::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn append<'a>(
        &'a mut self,
        _: &'a [u8],
    ) -> speech_model_store::Task<'a, Result<(), speech_model_store::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn publish<'a>(
        &'a mut self,
        _: &'a Path,
    ) -> speech_model_store::Task<'a, Result<(), speech_model_store::Error>> {
        Box::pin(async { unreachable!("held synthetic HTTP cannot publish") })
    }
}
struct HeldBody;
impl Body for HeldBody {
    fn next(
        &mut self,
    ) -> speech_model_store::Task<'_, Result<Option<Vec<u8>>, speech_model_store::Error>> {
        Box::pin(std::future::pending())
    }
}
impl Transport for Memory {
    fn get<'a>(
        &'a self,
        _: &'a str,
        _: u64,
    ) -> speech_model_store::Task<'a, Result<Response, speech_model_store::Error>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.started.notify_waiters();
            Ok(Response {
                status: 200,
                content_range: None,
                content_length: None,
                body: Box::new(HeldBody),
            })
        })
    }
}
struct Authority {
    snapshot: Mutex<Snapshot>,
    allowed: AtomicBool,
    patches: Mutex<Vec<Value>>,
    events: Arc<Mutex<Vec<Value>>>,
}
impl Settings for Authority {
    fn read<'a>(&'a self, _: &'static str) -> Task<'a, Result<Snapshot, Error>> {
        Box::pin(async { Ok(self.snapshot.lock().unwrap().clone()) })
    }
    fn patch<'a>(&'a self, operation: &'static str, patch: Value) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            if !self.allowed.load(Ordering::SeqCst) {
                return Err(Error::Operation {
                    operation,
                    cause: "revoked".into(),
                });
            }
            let mut settings = self.snapshot.lock().unwrap();
            if let Some(id) = patch["speechModelId"].as_str() {
                settings.model_id = id.into();
            }
            if let Some(value) = patch["speechAcceleration"].as_str() {
                settings.acceleration = value.into();
            }
            if let Some(value) = patch["speechModelUnloadTimeout"].as_str() {
                settings.unload = match value {
                    "immediately" => Unload::Immediately,
                    "min_2" => Unload::Min2,
                    "min_5" => Unload::Min5,
                    "min_10" => Unload::Min10,
                    "min_15" => Unload::Min15,
                    "hour_1" => Unload::Hour1,
                    "never" => Unload::Never,
                    _ => panic!("invalid synthetic settings patch"),
                };
            }
            self.events
                .lock()
                .unwrap()
                .push(json!({"kind":"settings","patch":patch}));
            self.patches.lock().unwrap().push(patch);
            Ok(())
        })
    }
}
struct NoEngine {
    discoveries: AtomicU64,
}
impl Engine for NoEngine {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async {
            Err(Error::Operation {
                operation: "fixture load",
                cause: "not installed".into(),
            })
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async {
            self.discoveries.fetch_add(1, Ordering::SeqCst);
            Ok(json!([]))
        })
    }
}
fn fixture() -> (
    candidate::Service,
    Arc<Authority>,
    Arc<Memory>,
    Arc<NoEngine>,
) {
    sandbox::assert_isolated();
    let memory = Arc::new(Memory::default());
    let tiny = speech_catalog::get("handy-computer/moonshine-tiny-gguf").unwrap();
    memory.ready.lock().unwrap().insert(tiny.id().into());
    let authority = Arc::new(Authority {
        snapshot: Mutex::new(Snapshot {
            model_id: tiny.id().into(),
            acceleration: "cpu".into(),
            unload: Unload::Min2,
        }),
        allowed: AtomicBool::new(true),
        patches: Mutex::new(vec![]),
        events: Arc::new(Mutex::new(vec![])),
    });
    let engine = Arc::new(NoEngine {
        discoveries: AtomicU64::new(0),
    });
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory.clone())),
        engine.clone(),
        Arc::new(candidate::WallClock),
        None,
    );
    (service, authority, memory, engine)
}
#[tokio::test]
async fn fresh_settings_selection_and_revoked_commit() {
    let (service, authority, _memory, engine) = fixture();
    let call = service.bind(authority.clone());
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    let status = call.status().await.unwrap();
    assert_eq!(status, golden["cases"][0]["observations"][0]["value"]);
    assert!(matches!(
        call.select("missing-model").await,
        Err(Error::ModelNotFound { .. })
    ));
    let default = speech_catalog::all()[0].id();
    let models = call.select(default).await.unwrap();
    assert_eq!(models, golden["cases"][0]["observations"][2]["value"]);
    assert!(models["models"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model["active"] == false));
    authority.allowed.store(false, Ordering::SeqCst);
    let before = authority.snapshot.lock().unwrap().model_id.clone();
    assert!(
        matches!(call.select("handy-computer/moonshine-tiny-gguf").await,Err(Error::Operation{cause,..}) if cause=="revoked")
    );
    assert_eq!(authority.snapshot.lock().unwrap().model_id, before);
    call.status().await.unwrap();
    assert_eq!(engine.discoveries.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
    assert!(matches!(
        call.select(default).await,
        Err(Error::Busy { .. })
    ));
}
#[tokio::test]
async fn owned_download_deduplicates_and_cancellation_releases_slot() {
    let (service, authority, memory, _) = fixture();
    let id = speech_catalog::all()[0].id();
    let call = service.bind(authority.clone());
    let changed = memory.started.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    let owner = service.clone();
    let auth = authority.clone();
    let first = tokio::spawn(async move { owner.bind(auth).download(id).await });
    changed.await;
    assert_eq!(memory.requests.load(Ordering::SeqCst), 1);
    assert!(matches!(call.select(id).await, Err(Error::Busy { .. })));
    let mut second = Box::pin(call.download(id));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(second.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    // Dropping the original caller does not cancel the shared transfer owner.
    first.abort();
    let _ = first.await;
    let listed = call.models(false).await.unwrap();
    assert_eq!(listed["models"][0]["state"], "downloading");
    assert_eq!(memory.requests.load(Ordering::SeqCst), 1);
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    assert_eq!(
        call.cancel_download("wrong-id").await.unwrap(),
        golden["cases"][2]["observations"][2]["value"]
    );
    assert_eq!(
        call.models(false).await.unwrap()["models"][0]["state"],
        "downloading"
    );
    assert_eq!(
        call.cancel_download(id).await.unwrap(),
        golden["cases"][2]["observations"][3]["value"]
    );
    assert_eq!(
        second.await.unwrap(),
        golden["cases"][2]["observations"][5]["value"]
    );
    assert_eq!(
        call.models(false).await.unwrap()["models"][0]["state"],
        "downloadable"
    );
    call.select(id).await.unwrap();
    service.shutdown().await.unwrap();
}

struct VirtualRuntime {
    events: Arc<Mutex<Vec<Value>>>,
    closed: AtomicBool,
    unknown: bool,
}
impl candidate::ModelRuntime for VirtualRuntime {
    fn close(&self) -> Task<'_, speech_worker::Cleanup> {
        Box::pin(async move {
            if !self.closed.swap(true, Ordering::SeqCst) {
                self.events.lock().unwrap().push(json!({"kind":"dispose"}));
            }
            if self.unknown {
                speech_worker::Cleanup::Unknown
            } else {
                speech_worker::Cleanup::Confirmed { graceful: true }
            }
        })
    }
}
struct VirtualEngine {
    events: Arc<Mutex<Vec<Value>>>,
    unknown: bool,
}
impl Engine for VirtualEngine {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        path: &'a Path,
        acceleration: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async move {
            self.events
                .lock()
                .unwrap()
                .push(json!({"kind":"load","path":path,"acceleration":acceleration}));
            Ok(Loaded {
                runtime: Arc::new(VirtualRuntime {
                    events: self.events.clone(),
                    closed: AtomicBool::new(false),
                    unknown: self.unknown,
                }),
                metadata: json!({"backend":"CPU"}),
            })
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async {
            self.events.lock().unwrap().push(json!({"kind":"discover"}));
            Ok(json!([]))
        })
    }
}
struct VirtualClock(tokio::sync::watch::Sender<i64>);
impl VirtualClock {
    fn new() -> Self {
        Self(tokio::sync::watch::channel(0).0)
    }
    fn advance(&self, time: i64) {
        self.0.send_replace(time);
    }
}
impl candidate::Clock for VirtualClock {
    fn now(&self) -> i64 {
        *self.0.borrow()
    }
    fn sleep_until(&self, deadline: i64) -> Task<'_, ()> {
        let mut changes = self.0.subscribe();
        Box::pin(async move {
            loop {
                if *changes.borrow_and_update() >= deadline {
                    break;
                }
                if changes.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}
fn virtual_fixture(
    unknown: bool,
) -> (
    candidate::Service,
    Arc<Authority>,
    Arc<Memory>,
    Arc<VirtualClock>,
) {
    let (_unused, authority, memory, _engine) = fixture();
    let engine = Arc::new(VirtualEngine {
        events: authority.events.clone(),
        unknown,
    });
    let clock = Arc::new(VirtualClock::new());
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory.clone())),
        engine,
        clock.clone(),
        None,
    );
    (service, authority, memory, clock)
}
async fn flush() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}
#[tokio::test]
async fn whole_service_prepare_unload_and_selection_order() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    let (service, authority, _memory, clock) = virtual_fixture(false);
    let call = service.bind(authority.clone());
    call.prepare().await.unwrap();
    assert_eq!(
        json!(*authority.events.lock().unwrap()),
        golden["cases"][1]["observations"][1]["events"]
    );
    clock.advance(120_000);
    flush().await;
    assert_eq!(
        json!(*authority.events.lock().unwrap()),
        golden["cases"][1]["observations"][2]["events"]
    );
    clock.advance(300_000);
    flush().await;
    assert_eq!(
        json!(*authority.events.lock().unwrap()),
        golden["cases"][1]["observations"][3]["events"]
    );
    service.shutdown().await.unwrap();
    let (service, authority, _memory, _clock) = virtual_fixture(false);
    let call = service.bind(authority.clone());
    call.prepare().await.unwrap();
    call.select(speech_catalog::all()[0].id()).await.unwrap();
    assert_eq!(
        json!(*authority.events.lock().unwrap()),
        golden["cases"][4]["events"]
    );
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn removal_uses_catalog_order_and_never_timeout_is_sticky() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    let (service, authority, memory, _clock) = virtual_fixture(false);
    let call = service.bind(authority.clone());
    memory
        .ready
        .lock()
        .unwrap()
        .insert(speech_catalog::all()[0].id().into());
    call.prepare().await.unwrap();
    let models = call
        .remove("handy-computer/moonshine-tiny-gguf")
        .await
        .unwrap();
    assert_eq!(models, golden["cases"][3]["observations"][1]["value"]);
    service.shutdown().await.unwrap();
    let (service, authority, _memory, clock) = virtual_fixture(false);
    let call = service.bind(authority.clone());
    call.prepare().await.unwrap();
    assert_eq!(
        call.update_unload(Unload::Never).await.unwrap(),
        golden["cases"][5]["observations"][1]["value"]
    );
    let before = authority.events.lock().unwrap().clone();
    clock.advance(2 * 60 * 60 * 1000);
    flush().await;
    assert_eq!(*authority.events.lock().unwrap(), before);
    service.shutdown().await.unwrap();
    assert_eq!(
        authority.events.lock().unwrap().last(),
        Some(&json!({"kind":"dispose"}))
    );
}
#[tokio::test]
async fn unknown_cleanup_blocks_reuse_and_stays_unknown() {
    let (service, authority, _memory, _clock) = virtual_fixture(true);
    let call = service.bind(authority.clone());
    call.prepare().await.unwrap();
    assert_eq!(
        call.select(speech_catalog::all()[0].id()).await,
        Err(Error::CleanupUnknown)
    );
    assert_eq!(call.prepare().await, Err(Error::CleanupUnknown));
    assert_eq!(service.shutdown().await, Err(Error::CleanupUnknown));
    assert!(authority.patches.lock().unwrap().is_empty());
}

struct Gate {
    open: tokio::sync::watch::Sender<bool>,
    entered: tokio::sync::Notify,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: tokio::sync::watch::channel(false).0,
            entered: tokio::sync::Notify::new(),
        })
    }
    async fn wait(&self) {
        let mut state = self.open.subscribe();
        self.entered.notify_waiters();
        while !*state.borrow_and_update() {
            state.changed().await.unwrap();
        }
    }
}
struct ControlledRuntime {
    gate: Option<Arc<Gate>>,
    events: Arc<Mutex<Vec<Value>>>,
}
impl candidate::ModelRuntime for ControlledRuntime {
    fn close(&self) -> Task<'_, speech_worker::Cleanup> {
        Box::pin(async move {
            if let Some(gate) = &self.gate {
                gate.wait().await;
            }
            self.events.lock().unwrap().push(json!({"kind":"dispose"}));
            speech_worker::Cleanup::Confirmed { graceful: true }
        })
    }
}
struct ControlledEngine {
    load: Option<Arc<Gate>>,
    close: Option<Arc<Gate>>,
    events: Arc<Mutex<Vec<Value>>>,
}
impl Engine for ControlledEngine {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async move {
            if let Some(gate) = &self.load {
                gate.wait().await;
            }
            self.events.lock().unwrap().push(json!({"kind":"load"}));
            Ok(Loaded {
                runtime: Arc::new(ControlledRuntime {
                    gate: self.close.clone(),
                    events: self.events.clone(),
                }),
                metadata: json!({}),
            })
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async { Ok(json!([])) })
    }
}
#[tokio::test]
async fn authority_is_rechecked_after_asynchronous_disposal() {
    let (_unused, authority, memory, _) = fixture();
    let gate = Gate::new();
    let engine = Arc::new(ControlledEngine {
        load: None,
        close: Some(gate.clone()),
        events: authority.events.clone(),
    });
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory)),
        engine,
        Arc::new(candidate::WallClock),
        None,
    );
    service.bind(authority.clone()).prepare().await.unwrap();
    let entered = gate.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let owner = service.clone();
    let auth = authority.clone();
    let selection =
        tokio::spawn(async move { owner.bind(auth).select(speech_catalog::all()[0].id()).await });
    entered.await;
    authority.allowed.store(false, Ordering::SeqCst);
    gate.open.send_replace(true);
    assert!(matches!(selection.await.unwrap(),Err(Error::Operation{cause,..}) if cause=="revoked"));
    assert!(authority.patches.lock().unwrap().is_empty());
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn shutdown_fences_a_cancelled_callers_pending_load() {
    let (_unused, authority, memory, _) = fixture();
    let gate = Gate::new();
    let engine = Arc::new(ControlledEngine {
        load: Some(gate.clone()),
        close: None,
        events: authority.events.clone(),
    });
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory)),
        engine,
        Arc::new(candidate::WallClock),
        None,
    );
    let entered = gate.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let owner = service.clone();
    let auth = authority.clone();
    let caller = tokio::spawn(async move { owner.bind(auth).prepare().await });
    entered.await;
    caller.abort();
    let _ = caller.await;
    let mut shutdown = Box::pin(service.shutdown());
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(shutdown.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    gate.open.send_replace(true);
    shutdown.await.unwrap();
    assert_eq!(
        *authority.events.lock().unwrap(),
        vec![json!({"kind":"load"}), json!({"kind":"dispose"})]
    );
    assert!(matches!(
        service.bind(authority).prepare().await,
        Err(Error::Busy { .. })
    ));
}
