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
            .chain(std::iter::once(speech_catalog::silero()))
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
                .chain(std::iter::once(speech_catalog::silero()))
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
    assert_eq!(memory.requests.load(Ordering::SeqCst), 2);
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
    assert_eq!(memory.requests.load(Ordering::SeqCst), 2);
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

#[tokio::test]
async fn silero_recording_timeout_backoff_and_installation_reset() {
    use candidate::silence::{Purpose, Silence};
    let (_service, _authority, memory, clock) = virtual_fixture(false);
    let silence = Silence::new(
        Arc::new(Store::new(memory.clone(), memory.clone())),
        clock.clone(),
    );
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    let mut recording = silence.acquire(Purpose::Recording, receiver.clone());
    std::future::poll_fn(|context| {
        assert!(recording.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(memory.requests.load(Ordering::SeqCst), 1);
    clock.advance(15_000);
    assert_eq!(recording.await.unwrap(), None);
    assert_eq!(
        silence
            .acquire(Purpose::Recording, receiver.clone())
            .await
            .unwrap(),
        None
    );
    assert_eq!(memory.requests.load(Ordering::SeqCst), 1);
    let vad = speech_catalog::silero();
    memory.ready.lock().unwrap().insert(vad.id().into());
    assert!(silence
        .acquire(Purpose::Installation, receiver.clone())
        .await
        .unwrap()
        .is_some());
    memory.ready.lock().unwrap().remove(vad.id());
    let mut next = silence.acquire(Purpose::Recording, receiver);
    std::future::poll_fn(|context| {
        assert!(next.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(memory.requests.load(Ordering::SeqCst), 2);
    cancel.send_replace(true);
    assert_eq!(next.await, Err(speech_model_store::Error::Cancelled));
}
#[tokio::test]
async fn cached_main_download_still_waits_for_optional_silero_deadline() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    assert_eq!(speech_catalog::silero().as_value(), &golden["silero"]);
    assert!(speech_catalog::get(speech_catalog::silero().id()).is_none());
    let (service, authority, memory, clock) = virtual_fixture(false);
    let call = service.bind(authority);
    let entered = memory.started.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let mut download = Box::pin(call.download("handy-computer/moonshine-tiny-gguf"));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(download.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    entered.await;
    // Main model was verified from the explicit virtual backend; only VAD uses HTTP.
    assert_eq!(memory.requests.load(Ordering::SeqCst), 1);
    clock.advance(120_000);
    let models = download.await.unwrap();
    assert_eq!(models, golden["cases"][7]["observations"][0]["value"]);
    service.shutdown().await.unwrap();
}

#[path = "speech/service-process.rs"]
mod process;
#[test]
fn actual_service_factory_transcription_and_cpu_memo_match_native() {
    if process::fresh("actual_service_factory_transcription_and_cpu_memo_match_native") {
        return;
    }
    process::runtime().block_on(async {
        tokio::spawn(async {
            process::assert_stack();
            let (_unused, authority, memory, _) = fixture();
            authority.snapshot.lock().unwrap().acceleration = "auto".into();
            let engine = process::Engine::new("gpu-inference");
            let service = candidate::Service::new(
                Arc::new(Store::new(memory.clone(), memory)),
                engine.clone(),
                Arc::new(candidate::WallClock),
                None,
            );
            let call = service.bind(authority.clone());
            let golden: Value =
                serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
            let case = golden["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["name"] == "buffered-automatic-cpu-retry-and-setting-memo-fence")
                .unwrap();
            for (index, acceleration) in ["auto", "auto", "cpu", "auto"].iter().enumerate() {
                authority.snapshot.lock().unwrap().acceleration = (*acceleration).into();
                let options = o3_server::speech_request::TranscriptionOptions::decode(
                    case["observations"][0]["options"].clone(),
                )
                .unwrap();
                assert_eq!(
                    json!(call
                        .transcribe(0.25f32.to_le_bytes().to_vec(), options)
                        .await
                        .unwrap()),
                    case["observations"][index + 1]["value"]
                );
            }
            assert_eq!(engine.placements(), vec!["auto", "cpu", "auto", "cpu"]);
            assert_eq!(
                engine
                    .events()
                    .iter()
                    .filter(|event| event["kind"] == "transcribe")
                    .count(),
                6
            );
            service.shutdown().await.unwrap();
        })
        .await
        .unwrap();
    });
}
#[test]
fn actual_service_cancelled_inference_is_fenced_after_caller_disappears() {
    if process::fresh("actual_service_cancelled_inference_is_fenced_after_caller_disappears") {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
  process::assert_stack();let (_unused,authority,memory,_)=fixture();let engine=process::Engine::new("hang");
  let service=candidate::Service::new(Arc::new(Store::new(memory.clone(),memory)),engine.clone(),Arc::new(candidate::WallClock),None);
  // Cold runtime capture is readiness work, outside this inference-entry witness.
  service.bind(authority.clone()).prepare().await.unwrap();assert_eq!(engine.placements(),vec!["cpu"]);
  let owner=service.clone();let auth=authority.clone();
  let mut caller=tokio::spawn(async move {owner.bind(auth).transcribe(0.25f32.to_le_bytes().to_vec(),o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":false,"speechSilenceFiltering":false})).unwrap()).await});
  let pid=tokio::select! {pid=engine.entered()=>pid,result=&mut caller=>panic!("Hung inference completed before its entry witness: {result:?}")};let exit=process::exit(pid);
  assert_eq!(service.bind(authority).status().await.unwrap()["state"],"transcribing");
  caller.abort();let _=caller.await;service.shutdown().await.unwrap();
  let _event=tokio::time::timeout(std::time::Duration::from_secs(2),exit.readable()).await.unwrap().unwrap();
  assert_eq!(engine.events().iter().filter(|event|event["kind"]=="transcribe").count(),1);
 }).await.unwrap();});
}
#[tokio::test]
async fn buffered_invalid_pcm_and_missing_model_match_whole_service_errors() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/speech-service-4314.json")).unwrap();
    let case = golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "buffered-invalid-audio-and-missing-model")
        .unwrap();
    let (service, authority, memory, engine) = fixture();
    let call = service.bind(authority);
    for (index, body) in [vec![], vec![0; 3], f32::NAN.to_le_bytes().to_vec()]
        .into_iter()
        .enumerate()
    {
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"auto","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":false,"speechSilenceFiltering":false})).unwrap();
        match call.transcribe(body, options).await.unwrap_err() {
            Error::InvalidAudio { message, .. } => {
                assert_eq!(json!(message), case["observations"][index]["message"])
            }
            other => panic!("unexpected typed error {other:?}"),
        }
    }
    memory.ready.lock().unwrap().clear();
    let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"auto","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":false,"speechSilenceFiltering":false})).unwrap();
    assert!(matches!(
        call.transcribe(0.25f32.to_le_bytes().to_vec(), options)
            .await,
        Err(Error::ModelNotFound { .. })
    ));
    assert_eq!(engine.discoveries.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

struct CancellableAdmission {
    entered: tokio::sync::Notify,
    cancelled: std::sync::atomic::AtomicUsize,
}
impl Engine for CancellableAdmission {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async { panic!("Preparation must pass its owned cancellation authority") })
    }
    fn load_cancellable<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async move {
            self.entered.notify_waiters();
            while !*cancel.borrow() {
                if cancel.changed().await.is_err() {
                    break;
                }
            }
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Err(Error::Cancelled)
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async { Ok(json!([])) })
    }
}
#[tokio::test]
async fn preparation_deadline_and_dropped_caller_signal_the_owned_loader() {
    sandbox::assert_isolated();
    let (_unused, authority, memory, _) = fixture();
    let engine = Arc::new(CancellableAdmission {
        entered: tokio::sync::Notify::new(),
        cancelled: std::sync::atomic::AtomicUsize::new(0),
    });
    let clock = Arc::new(VirtualClock::new());
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory)),
        engine.clone(),
        clock.clone(),
        None,
    );
    for deadline in [true, false] {
        let entered = engine.entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();
        let owner = service.clone();
        let auth = authority.clone();
        let caller = tokio::spawn(async move { owner.bind(auth).prepare().await });
        entered.await;
        if deadline {
            clock.advance(120_000);
            assert!(matches!(
                caller.await.unwrap(),
                Err(Error::Operation {
                    operation: "model preparation",
                    ..
                })
            ));
        } else {
            caller.abort();
            let _ = caller.await;
        }
    }
    // Shutdown is a completion barrier for the still-owned cancelled job.
    service.shutdown().await.unwrap();
    assert_eq!(engine.cancelled.load(Ordering::SeqCst), 2);
    assert!(!authority
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "load"));
}

