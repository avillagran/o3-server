//! Native speech coordination. Resource ownership and request authority are separate.
pub mod silence;
use crate::{
    speech_catalog::{self, Model},
    speech_model_store::Store,
    speech_worker::{Cleanup, Worker},
};
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
pub type Task<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unload {
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
    Never,
}
impl Unload {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediately => "immediately",
            Self::Min2 => "min_2",
            Self::Min5 => "min_5",
            Self::Min10 => "min_10",
            Self::Min15 => "min_15",
            Self::Hour1 => "hour_1",
            Self::Never => "never",
        }
    }
    fn millis(self) -> Option<u64> {
        match self {
            Self::Immediately => Some(0),
            Self::Min2 => Some(120_000),
            Self::Min5 => Some(300_000),
            Self::Min10 => Some(600_000),
            Self::Min15 => Some(900_000),
            Self::Hour1 => Some(3_600_000),
            Self::Never => None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub model_id: String,
    pub acceleration: String,
    pub unload: Unload,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Busy {
        operation: &'static str,
    },
    ModelNotFound {
        model_id: String,
    },
    Operation {
        operation: &'static str,
        cause: String,
    },
    Unsupported {
        platform: String,
        architecture: String,
    },
    CleanupUnknown,
}
/// Bind one of these to each request/session facade. Implementations must read
/// effective settings afresh and revalidate current authority/policy at commit.
/// A successful earlier snapshot is never a reusable authorization decision.
pub trait Settings: Send + Sync {
    fn read<'a>(&'a self, operation: &'static str) -> Task<'a, Result<Snapshot, Error>>;
    fn patch<'a>(&'a self, operation: &'static str, patch: Value) -> Task<'a, Result<(), Error>>;
}
/// Resource lifetime interface; only the concrete adapter below owns a Worker.
/// Tests inject an explicit virtual lifetime, never a fabricated process handle.
pub trait ModelRuntime: Send + Sync {
    fn close(&self) -> Task<'_, Cleanup>;
}
pub struct Loaded {
    pub runtime: Arc<dyn ModelRuntime>,
    pub metadata: Value,
}
struct WorkerModel(Worker);
impl ModelRuntime for WorkerModel {
    fn close(&self) -> Task<'_, Cleanup> {
        Box::pin(self.0.close())
    }
}
/// The engine implementation owns sealed package/model inputs. No caller URL or
/// raw executable/model path belongs in a public speech request.
pub trait Engine: Send + Sync {
    fn load<'a>(
        &'a self,
        model: &'a Model,
        path: &'a std::path::Path,
        acceleration: &'a str,
    ) -> Task<'a, Result<Loaded, Error>>;
    fn devices(&self) -> Task<'_, Result<Value, Error>>;
}
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
    fn sleep_until(&self, deadline: i64) -> Task<'_, ()>;
}
pub struct WallClock;
impl Clock for WallClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
    fn sleep_until(&self, deadline: i64) -> Task<'_, ()> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(
                deadline.saturating_sub(self.now()).max(0) as u64,
            ))
            .await;
        })
    }
}
#[derive(Clone)]
struct CachedModel {
    id: String,
    acceleration: String,
    runtime: Arc<dyn ModelRuntime>,
}
struct Transfer {
    id: u64,
    model_id: String,
    downloaded: u64,
    verifying: bool,
    cancel: watch::Sender<bool>,
    result: watch::Receiver<Option<Result<(), Error>>>,
}
struct State {
    closed: bool,
    unknown: bool,
    next: u64,
    active: Option<u64>,
    transcribing: u64,
    loaded: Option<CachedModel>,
    downloading: Option<Transfer>,
    unload: Unload,
    deadline: Option<i64>,
    devices: Option<(i64, Result<Value, String>)>,
}
struct Inner {
    state: Mutex<State>,
    store: Arc<Store>,
    engine: Arc<dyn Engine>,
    clock: Arc<dyn Clock>,
    timer: watch::Sender<Option<i64>>,
    changed: tokio::sync::Notify,
    silence: Arc<silence::Silence>,
    discovery: tokio::sync::Mutex<()>,
    disposal: tokio::sync::Mutex<()>,
}
/// Shared resources, but no captured privileged settings writer.
#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
    _lease: Arc<Lease>,
    unsupported: Option<(String, String, String)>,
}
pub struct Call {
    service: Service,
    settings: Arc<dyn Settings>,
}
impl Service {
    pub fn new(
        store: Arc<Store>,
        engine: Arc<dyn Engine>,
        clock: Arc<dyn Clock>,
        unsupported: Option<(String, String, String)>,
    ) -> Self {
        let (timer, mut deadlines) = watch::channel(None);
        let mut service = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    closed: false,
                    unknown: false,
                    next: 1,
                    active: None,
                    transcribing: 0,
                    loaded: None,
                    downloading: None,
                    unload: Unload::Min15,
                    deadline: None,
                    devices: None,
                }),
                silence: Arc::new(silence::Silence::new(store.clone(), clock.clone())),
                store,
                engine,
                clock,
                timer,
                changed: tokio::sync::Notify::new(),
                discovery: tokio::sync::Mutex::new(()),
                disposal: tokio::sync::Mutex::new(()),
            }),
            _lease: Arc::new(Lease {
                inner: std::sync::Weak::new(),
            }),
            unsupported,
        };
        service._lease = Arc::new(Lease {
            inner: Arc::downgrade(&service.inner),
        });
        let owner = Arc::downgrade(&service.inner);
        let clock = service.inner.clock.clone();
        tokio::spawn(async move {
            loop {
                let deadline = *deadlines.borrow_and_update();
                if let Some(deadline) = deadline {
                    tokio::select! {
                        _ = clock.sleep_until(deadline) => {},
                        changed = deadlines.changed() => { if changed.is_err() {break;} continue; }
                    }
                    let Some(inner) = owner.upgrade() else {
                        break;
                    };
                    let permit = {
                        let Ok(mut state) = inner.state.lock() else {
                            break;
                        };
                        if state.deadline != Some(deadline) {
                            continue;
                        }
                        if state.closed {
                            break;
                        }
                        if Inner::busy(&state) || state.transcribing > 0 {
                            inner.schedule(&mut state, 1000);
                            continue;
                        }
                        inner.reserve_locked(&mut state)
                    };
                    let _permit = permit;
                    let _ = inner.dispose().await;
                } else if deadlines.changed().await.is_err() {
                    break;
                }
            }
        });
        service
    }
    pub fn bind(&self, settings: Arc<dyn Settings>) -> Call {
        Call {
            service: self.clone(),
            settings,
        }
    }
}
impl Inner {
    fn schedule(&self, state: &mut State, minimum: u64) {
        state.deadline = if state.loaded.is_none() || state.closed {
            None
        } else {
            state.unload.millis().map(|millis| {
                self.clock
                    .now()
                    .saturating_add(millis.max(minimum).min(i64::MAX as u64) as i64)
            })
        };
        self.timer.send_replace(state.deadline);
    }
    fn busy(state: &State) -> bool {
        state.closed || state.unknown || state.active.is_some() || state.downloading.is_some()
    }
}
impl Call {
    async fn snapshot(&self, operation: &'static str) -> Result<Snapshot, Error> {
        let snapshot = self.settings.read(operation).await?;
        let mut state = self
            .service
            .inner
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?;
        if state.unload != snapshot.unload {
            state.unload = snapshot.unload;
            self.service.inner.schedule(&mut state, 0);
        }
        Ok(snapshot)
    }
    async fn devices(&self) -> Value {
        let inner = self.service.inner.clone();
        owned(async move {
            let _discovery = inner.discovery.lock().await;
            let now = inner.clock.now();
            {
                let state = inner.state.lock().map_err(|_| Error::CleanupUnknown)?;
                if let Some((expiry, value)) = &state.devices {
                    if now < *expiry {
                        return Ok(value.clone().unwrap_or_else(|_| json!([])));
                    }
                }
                if state.closed || state.unknown {
                    return Ok(json!([]));
                }
            }
            let value = inner.engine.devices().await;
            let mut state = inner.state.lock().map_err(|_| Error::CleanupUnknown)?;
            if value == Err(Error::CleanupUnknown) {
                state.unknown = true;
            }
            state.devices = Some((
                if value.is_ok() {
                    i64::MAX
                } else {
                    now.saturating_add(60_000)
                },
                value.clone().map_err(|cause| format!("{cause:?}")),
            ));
            Ok(value.unwrap_or_else(|_| json!([])))
        })
        .await
        .unwrap_or_else(|_| json!([]))
    }
    pub async fn status(&self) -> Result<Value, Error> {
        self.status_for("status").await
    }
    async fn status_for(&self, operation: &'static str) -> Result<Value, Error> {
        let snapshot = self.snapshot(operation).await?;
        if let Some((_, _, reason)) = &self.service.unsupported {
            return Ok(json!({"supported":false,"reason":reason}));
        }
        let model = speech_catalog::selected(&snapshot.model_id);
        let definition = model.as_value();
        let ready = self.service.inner.store.is_ready(model);
        let transcribing = self
            .service
            .inner
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .transcribing
            > 0;
        let devices = self.devices().await;
        Ok(
            json!({"supported":true,"state":if transcribing {"transcribing"}else if ready {"ready"}else{"missing-model"},"modelId":model.id(),"model":definition["name"],"size":model.size(),"supportsStreaming":definition["supportsStreaming"],"supportsBufferedStreaming":true,"supportsTranslation":definition["supportsTranslation"],"languages":definition["languages"],"supportsLanguageDetection":definition["supportsLanguageDetection"],"acceleration":snapshot.acceleration,"modelUnloadTimeout":snapshot.unload.as_str(),"gpuDevices":devices}),
        )
    }
    pub async fn models(&self, benchmarks: bool) -> Result<Value, Error> {
        let snapshot = self.snapshot("model listing").await?;
        self.models_with(&snapshot, benchmarks)
    }
    fn models_with(&self, snapshot: &Snapshot, benchmarks: bool) -> Result<Value, Error> {
        let selected = speech_catalog::selected(&snapshot.model_id);
        let state = self
            .service
            .inner
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?;
        let models=speech_catalog::all().iter().map(|model|{
            let source=model.as_value();let ready=self.service.inner.store.is_ready(model);let transfer=state.downloading.as_ref().filter(|transfer|transfer.model_id==model.id());
            let mut value=json!({"id":model.id(),"name":source["name"],"description":source["description"],"size":model.size(),"languages":source["languages"],"accuracy":0,"speed":0,"recommended":source["recommended"],"supportsStreaming":source["supportsStreaming"],"supportsLanguageDetection":source["supportsLanguageDetection"],"supportsTranslation":source["supportsTranslation"],"active":ready&&selected.id()==model.id(),"state":if let Some(transfer)=transfer {if transfer.verifying{"verifying"}else{"downloading"}}else if ready{"installed"}else{"downloadable"}});
            if benchmarks {value["benchmark"]=source["benchmark"].clone();}
            if let Some(transfer)=transfer {value["downloaded"]=json!(transfer.downloaded);}
            value
        }).collect::<Vec<_>>();
        Ok(json!({"models":models}))
    }
}

