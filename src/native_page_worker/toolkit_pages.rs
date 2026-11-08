//! Real retained Page workers for the private source toolkit. This coordinator
//! only attaches existing physical tabs; it never manufactures status or opens
//! a browser as a fallback for a missing tab.
use super::{
    control::{ControlledPage, Error as ControlError},
    source_deadline::FrozenDeadline,
    toolkit_callback::CallbackRequest,
    toolkit_owner::{CallbackExecutor, CallbackFuture, CallbackOutput, SourceCallback},
    CloseReceipt, Fault, NativePage, OperationResult, Worker, WorkerSpec,
};
use crate::store::State;
use serde_json::{json, Value};
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::watch;

fn directory(path: &Path) -> Result<File, Fault> {
    if !path.is_absolute() {
        return Err(Fault::InvalidInstallation);
    }
    let mut directory = File::open("/").map_err(|_| Fault::InvalidInstallation)?;
    for component in path.components().skip(1) {
        let Component::Normal(name) = component else {
            return Err(Fault::InvalidInstallation);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| Fault::InvalidInstallation)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(Fault::InvalidInstallation);
        }
        directory = unsafe { File::from_raw_fd(fd) };
        let metadata = directory
            .metadata()
            .map_err(|_| Fault::InvalidInstallation)?;
        if (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(Fault::InvalidInstallation);
        }
    }
    let metadata = directory
        .metadata()
        .map_err(|_| Fault::InvalidInstallation)?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700 {
        return Err(Fault::InvalidInstallation);
    }
    Ok(directory)
}

struct Homes {
    path: PathBuf,
    directory: File,
}
impl Homes {
    fn create(&self) -> Result<PathBuf, Fault> {
        let current = directory(&self.path)?;
        let captured = self
            .directory
            .metadata()
            .map_err(|_| Fault::InvalidInstallation)?;
        let named = current.metadata().map_err(|_| Fault::InvalidInstallation)?;
        if (captured.dev(), captured.ino()) != (named.dev(), named.ino()) {
            return Err(Fault::InvalidInstallation);
        }
        let name = format!(
            "page-{}",
            crate::store::random_id().map_err(|_| Fault::InvalidInstallation)?
        );
        let encoded = CString::new(name.as_bytes()).map_err(|_| Fault::InvalidInstallation)?;
        if unsafe { libc::mkdirat(current.as_raw_fd(), encoded.as_ptr(), 0o700) } != 0 {
            return Err(Fault::InvalidInstallation);
        }
        let path = self.path.join(name);
        let opened = directory(&path)?;
        let metadata = opened.metadata().map_err(|_| Fault::InvalidInstallation)?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700 {
            return Err(Fault::InvalidInstallation);
        }
        // Retain empty homes and worker artifacts. No tree removal belongs to
        // this factory's grant retirement or its tests.
        Ok(path)
    }
}

/// Only trusted application startup constructs this installation. In this
/// template, private_home is the exclusive parent for individual Page homes.
#[derive(Clone)]
pub struct PageInstallation {
    template: WorkerSpec,
    homes: Arc<Homes>,
}
impl PageInstallation {
    pub fn new(template: WorkerSpec) -> Result<Self, Fault> {
        template.validate()?;
        let settings = template
            .configuration
            .as_object()
            .ok_or(Fault::InvalidInstallation)?;
        let names = ["nativeRoot", "playwrightRoot", "effectRoot"];
        if settings.len() != names.len()
            || names.iter().any(|name| {
                !settings
                    .get(*name)
                    .and_then(Value::as_str)
                    .is_some_and(|value| Path::new(value).is_absolute())
            })
        {
            return Err(Fault::InvalidInstallation);
        }
        let homes = Arc::new(Homes {
            directory: directory(&template.private_home)?,
            path: template.private_home.clone(),
        });
        Ok(Self { template, homes })
    }
    fn worker(&self) -> Result<WorkerSpec, Fault> {
        let mut specification = self.template.clone();
        specification.private_home = self.homes.create()?;
        Ok(specification)
    }
}