#[tokio::test]
async fn cached_engine_requires_a_current_installed_model_receipt() {
    sandbox::assert_isolated();
    let (service, authority, memory, _) = virtual_fixture(false);
    let call = service.bind(authority.clone());
    call.prepare().await.unwrap();
    let model_id = authority.snapshot.lock().unwrap().model_id.clone();
    memory.ready.lock().unwrap().remove(&model_id);
    assert_eq!(
        call.prepare().await,
        Err(Error::ModelNotFound {
            model_id: model_id.clone()
        })
    );
    let options = o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"auto","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":false,"speechSilenceFiltering":false})).unwrap();
    assert_eq!(
        call.transcribe(0.25f32.to_le_bytes().to_vec(), options)
            .await,
        Err(Error::ModelNotFound {
            model_id: model_id.clone()
        })
    );
    memory.ready.lock().unwrap().insert(model_id);
    call.prepare().await.unwrap();
    assert_eq!(
        authority
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "load")
            .count(),
        1
    );
    service.shutdown().await.unwrap();
}

#[test]
fn actual_stream_cpu_replay_revisions_and_scope_reservation_match_native() {
    if process::fresh("actual_stream_cpu_replay_revisions_and_scope_reservation_match_native") {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
        process::assert_stack();
        let golden:Value=serde_json::from_str(include_str!("fixtures/speech-service-stream-4314.json")).unwrap();
        let trace=golden["cases"].as_array().unwrap().iter().find(|case|case["name"]=="stream-native-cpu-replay-revision-and-reservation").unwrap();
        let (_unused,authority,memory,_)=fixture();authority.snapshot.lock().unwrap().acceleration="auto".into();
        let engine=process::Engine::new("gpu-stream");
        let service=candidate::Service::new(Arc::new(Store::new(memory.clone(),memory)),engine.clone(),Arc::new(candidate::WallClock),None);
        let call=service.bind(authority);
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":true,"speechCustomWords":[{"term":"O3","aliases":["oh three"]}],"speechSilenceFiltering":false})).unwrap();
        let stream=call.start_stream(options).await.unwrap();
        assert!(matches!(call.prepare().await,Err(Error::Busy{..})));
        for index in 1..=3 {assert_eq!(stream.feed([0.25f32.to_le_bytes();512].concat()).await.unwrap(),trace["observations"][index]["value"]);}
        assert_eq!(json!(stream.finish().await.unwrap()),trace["observations"][4]["value"]);
        assert!(matches!(stream.feed([0.25f32.to_le_bytes();512].concat()).await,Err(Error::Operation{operation:"streaming transcription",..})));
        assert!(matches!(call.prepare().await,Err(Error::Busy{..})));
        stream.close().await.unwrap();
        call.prepare().await.unwrap();
        assert_eq!(engine.placements(),vec!["auto","cpu"]);
        assert_eq!(engine.events().iter().filter(|event|event["kind"]=="feed").count(),5);
        service.shutdown().await.unwrap();
    }).await.unwrap();});
}
#[test]
fn actual_unfinished_stream_reset_retains_engine_and_invalid_chunks_are_recoverable() {
    if process::fresh(
        "actual_unfinished_stream_reset_retains_engine_and_invalid_chunks_are_recoverable",
    ) {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
        process::assert_stack();let (_unused,authority,memory,_)=fixture();let engine=process::Engine::new("stream");
        let service=candidate::Service::new(Arc::new(Store::new(memory.clone(),memory)),engine.clone(),Arc::new(candidate::WallClock),None);
        let call=service.bind(authority);
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})).unwrap();
        let stream=call.start_stream(options).await.unwrap();
        for bytes in [vec![],vec![0;3],vec![0;64_004]] {assert!(matches!(stream.feed(bytes).await,Err(Error::InvalidAudio{..})));}
        assert_eq!(stream.feed(0.25f32.to_le_bytes().to_vec()).await.unwrap()["revision"],1);
        let pid=engine.events().iter().find(|event|event["kind"]=="load").unwrap()["pid"].as_u64().unwrap() as u32;
        let exit=process::exit(pid);
        stream.close().await.unwrap();
        call.prepare().await.unwrap();
        assert_eq!(engine.placements(),vec!["cpu"]);
        assert_eq!(engine.events().iter().filter(|event|event["kind"]=="stream-reset").count(),1);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),exit.readable()).await.is_err());
        service.shutdown().await.unwrap();
        let _exit=tokio::time::timeout(std::time::Duration::from_secs(2),exit.readable()).await.unwrap().unwrap();
    }).await.unwrap();});
}
#[test]
fn actual_hung_stream_scope_close_fences_the_owned_worker_and_releases_permit() {
    if process::fresh("actual_hung_stream_scope_close_fences_the_owned_worker_and_releases_permit")
    {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
        process::assert_stack();let (_unused,authority,memory,_)=fixture();let engine=process::Engine::new("stream-hang");
        let service=candidate::Service::new(Arc::new(Store::new(memory.clone(),memory)),engine.clone(),Arc::new(candidate::WallClock),None);
        let call=service.bind(authority);
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})).unwrap();
        let stream=call.start_stream(options).await.unwrap();let feeding=stream.clone();
        let caller=tokio::spawn(async move {feeding.feed([0.25f32.to_le_bytes();512].concat()).await});
        let pid=engine.entered().await;let exit=process::exit(pid);
        assert!(matches!(stream.feed(0.25f32.to_le_bytes().to_vec()).await,Err(Error::Operation{..})));
        tokio::time::timeout(std::time::Duration::from_millis(5000),stream.close()).await.unwrap().unwrap();
        assert!(caller.await.unwrap().is_err());
        let _exit=tokio::time::timeout(std::time::Duration::from_secs(2),exit.readable()).await.unwrap().unwrap();
        call.prepare().await.unwrap();assert_eq!(engine.placements().len(),2);
        service.shutdown().await.unwrap();
    }).await.unwrap();});
}