/// A reservation is owned by its spawned job, never by the waiting RPC future.
struct Permit {
    inner: Arc<Inner>,
    token: u64,
    minimum: u64,
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut state) = self.inner.state.lock() {
            if state.active == Some(self.token) {
                if std::thread::panicking() {
                    state.unknown = true;
                }
                state.active = None;
                self.inner.schedule(&mut state, self.minimum);
            }
        }
        self.inner.changed.notify_waiters();
    }
}
impl Inner {
    fn reserve_locked(self: &Arc<Self>, state: &mut State) -> Permit {
        let token = state.next;
        state.next = state.next.wrapping_add(1);
        state.active = Some(token);
        Permit {
            inner: self.clone(),
            token,
            minimum: 0,
        }
    }
    fn reserve(self: &Arc<Self>, operation: &'static str) -> Result<Permit, Error> {
        let mut state = self.state.lock().map_err(|_| Error::CleanupUnknown)?;
        if state.unknown {
            return Err(Error::CleanupUnknown);
        }
        if Self::busy(&state) {
            return Err(Error::Busy { operation });
        }
        Ok(self.reserve_locked(&mut state))
    }
    async fn dispose(&self) -> Result<(), Error> {
        let _disposal = self.disposal.lock().await;
        if self
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .unknown
        {
            return Err(Error::CleanupUnknown);
        }
        let model = self
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .loaded
            .take();
        if let Some(model) = model {
            if model.runtime.close().await == Cleanup::Unknown {
                self.state
                    .lock()
                    .map_err(|_| Error::CleanupUnknown)?
                    .unknown = true;
                return Err(Error::CleanupUnknown);
            }
        }
        Ok(())
    }
}
fn operation_error(operation: &'static str, cause: impl std::fmt::Display) -> Error {
    Error::Operation {
        operation,
        cause: cause.to_string(),
    }
}
async fn owned<T: Send + 'static>(
    job: impl Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, Error> {
    // Dropping this receiver leaves the resource owner running through cleanup.
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = job.await;
        let _ = tx.send(result);
    });
    rx.await.map_err(|_| Error::CleanupUnknown)?
}
impl Call {
    fn supported(&self) -> Result<(), Error> {
        if let Some((platform, architecture, _)) = &self.service.unsupported {
            return Err(Error::Unsupported {
                platform: platform.clone(),
                architecture: architecture.clone(),
            });
        }
        Ok(())
    }
    pub async fn select(&self, id: &str) -> Result<Value, Error> {
        let inner = self.service.inner.clone();
        let permit = inner.reserve("model selection")?;
        let model = speech_catalog::get(id).ok_or_else(|| Error::ModelNotFound {
            model_id: id.into(),
        })?;
        let settings = self.settings.clone();
        owned(async move {
            {
                let _permit = permit;
                inner.dispose().await?;
            }
            if inner
                .state
                .lock()
                .map_err(|_| Error::CleanupUnknown)?
                .closed
            {
                return Err(Error::Busy {
                    operation: "model selection",
                });
            }
            // This is a fresh authority check at commit, after asynchronous dispose.
            settings
                .patch("model selection", json!({"speechModelId":model.id()}))
                .await
        })
        .await?;
        self.models(false).await
    }
    pub async fn prepare(&self) -> Result<(), Error> {
        let snapshot = self.snapshot("model preparation").await?;
        let inner = self.service.inner.clone();
        let mut permit = inner.reserve("model preparation")?;
        self.supported()?;
        permit.minimum = 300_000;
        let model = speech_catalog::selected(&snapshot.model_id);
        owned(async move {
            let _permit = permit;
            if inner
                .state
                .lock()
                .map_err(|_| Error::CleanupUnknown)?
                .loaded
                .as_ref()
                .is_some_and(|loaded| {
                    loaded.id == model.id() && loaded.acceleration == snapshot.acceleration
                })
            {
                return Ok(());
            }
            let path =
                inner
                    .store
                    .installed_path(model)
                    .await
                    .ok_or_else(|| Error::ModelNotFound {
                        model_id: model.id().into(),
                    })?;
            inner.dispose().await?;
            let loaded = match inner
                .engine
                .load(model, &path, &snapshot.acceleration)
                .await
            {
                Ok(loaded) => loaded,
                Err(cause) => {
                    if cause == Error::CleanupUnknown {
                        inner
                            .state
                            .lock()
                            .map_err(|_| Error::CleanupUnknown)?
                            .unknown = true;
                    }
                    return Err(cause);
                }
            };
            let published = {
                let mut state = inner.state.lock().map_err(|_| Error::CleanupUnknown)?;
                if state.closed {
                    false
                } else {
                    state.loaded = Some(CachedModel {
                        id: model.id().into(),
                        acceleration: snapshot.acceleration,
                        runtime: loaded.runtime.clone(),
                    });
                    true
                }
            };
            if published {
                Ok(())
            } else {
                if loaded.runtime.close().await == Cleanup::Unknown {
                    inner
                        .state
                        .lock()
                        .map_err(|_| Error::CleanupUnknown)?
                        .unknown = true;
                    return Err(Error::CleanupUnknown);
                }
                Err(Error::Busy {
                    operation: "model preparation",
                })
            }
        })
        .await
    }
    pub async fn remove(&self, id: &str) -> Result<Value, Error> {
        let snapshot = self.snapshot("model removal").await?;
        let inner = self.service.inner.clone();
        let permit = inner.reserve("model removal")?;
        let model = speech_catalog::get(id).ok_or_else(|| Error::ModelNotFound {
            model_id: id.into(),
        })?;
        let settings = self.settings.clone();
        owned(async move {
            let replacement = {
                let _permit = permit;
                let loaded = inner
                    .state
                    .lock()
                    .map_err(|_| Error::CleanupUnknown)?
                    .loaded
                    .as_ref()
                    .is_some_and(|loaded| loaded.id == model.id());
                if loaded {
                    inner.dispose().await?;
                }
                inner
                    .store
                    .remove(model)
                    .await
                    .map_err(|cause| operation_error("model removal", cause))?;
                if speech_catalog::selected(&snapshot.model_id).id() == model.id() {
                    speech_catalog::all()
                        .iter()
                        .find(|other| other.id() != model.id() && inner.store.is_ready(other))
                        .map(|other| other.id().to_owned())
                } else {
                    None
                }
            };
            if let Some(id) = replacement {
                if inner
                    .state
                    .lock()
                    .map_err(|_| Error::CleanupUnknown)?
                    .closed
                {
                    return Err(Error::Busy {
                        operation: "model removal",
                    });
                }
                settings
                    .patch("model removal", json!({"speechModelId":id}))
                    .await?;
            }
            Ok(())
        })
        .await?;
        self.models(false).await
    }
    pub async fn update_acceleration(&self, value: &str) -> Result<Value, Error> {
        self.settings
            .patch("acceleration update", json!({"speechAcceleration":value}))
            .await?;
        self.status_for("acceleration update").await
    }
    pub async fn update_unload(&self, value: Unload) -> Result<Value, Error> {
        self.settings
            .patch(
                "model unload timeout update",
                json!({"speechModelUnloadTimeout":value.as_str()}),
            )
            .await?;
        self.status_for("model unload timeout update").await
    }
    pub async fn download(&self, id: &str) -> Result<Value, Error> {
        self.supported()?;
        let model = speech_catalog::get(id).ok_or_else(|| Error::ModelNotFound {
            model_id: id.into(),
        })?;
        let inner = self.service.inner.clone();
        let mut result = {
            let mut state = inner.state.lock().map_err(|_| Error::CleanupUnknown)?;
            if state.closed || state.active.is_some() {
                return Err(Error::Busy {
                    operation: "model download",
                });
            }
            if state.unknown {
                return Err(Error::CleanupUnknown);
            }
            if let Some(transfer) = &state.downloading {
                if transfer.model_id != id {
                    return Err(Error::Busy {
                        operation: "model download",
                    });
                }
                transfer.result.clone()
            } else {
                let token = state.next;
                state.next = state.next.wrapping_add(1);
                let (cancel, cancel_rx) = watch::channel(false);
                let (done, receiver) = watch::channel(None);
                state.downloading = Some(Transfer {
                    id: token,
                    model_id: id.into(),
                    downloaded: 0,
                    verifying: false,
                    cancel,
                    result: receiver.clone(),
                });
                let owner = inner.clone();
                tokio::spawn(async move {
                    let progress = |progress| {
                        if let Ok(mut state) = owner.state.lock() {
                            if let Some(transfer) = state
                                .downloading
                                .as_mut()
                                .filter(|transfer| transfer.id == token)
                            {
                                match progress {
                                    crate::speech_model_store::Progress::Downloaded(bytes) => {
                                        transfer.downloaded = bytes;
                                        transfer.verifying = false;
                                    }
                                    crate::speech_model_store::Progress::VerificationStarted => {
                                        transfer.verifying = true
                                    }
                                }
                            }
                        }
                    };
                    let result = tokio::try_join!(
                        owner.store.download(model, &progress, cancel_rx.clone()),
                        owner
                            .silence
                            .acquire(silence::Purpose::Installation, cancel_rx),
                    )
                    .map(|(path, _)| path);
                    let result = match result {
                        Ok(_) | Err(crate::speech_model_store::Error::Cancelled) => Ok(()),
                        Err(cause) => Err(operation_error("model download", cause)),
                    };
                    if let Ok(mut state) = owner.state.lock() {
                        if state
                            .downloading
                            .as_ref()
                            .is_some_and(|transfer| transfer.id == token)
                        {
                            state.downloading = None;
                            owner.schedule(&mut state, 0);
                        }
                    }
                    done.send_replace(Some(result));
                    owner.changed.notify_waiters();
                });
                receiver
            }
        };
        loop {
            if let Some(done) = result.borrow_and_update().clone() {
                done?;
                break;
            }
            result.changed().await.map_err(|_| Error::CleanupUnknown)?;
        }
        self.models(false).await
    }
    pub async fn cancel_download(&self, id: &str) -> Result<Value, Error> {
        let pending = {
            let state = self
                .service
                .inner
                .state
                .lock()
                .map_err(|_| Error::CleanupUnknown)?;
            state
                .downloading
                .as_ref()
                .filter(|transfer| transfer.model_id == id)
                .map(|transfer| {
                    transfer.cancel.send_replace(true);
                    transfer.result.clone()
                })
        };
        if let Some(mut pending) = pending {
            while pending.borrow_and_update().is_none() {
                pending.changed().await.map_err(|_| Error::CleanupUnknown)?;
            }
        }
        self.models(false).await
    }
}
struct Lease {
    inner: std::sync::Weak<Inner>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.begin_shutdown();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = inner.finish_shutdown().await;
            });
        }
    }
}
impl Inner {
    fn begin_shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.deadline = None;
            self.timer.send_replace(None);
            if let Some(transfer) = &state.downloading {
                transfer.cancel.send_replace(true);
            }
        }
    }
    async fn finish_shutdown(&self) -> Result<(), Error> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let busy = {
                let state = self.state.lock().map_err(|_| Error::CleanupUnknown)?;
                state.active.is_some() || state.downloading.is_some()
            };
            if !busy {
                break;
            }
            changed.await;
        }
        let _discovery = self.discovery.lock().await;
        self.dispose().await?;
        if self
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .unknown
        {
            Err(Error::CleanupUnknown)
        } else {
            Ok(())
        }
    }
}
impl Service {
    /// Marks closing before awaiting any operation. No fresh work can resurrect a model.
    pub async fn shutdown(&self) -> Result<(), Error> {
        let inner = self.inner.clone();
        inner.begin_shutdown();
        owned(async move { inner.finish_shutdown().await }).await
    }
}

