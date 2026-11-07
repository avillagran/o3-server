//! DeviceService consumers for an explicitly owned native hub; no fallback success is synthesized.
use crate::{
    device_native_hub::{Device, Platform},
    device_native_owner::{LocalOwner, ReadyHost},
    device_native_ssh_owner::SshOwner,
    device_native_state as kernel,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

pub enum OwnedHost {
    Local(LocalOwner),
    Ssh(SshOwner),
}
impl OwnedHost {
    pub fn current(&self) -> Option<ReadyHost> {
        match self {
            Self::Local(owner) => owner.current(),
            Self::Ssh(owner) => owner.current(),
        }
    }
    pub fn ensure_ready(
        &self,
    ) -> crate::device_native_owner::HostFuture<'_, Result<ReadyHost, Value>> {
        match self {
            Self::Local(owner) => Box::pin(owner.ensure_ready()),
            Self::Ssh(owner) => Box::pin(owner.ensure_ready()),
        }
    }
    pub fn stop(&self) -> crate::device_native_owner::HostFuture<'_, Result<(), Value>> {
        match self {
            Self::Local(owner) => Box::pin(async move {
                owner.stop().await;
                Ok(())
            }),
            Self::Ssh(owner) => Box::pin(owner.stop()),
        }
    }
    pub fn summary(&self) -> Option<Value> {
        match self {
            Self::Local(_) => None,
            Self::Ssh(owner) => owner.summary(),
        }
    }
}
pub struct NativeRuntime {
    pub owner: OwnedHost,
    host_id: String,
    label: String,
    stop_pending: AtomicBool,
    state: Mutex<Value>,
    diagnostics: Mutex<Option<String>>,
    enabled: AtomicBool,
    changed: crate::terminal::Notifier,
}
impl NativeRuntime {
    pub fn new(owner: LocalOwner, summary: Value, changed: crate::terminal::Notifier) -> Self {
        Self::with_owner(OwnedHost::Local(owner), summary, changed)
    }
    pub fn for_ssh(owner: SshOwner, summary: Value, changed: crate::terminal::Notifier) -> Self {
        Self::with_owner(OwnedHost::Ssh(owner), summary, changed)
    }
    pub fn ssh(
        cli: Arc<crate::device_native_ssh_transport::Cli>,
        summary: Value,
        changed: crate::terminal::Notifier,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let owner = SshOwner::new(cli).with_status(Arc::new(move |status, detail| {
                if let Some(runtime) = weak.upgrade() {
                    runtime.status(status, detail);
                }
            }));
            Self::with_owner(OwnedHost::Ssh(owner), summary, changed)
        })
    }
    fn with_owner(owner: OwnedHost, summary: Value, changed: crate::terminal::Notifier) -> Self {
        let host_id = summary["id"]
            .as_str()
            .expect("validated host summary ID")
            .to_owned();
        let label = summary["label"]
            .as_str()
            .unwrap_or("Device host")
            .to_owned();
        let mut state = json!({"hosts":[summary],"devices":[],"sessions":[],"hostStatuses":{},"hostStatus":"idle","hubBasePath":"/api/device-hub","revision":0,"supportsHostRetry":true,"supportsToolUpdate":false,"supportsEasHosts":false,"supportsSimlockHosts":false});
        state["hostStatuses"][&host_id] = json!({"status":"idle"});
        Self {
            owner,
            host_id,
            label,
            stop_pending: AtomicBool::new(false),
            state: Mutex::new(state),
            diagnostics: Mutex::new(None),
            enabled: AtomicBool::new(true),
            changed,
        }
    }
    pub fn update_owner_summary(&self) {
        if let Some(probe) = self.owner.summary() {
            let mut state = self.state.lock().unwrap();
            state["hosts"][0]["platforms"] = probe["platforms"].clone();
            state["hosts"][0]["hubInstalled"] = json!(probe["verifiedInstalled"] == true);
        }
    }
    fn status(&self, status: &str, detail: Option<&str>) {
        let cached = self.diagnostics.lock().unwrap().clone();
        let mut state = self.state.lock().unwrap();
        let mut update = json!({"status":status});
        if let Some(detail) = detail {
            update["detail"] = json!(detail);
        }
        *state = kernel::apply_status(&state, &self.host_id, &update, cached.as_deref(), true);
        drop(state);
        self.changed.send_modify(|version| *version += 1);
    }
    pub fn snapshot(&self) -> Value {
        self.state.lock().unwrap().clone()
    }
    pub async fn configure(&self, settings: &Value) -> Result<(), Value> {
        let enabled = settings["enableDeviceSupport"] != false;
        let was_enabled = self.enabled.swap(enabled, Ordering::AcqRel);
        {
            let mut state = self.state.lock().unwrap();
            state["onboardingCompleted"] = settings["deviceOnboardingCompleted"].clone();
            state["agentAccessEnabled"] = json!(settings["enableAgentDeviceAccess"] == true);
        }
        if !enabled && (was_enabled || self.stop_pending.load(Ordering::Acquire)) {
            self.stop_pending.store(true, Ordering::Release);
            if let Err(error) = self.owner.stop().await {
                self.status("failed", error["message"].as_str());
                return Err(error);
            }
            self.stop_pending.store(false, Ordering::Release);
            let mut state = self.state.lock().unwrap();
            state["hostStatus"] = json!("disabled");
            state.as_object_mut().unwrap().remove("hostStatusDetail");
            state["hostStatuses"] = json!({});
            state["devices"] = json!([]);
            state["sessions"] = json!([]);
            state["bootingDevices"] = json!([]);
            state["revision"] = json!(state["revision"].as_u64().unwrap_or(0) + 1);
            drop(state);
            *self.diagnostics.lock().unwrap() = None;
            self.changed.send_modify(|version| *version += 1);
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), Value> {
        self.enabled.store(false, Ordering::Release);
        self.stop_pending.store(true, Ordering::Release);
        if let Err(error) = self.owner.stop().await {
            self.status("failed", error["message"].as_str());
            return Err(error);
        }
        self.stop_pending.store(false, Ordering::Release);
        Ok(())
    }
    pub async fn list(&self) -> Result<Value, Value> {
        if !self.enabled.load(Ordering::Acquire) {
            return Ok(self.snapshot());
        }
        if self.host_id == "local"
            && !self.snapshot()["hosts"][0]["platforms"]
                .as_array()
                .is_some_and(|platforms| {
                    platforms
                        .iter()
                        .any(|platform| platform["available"] == true)
                })
        {
            return Ok(self.snapshot());
        }
        if self.owner.current().is_none() {
            self.status("starting", None);
        }
        let ready = match self.owner.ensure_ready().await {
            Ok(ready) => ready,
            Err(error) => {
                self.update_owner_summary();
                self.status("failed", error["message"].as_str());
                return Err(error);
            }
        };
        self.update_owner_summary();
        self.refresh(&ready).await
    }
    async fn refresh(&self, ready: &ReadyHost) -> Result<Value, Value> {
        let before = self.snapshot();
        let sessions = before["sessions"].as_array().cloned().unwrap_or_default();
        let listing = match ready.hub.client.list().await {
            Ok(listing) => listing,
            Err(error) => {
                self.status("failed", error["message"].as_str());
                return Err(error);
            }
        };
        self.update_owner_summary();
        let mut state = self.state.lock().unwrap();
        let summaries = state["hosts"].clone();
        let refresh = kernel::apply_discovery(
            &state,
            kernel::RefreshInput {
                host_id: &self.host_id,
                listing: &listing,
                host_summaries: &summaries,
                sessions_before: &sessions,
                ends_devices: false,
                enabled: self.enabled.load(Ordering::Acquire),
                host_unchanged: self
                    .owner
                    .current()
                    .is_some_and(|current| current.generation == ready.generation),
            },
        );
        *state = refresh.state;
        match refresh.detail {
            kernel::DetailUpdate::Set(detail) => *self.diagnostics.lock().unwrap() = Some(detail),
            kernel::DetailUpdate::Clear => *self.diagnostics.lock().unwrap() = None,
            kernel::DetailUpdate::Unchanged => {}
        }
        let snapshot = state.clone();
        drop(state);
        self.changed.send_modify(|version| *version += 1);
        Ok(snapshot)
    }
    fn device(&self, id: &str) -> Result<Device, Value> {
        let state = self.state.lock().unwrap();
        state["devices"]
            .as_array()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|device| device["hostId"] == self.host_id && device["id"] == id)
            })
            .cloned()
            .ok_or_else(|| not_found(&self.host_id, id))
            .and_then(|device| {
                serde_json::from_value(device).map_err(|_| not_found(&self.host_id, id))
            })
    }
    fn active(&self, ready: &ReadyHost) -> bool {
        self.enabled.load(Ordering::Acquire)
            && self
                .owner
                .current()
                .is_some_and(|current| current.generation == ready.generation)
    }
    pub async fn open(&self, input: &Value) -> Result<Value, Value> {
        if input["hostId"]
            .as_str()
            .is_some_and(|id| id != self.host_id)
        {
            return Err(host_changed(&self.host_id, &self.label));
        }
        let thread = crate::p2::str_field(input, "threadId")?;
        let id = crate::p2::str_field(input, "deviceId")?;
        if let Some(platform) = self.snapshot()["hosts"][0]["platforms"]
            .as_array()
            .and_then(|platforms| {
                platforms
                    .iter()
                    .find(|platform| platform["platform"] == input["platform"])
            })
        {
            if platform["available"] != true {
                let reason = platform["reason"]
                    .as_str()
                    .unwrap_or("This platform is unavailable on this host");
                return Err(
                    json!({"_tag":"DevicePlatformUnavailableError","hostId":self.host_id,"platform":input["platform"],"reason":reason,"message":format!("{} devices are unavailable on host {}: {reason}",input["platform"].as_str().unwrap_or("unknown"),self.host_id)}),
                );
            }
        }
        self.list().await?;
        let ready = self
            .owner
            .current()
            .ok_or_else(|| host_changed(&self.host_id, &self.label))?;
        let mut device = self.device(id)?;
        if input["platform"] != device.platform.label() {
            return Err(not_found(&self.host_id, id));
        }
        if device.platform == Platform::Android && !device.physical {
            ready.hub.client.check_video_encoder().await?;
        }
        if !device.booted && input["boot"] != false {
            let view = {
                let state = self.state.lock().unwrap();
                state["devices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|device| device["id"] == id && device["hostId"] == self.host_id)
                    .unwrap()
                    .clone()
            };
            {
                let mut state = self.state.lock().unwrap();
                *state = kernel::begin_boot(&state, &view, thread);
            }
            self.changed.send_modify(|version| *version += 1);
            let guard = BootGuard {
                runtime: self,
                id: id.to_owned(),
            };
            let returned = ready.hub.client.boot(&device).await?;
            drop(guard);
            let fresh = self.refresh(&ready).await?;
            device = serde_json::from_value(kernel::confirm_boot(
                &fresh,
                &self.host_id,
                device.platform,
                id,
                &returned,
            )?)
            .map_err(|_| not_found(&self.host_id, &returned))?;
        } else if device.booted {
            ready.hub.client.attach(&device).await?;
        }
        if !self.active(&ready) {
            return Err(host_changed(&self.host_id, &self.label));
        }
        let session = json!({"threadId":thread,"hostId":self.host_id,"deviceId":device.id,"platform":device.platform,"openedAt":crate::store::iso(crate::store::now_ms())});
        {
            let mut state = self.state.lock().unwrap();
            *state = kernel::record_session(&state, &session);
        }
        self.changed.send_modify(|version| *version += 1);
        Ok(session)
    }
    pub async fn close(&self, input: &Value) -> Result<Value, Value> {
        let thread = crate::p2::str_field(input, "threadId")?;
        if input["hostId"]
            .as_str()
            .is_some_and(|id| id != self.host_id)
        {
            return Err(host_changed(&self.host_id, &self.label));
        }
        if input["shutdown"] == true {
            let id = crate::p2::str_field(input, "deviceId")?;
            let device = self.device(id)?;
            let ready = self
                .owner
                .current()
                .ok_or_else(|| host_changed(&self.host_id, &self.label))?;
            ready.hub.client.shutdown(&device).await?;
        }
        let mut state = self.state.lock().unwrap();
        state["sessions"].as_array_mut().unwrap().retain(|session| {
            !(session["threadId"] == thread
                && input
                    .get("deviceId")
                    .is_none_or(|id| session["deviceId"] == *id))
        });
        state["revision"] = json!(state["revision"].as_u64().unwrap_or(0) + 1);
        drop(state);
        self.changed.send_modify(|version| *version += 1);
        Ok(Value::Null)
    }
    pub fn origin(&self) -> Option<String> {
        self.owner
            .current()
            .filter(|_| self.enabled.load(Ordering::Acquire))
            .map(|ready| ready.hub.origin)
    }
}
struct BootGuard<'a> {
    runtime: &'a NativeRuntime,
    id: String,
}
impl Drop for BootGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.runtime.state.lock().unwrap();
        *state = kernel::finish_boot(&state, &self.runtime.host_id, &self.id);
        drop(state);
        self.runtime.changed.send_modify(|version| *version += 1);
    }
}
fn not_found(host_id: &str, id: &str) -> Value {
    json!({"_tag":"DeviceNotFoundError","hostId":host_id,"deviceId":id,"message":format!("Device {id} was not found on host {host_id}.")})
}
fn host_changed(host_id: &str, label: &str) -> Value {
    let reason = "Host configuration changed. Retry the operation.";
    json!({"_tag":"DeviceHostUnavailableError","hostId":host_id,"hostLabel":label,"reason":reason,"message":format!("{label} is unavailable: {reason}")})
}