#[test]
fn actual_stream_retains_at_most_native_recording_budget() {
    if process::fresh("actual_stream_retains_at_most_native_recording_budget") {
        return;
    }
    process::runtime().block_on(async {tokio::spawn(async {
        process::assert_stack();let (_unused,authority,memory,_)=fixture();let engine=process::Engine::new("stream");
        let service=candidate::Service::new(Arc::new(Store::new(memory.clone(),memory)),engine,Arc::new(candidate::WallClock),None);
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})).unwrap();
        let stream=service.bind(authority).start_stream(options).await.unwrap();
        for _ in 0..300 {stream.feed(vec![0;64_000]).await.unwrap();}
        assert_eq!(stream.feed(vec![0;4]).await,Err(Error::InvalidAudio{byte_length:4,message:"Speech stream audio limit exceeded."}));
        stream.finish().await.unwrap();stream.close().await.unwrap();service.shutdown().await.unwrap();
    }).await.unwrap();});
}

struct UncertainInferenceRuntime(Arc<std::sync::atomic::AtomicUsize>);
impl candidate::ModelRuntime for UncertainInferenceRuntime {
    fn close(&self) -> Task<'_, speech_worker::Cleanup> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            speech_worker::Cleanup::Confirmed { graceful: false }
        })
    }
    fn infer<'a>(
        &'a self,
        _: &'a o3_server::speech_kernel::Pcm,
        _: &'a str,
        _: &'a o3_server::speech_request::TranscriptionOptions,
    ) -> Task<'a, Result<String, Error>> {
        Box::pin(async { Err(Error::CleanupUnknown) })
    }
    fn begin_stream<'a>(
        &'a self,
        _: &'a str,
        _: &'a o3_server::speech_request::TranscriptionOptions,
    ) -> Task<'a, Result<(), Error>> {
        Box::pin(async { Err(Error::CleanupUnknown) })
    }
}
struct UncertainInferenceEngine {
    loads: std::sync::atomic::AtomicUsize,
    closes: Arc<std::sync::atomic::AtomicUsize>,
}
impl Engine for UncertainInferenceEngine {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(Loaded {
                runtime: Arc::new(UncertainInferenceRuntime(self.closes.clone())),
                metadata: json!({"backend":"fixture-gpu","supportsStreaming":true,"supportsLongForm":true}),
            })
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async { Ok(json!([])) })
    }
}
#[tokio::test]
async fn uncertain_runtime_outcomes_never_retry_or_clear_ownership_claims() {
    sandbox::assert_isolated();
    for stream in [false, true] {
        let (_unused, authority, memory, _) = fixture();
        authority.snapshot.lock().unwrap().acceleration = "auto".into();
        let closes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let engine = Arc::new(UncertainInferenceEngine {
            loads: std::sync::atomic::AtomicUsize::new(0),
            closes: closes.clone(),
        });
        let service = candidate::Service::new(
            Arc::new(Store::new(memory.clone(), memory)),
            engine.clone(),
            Arc::new(candidate::WallClock),
            None,
        );
        let call = service.bind(authority);
        let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})).unwrap();
        if stream {
            assert!(matches!(
                call.start_stream(options).await,
                Err(Error::CleanupUnknown)
            ));
        } else {
            assert_eq!(
                call.transcribe(0.25f32.to_le_bytes().to_vec(), options)
                    .await,
                Err(Error::CleanupUnknown)
            );
        }
        assert_eq!(call.prepare().await, Err(Error::CleanupUnknown));
        assert_eq!(service.shutdown().await, Err(Error::CleanupUnknown));
        assert_eq!(engine.loads.load(Ordering::SeqCst), 1);
        assert_eq!(
            closes.load(Ordering::SeqCst),
            1,
            "an owned handle is still closed, without erasing the earlier uncertainty"
        );
    }
}