/// A trusted installed-file receipt, not a wire payload or package downloader.
#[derive(Clone)]
pub struct EngineFile {
    pub relative: std::path::PathBuf,
    pub source: std::path::PathBuf,
    pub sha256: String,
    pub bytes: u64,
}
/// Actual Worker-backed engine. Every operation re-admits all graph files before
/// the Worker creates immutable private snapshots. No ambient Node or PATH lookup.
pub struct WorkerEngine {
    pub node: Arc<crate::speech_worker::NodeRuntime>,
    pub entry: std::path::PathBuf,
    pub files: Vec<EngineFile>,
    pub directory: std::path::PathBuf,
    pub startup_deadline: Duration,
    pub operation_deadline: Duration,
    pub close_deadline: Duration,
}
impl WorkerEngine {
    async fn start(
        &self,
        model: Option<(&Model, &std::path::Path)>,
        acceleration: &str,
    ) -> Result<Worker, Error> {
        let files = self.files.clone();
        let entry = self.entry.clone();
        let model =
            model.map(|(model, path)| (path.to_owned(), model.sha256().to_owned(), model.size()));
        let (engine, model) = tokio::task::spawn_blocking(move || {
            let graph = files
                .into_iter()
                .map(|file| {
                    crate::speech_worker::SealedFile::admit(&file.source, &file.sha256, file.bytes)
                        .map(|sealed| (file.relative, sealed))
                })
                .collect::<std::io::Result<Vec<_>>>()?;
            let engine = crate::speech_worker::EnginePackage::admit(entry, graph)?;
            let model = model
                .map(|(path, sha, size)| crate::speech_worker::SealedFile::admit(&path, &sha, size))
                .transpose()?;
            Ok::<_, std::io::Error>((engine, model))
        })
        .await
        .map_err(|cause| operation_error("engine admission", cause))?
        .map_err(|cause| operation_error("engine admission", cause))?;
        let directory = self.directory.join(
            crate::store::random_id().map_err(|cause| operation_error("engine startup", cause))?,
        );
        Worker::start(crate::speech_worker::WorkerSpec {
            node: self.node.clone(),
            engine,
            model,
            vad: None,
            directory,
            acceleration: acceleration.into(),
            startup_deadline: self.startup_deadline,
            operation_deadline: self.operation_deadline,
            close_deadline: self.close_deadline,
        })
        .await
        .map_err(|cause| {
            if cause
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<crate::speech_worker::StartupFailure>())
                .is_some_and(|failure| failure.cleanup == Cleanup::Unknown)
            {
                Error::CleanupUnknown
            } else {
                operation_error("engine startup", cause)
            }
        })
    }
}
impl Engine for WorkerEngine {
    fn load<'a>(
        &'a self,
        model: &'a Model,
        path: &'a std::path::Path,
        acceleration: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async move {
            let worker = self.start(Some((model, path)), acceleration).await?;
            match worker.load().await {
                Ok(metadata) => Ok(Loaded {
                    runtime: Arc::new(WorkerModel(worker)),
                    metadata,
                }),
                Err(cause) => {
                    if worker.close().await == Cleanup::Unknown {
                        Err(Error::CleanupUnknown)
                    } else {
                        Err(operation_error("engine loading", format!("{cause:?}")))
                    }
                }
            }
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async move {
            let worker = self.start(None, "auto").await?;
            worker.discover().await.map_err(|cause| {
                if cause == crate::speech_worker::Error::CleanupUnknown {
                    Error::CleanupUnknown
                } else {
                    operation_error("GPU discovery", format!("{cause:?}"))
                }
            })
        })
    }
}