struct Slot {
    page: NativePage,
    ready: watch::Receiver<Option<Result<ControlledPage, Fault>>>,
    closed: watch::Receiver<Option<Result<(), Fault>>>,
    birth_retirement: Arc<tokio::sync::RwLock<()>>,
}
impl Slot {
    async fn ready(&self) -> Result<ControlledPage, Fault> {
        let mut ready = self.ready.clone();
        loop {
            if let Some(result) = ready.borrow_and_update().clone() {
                return result;
            }
            if ready.changed().await.is_err() {
                return Err(Fault::CleanupUnknown);
            }
        }
    }
    async fn closed(&self) -> Result<(), Fault> {
        let mut closed = self.closed.clone();
        loop {
            if let Some(result) = *closed.borrow_and_update() {
                return result;
            }
            if closed.changed().await.is_err() {
                return Err(Fault::CleanupUnknown);
            }
        }
    }
}
struct Handle {
    owner: Weak<State>,
    installation: PageInstallation,
    slots: Mutex<Vec<Arc<Slot>>>,
    closing: AtomicBool,
    stop: watch::Sender<bool>,
}
impl crate::server_browser::BirthInstaller for Handle {
    fn validate(&self) -> Result<(), Value> {
        if self.closing.load(Ordering::Acquire) || *self.stop.borrow() {
            Err(json!({"message":"Canonical birth owner is closing","reason":"birth_owner_closed"}))
        } else {
            Ok(())
        }
    }
    fn install(self: Arc<Self>, page: NativePage) -> crate::server_browser::BirthInstallFuture {
        Box::pin(async move {
            let factory = OwnedPageFactory(self);
            let slot = factory.slot(page.clone(), StartPurpose::LocalBirth).map_err(|cause| json!({"message":"Canonical birth worker refused","cause":format!("{cause:?}")}))?;
            let retirement_gate = slot.birth_retirement.clone().read_owned().await;
            crate::server_browser::BirthInstaller::validate(factory.0.as_ref())?;
            let bound = slot.ready().await.map_err(|cause| json!({"message":"Canonical birth worker did not initialize","cause":format!("{cause:?}")}))?;
            crate::server_browser::CanonicalBirthInstallation::issued(page, bound, retirement_gate)
                .await
        })
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.stop.send_replace(true);
    }
}