struct UnknownResetRuntime(Arc<std::sync::atomic::AtomicUsize>);
impl candidate::ModelRuntime for UnknownResetRuntime {
    fn close(&self) -> Task<'_, speech_worker::Cleanup> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            speech_worker::Cleanup::Confirmed { graceful: false }
        })
    }
    fn begin_stream<'a>(
        &'a self,
        _: &'a str,
        _: &'a o3_server::speech_request::TranscriptionOptions,
    ) -> Task<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
    fn reset_stream(&self) -> Task<'_, Result<(), Error>> {
        Box::pin(async { Err(Error::CleanupUnknown) })
    }
}
struct UnknownResetEngine(Arc<std::sync::atomic::AtomicUsize>);
impl Engine for UnknownResetEngine {
    fn load<'a>(
        &'a self,
        _: &'a speech_catalog::Model,
        _: &'a Path,
        _: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async {
            Ok(Loaded {
                runtime: Arc::new(UnknownResetRuntime(self.0.clone())),
                metadata: json!({"backend":"CPU","supportsStreaming":true,"supportsLongForm":true}),
            })
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async { Ok(json!([])) })
    }
}
#[tokio::test]
async fn reset_unknown_survives_known_owned_close_and_repeated_scope_cleanup() {
    sandbox::assert_isolated();
    let (_unused, authority, memory, _) = fixture();
    let closes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = candidate::Service::new(
        Arc::new(Store::new(memory.clone(), memory)),
        Arc::new(UnknownResetEngine(closes.clone())),
        Arc::new(candidate::WallClock),
        None,
    );
    let call = service.bind(authority);
    let options=o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechRemoveFillerWords":false,"speechCustomWords":[],"speechSilenceFiltering":false})).unwrap();
    let stream = call.start_stream(options).await.unwrap();
    assert_eq!(stream.close().await, Err(Error::CleanupUnknown));
    assert_eq!(stream.close().await, Err(Error::CleanupUnknown));
    assert_eq!(call.prepare().await, Err(Error::CleanupUnknown));
    assert_eq!(service.shutdown().await, Err(Error::CleanupUnknown));
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}