/// Share this coordinator across toolkit callers for one actual State. Its
/// canonical identity is NativePage's physical owner allocation, not a tab ID,
/// request namespace, provider credential or ordinary Control generation.
#[derive(Clone)]
pub struct OwnedPageFactory(Arc<Handle>);
impl OwnedPageFactory {
    pub fn new(owner: &Arc<State>, installation: PageInstallation) -> Self {
        let handle = Arc::new(Handle {
            owner: Arc::downgrade(owner),
            installation,
            slots: Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
            stop: watch::channel(false).0,
        });
        let installer: Arc<dyn crate::server_browser::BirthInstaller> = handle.clone();
        owner
            .streams
            .p2
            .preview
            .browser
            .install_birth_provider(Arc::downgrade(&installer));
        Self(handle)
    }
    fn slot(&self, page: NativePage, purpose: StartPurpose) -> Result<Arc<Slot>, Fault> {
        let mut slots = self.0.slots.lock().unwrap();
        if self.0.closing.load(Ordering::Acquire) {
            return Err(Fault::Closed);
        }
        if let Some(slot) = slots.iter().find(|slot| slot.page.same_owner(&page)) {
            return Ok(slot.clone());
        }
        let (ready, readiness) = watch::channel(None);
        let (closed, completion) = watch::channel(None);
        let birth_retirement = Arc::new(tokio::sync::RwLock::new(()));
        let slot = Arc::new(Slot {
            birth_retirement: birth_retirement.clone(),
            page: page.clone(),
            ready: readiness,
            closed: completion,
        });
        slots.push(slot.clone());
        let installation = self.0.installation.clone();
        let mut stop = self.0.stop.subscribe();
        // Setup and resource retirement belong to this owned job, independently
        // of the first caller that starts observing the canonical Page.
        tokio::spawn(async move {
            let stopped = *stop.borrow();
            let result = if stopped {
                Err(Fault::Closed)
            } else {
                start(page, installation, purpose).await
            };
            match result {
                Ok(bound) => {
                    if *stop.borrow() {
                        ready.send_replace(Some(Err(Fault::Closed)));
                    } else {
                        ready.send_replace(Some(Ok(bound.clone())));
                    }
                    loop {
                        let stopped = *stop.borrow_and_update();
                        if stopped {
                            break;
                        }
                        if stop.changed().await.is_err() {
                            break;
                        }
                    }
                    // A configured birth retains these actual observers until
                    // its first author navigation/publication finishes or aborts.
                    let _retirement = birth_retirement.write().await;
                    closed.send_replace(Some(clean_close(bound.retire_worker().await)));
                }
                Err(cause) => {
                    ready.send_replace(Some(Err(cause)));
                    closed.send_replace(Some(if cause == Fault::CleanupUnknown {
                        Err(cause)
                    } else {
                        Ok(())
                    }));
                }
            }
        });
        Ok(slot)
    }
    async fn run_existing(
        &self,
        callback: SourceCallback,
        deadline: FrozenDeadline,
    ) -> Result<OperationResult, Fault> {
        if callback.validate().is_err() {
            return Ok(refusal("The source browser caller is no longer valid."));
        }
        let request = callback.request().clone();
        // These are the actual Page operations already implemented by the
        // pinned worker. snapshot's empty diagnostic arrays are not admitted.
        if !implemented(&request) {
            return Ok(refusal(
                "This operation requires the remaining native browser router.",
            ));
        }
        let Some(owner) = self.0.owner.upgrade() else {
            return Ok(refusal("The browser owner has closed."));
        };
        let browser = &owner.streams.p2.preview.browser;
        let owned = browser.owned_tabs(&request.thread_id, &request.agent_session_id);
        if !request.tab_id_explicit && owned.len() > 1 {
            return Ok(interrupted("Multiple tabs belong to this agent session. Pass a tabId from preview_open or preview_status tabs.", "tabRequired"));
        }
        let tab = request
            .tab_id
            .clone()
            .or_else(|| browser.latest_tab(&request.thread_id, Some(&request.agent_session_id)));
        let Some(tab) = tab else {
            return Ok(missing());
        };
        let physical = match browser.page(&request.thread_id, &tab) {
            Ok(physical) => physical,
            Err(_) => return Ok(missing()),
        };
        if request.operation != "waitFor"
            && physical
                .control()
                .snapshot()
                .agent_id
                .as_deref()
                .is_some_and(|agent| agent != request.agent_session_id)
        {
            return Ok(interrupted("This tab belongs to another agent session. You can read it, but open your own tab to act.", "agentMismatch"));
        }
        let page = match owner
            .streams
            .p2
            .preview
            .capture_native_page(&request.thread_id, &tab)
        {
            Ok(page) => page,
            Err(value) if value["reason"] == "previous_grant_cleanup_unknown" => {
                return Err(Fault::CleanupUnknown)
            }
            Err(_) => return Ok(refusal("The actual browser target cannot be captured.")),
        };
        if !physical.control().same_identity(&page.control()) || !browser.current(&physical) {
            return Ok(refusal(
                "The physical browser tab changed before admission.",
            ));
        }
        let slot = match self.slot(page.clone(), StartPurpose::Existing) {
            Ok(slot) => slot,
            Err(cause) => return ordinary_fault(cause),
        };
        drop(owner);
        let bound = match slot.ready().await {
            Ok(bound) => bound,
            Err(cause) => return ordinary_fault(cause),
        };
        match bound
            .source(callback, page, physical, self.0.owner.clone(), deadline)
            .await
        {
            Ok(result) => Ok(result),
            Err(ControlError::Control(value)) => Ok(control_error(value, request.timeout_ms)),
            Err(ControlError::Transport(cause)) => {
                if matches!(cause, Fault::CleanupUnknown) {
                    return Err(cause);
                }
                // The resource may have entered before its transport failed.
                // Require its independent close receipt before accounting an
                // ordinary operation error; never replay the action.
                clean_close(bound.retire_worker().await)?;
                ordinary_fault(cause)
            }
        }
    }
    async fn shutdown(&self) -> Result<(), Fault> {
        self.0.closing.store(true, Ordering::Release);
        self.0.stop.send_replace(true);
        let slots = self.0.slots.lock().unwrap().clone();
        let mut unknown = false;
        for slot in slots {
            if slot.closed().await.is_err() {
                unknown = true;
            }
        }
        if unknown {
            Err(Fault::CleanupUnknown)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum StartPurpose {
    Existing,
    LocalBirth,
}
async fn start(
    page: NativePage,
    installation: PageInstallation,
    purpose: StartPurpose,
) -> Result<ControlledPage, Fault> {
    let mut specification = installation.worker()?;
    if matches!(purpose, StartPurpose::LocalBirth) {
        let configuration = specification
            .configuration
            .as_object_mut()
            .ok_or(Fault::InvalidInstallation)?;
        if configuration.contains_key("canonicalBirth") {
            return Err(Fault::InvalidInstallation);
        }
        configuration.insert(
            "canonicalBirth".into(),
            json!({"kind":"new-local-target","targetId":page.target()}),
        );
    }

    let gateway = Arc::new(page.gateway().await.map_err(|_| Fault::Revoked)?);
    if let Err(cause) = gateway.configure_worker(&mut specification) {
        let close = gateway.shutdown().await?;
        return if close.clean {
            Err(cause)
        } else {
            Err(Fault::CleanupUnknown)
        };
    }
    let worker = Worker::spawn(specification, gateway)
        .await
        .map_err(|receipt| {
            if receipt.worker_reaped && receipt.owner_grant_closed {
                receipt.cause
            } else {
                Fault::CleanupUnknown
            }
        })?;
    Ok(ControlledPage::new(worker, page.control()))
}
fn clean_close(receipt: CloseReceipt) -> Result<(), Fault> {
    if receipt.worker_reaped && receipt.owner_grant_closed {
        Ok(())
    } else {
        Err(Fault::CleanupUnknown)
    }
}
fn implemented(request: &CallbackRequest) -> bool {
    matches!(
        (request.class, request.operation.as_str()),
        (
            super::NativeOperation::Act,
            "click" | "type" | "hover" | "select" | "drag" | "press" | "scroll"
        ) | (super::NativeOperation::Evaluate, "evaluate")
            | (super::NativeOperation::WaitFor, "waitFor")
    ) && !request.metadata_status
}
fn refusal(message: &str) -> OperationResult {
    OperationResult::NativeError(json!({"tag":"PreviewAutomationExecutionError","message":message}))
}
fn missing() -> OperationResult {
    OperationResult::NativeError(
        json!({"tag":"PreviewAutomationTabNotFoundError","message":"No server preview tab is open for this thread. Call preview_open first."}),
    )
}
fn interrupted(message: &str, reason: &str) -> OperationResult {
    OperationResult::NativeError(
        json!({"tag":"PreviewAutomationControlInterruptedError","message":message,"detail":reason}),
    )
}
fn control_error(value: Value, timeout: u64) -> OperationResult {
    let message = value["message"]
        .as_str()
        .unwrap_or("The native browser callback was refused.");
    if value["_tag"] == "BrowserControlInterrupted" {
        return OperationResult::NativeError(
            json!({"tag":"PreviewAutomationControlInterruptedError","message":message,"detail":value["reason"]}),
        );
    }
    if value["name"] == "TimeoutError" {
        return OperationResult::NativeError(
            json!({"tag":"PreviewAutomationTimeoutError","message":message,"detail":{"timeoutMs":timeout}}),
        );
    }
    refusal(message)
}
fn ordinary_fault(cause: Fault) -> Result<OperationResult, Fault> {
    if cause == Fault::CleanupUnknown {
        Err(cause)
    } else {
        Ok(refusal("The owned native Page runtime is unavailable."))
    }
}
impl CallbackExecutor for OwnedPageFactory {
    fn run(&self, callback: SourceCallback, deadline: FrozenDeadline) -> CallbackFuture {
        let factory = self.clone();
        // The source router intentionally discards click's point result. The
        // other listed source operations return void; select/evaluate do not.
        let void = matches!(
            callback.request().operation.as_str(),
            "click" | "type" | "hover" | "drag" | "press" | "scroll" | "waitFor"
        );
        Box::pin(async move {
            if callback.request().class == super::NativeOperation::Status {
                let entered = match callback.enter_status() {
                    Ok(entered) => entered,
                    Err(_) => {
                        return Ok(CallbackOutput::Value(refusal(
                            "The source status caller is unavailable.",
                        )))
                    }
                };
                let owner = factory.0.owner.upgrade().ok_or(Fault::Closed)?;
                let request = entered.callback().request();
                let browser = &owner.streams.p2.preview.browser;
                // Selection is read-only and bypasses the action ownership gate.
                // A present closing/unready target is never reported as absent.
                // An empty matching registry proves both explicit absence and
                // absence of every possible implicit latest-thread selection.
                let Some(catalog) = browser.status_catalog_if_absent(
                    &request.thread_id,
                    request.tab_id.as_deref(),
                    || owner.streams.p2.preview.profile_catalog(),
                ) else {
                    return Ok(CallbackOutput::Value(refusal(
                        "The existing tab status producers are not yet completely observed.",
                    )));
                };
                if entered.callback().validate().is_err() {
                    return Ok(CallbackOutput::Value(refusal(
                        "The source status caller has closed.",
                    )));
                }
                return Ok(CallbackOutput::StatusNoTab(catalog));
            }
            let result = factory.run_existing(callback, deadline).await?;
            Ok(match result {
                OperationResult::Value(_) | OperationResult::JavaScriptValue(_) if void => {
                    CallbackOutput::Undefined
                }
                value => CallbackOutput::Value(value),
            })
        })
    }
    fn mark_used(&self, request: &CallbackRequest) -> Result<(), Fault> {
        let owner = self.0.owner.upgrade().ok_or(Fault::Closed)?;
        owner.streams.p2.preview.browser.mark_used(
            &request.thread_id,
            &request.agent_session_id,
            request.tab_id.as_deref(),
        );
        Ok(())
    }
    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Fault>> + Send + '_>> {
        Box::pin(self.shutdown())
    }
}
