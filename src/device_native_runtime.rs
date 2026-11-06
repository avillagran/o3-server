//! DeviceService consumers for an explicitly owned native hub; no fallback success is synthesized.
use crate::{
    device_native_hub::{Device, Platform},
    device_native_owner::{LocalOwner, ReadyHost},
    device_native_state as kernel,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

pub struct NativeRuntime {
    pub owner: LocalOwner,
    state: Mutex<Value>,
    diagnostics: Mutex<Option<String>>,
    enabled: AtomicBool,
    changed: crate::terminal::Notifier,
}
impl NativeRuntime {
    pub fn new(owner: LocalOwner, summary: Value, changed: crate::terminal::Notifier) -> Self {
        Self {
            owner,
            state: Mutex::new(
                json!({"hosts":[summary],"devices":[],"sessions":[],"hostStatuses":{"local":{"status":"idle"}},"hostStatus":"idle","hubBasePath":"/api/device-hub","revision":0,"supportsHostRetry":true,"supportsToolUpdate":false,"supportsEasHosts":false,"supportsSimlockHosts":false}),
            ),
            diagnostics: Mutex::new(None),
            enabled: AtomicBool::new(true),
            changed,
        }
    }
    fn status(&self, status: &str, detail: Option<&str>) {
        let cached = self.diagnostics.lock().unwrap().clone();
        let mut state = self.state.lock().unwrap();
        let mut update = json!({"status":status});
        if let Some(detail) = detail {
            update["detail"] = json!(detail);
        }
        *state = kernel::apply_status(&state, "local", &update, cached.as_deref(), true);
        drop(state);
        self.changed.send_modify(|version| *version += 1);
    }
    pub fn snapshot(&self) -> Value {
        self.state.lock().unwrap().clone()
    }
    pub async fn configure(&self, settings: &Value) {
        let enabled = settings["enableDeviceSupport"] != false;
        let was_enabled = self.enabled.swap(enabled, Ordering::AcqRel);
        {
            let mut state = self.state.lock().unwrap();
            state["onboardingCompleted"] = settings["deviceOnboardingCompleted"].clone();
            state["agentAccessEnabled"] = json!(settings["enableAgentDeviceAccess"] == true);
        }
        if !enabled && was_enabled {
            self.owner.stop().await;
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
    }

    pub async fn list(&self) -> Result<Value, Value> {
        if !self.enabled.load(Ordering::Acquire) {
            return Ok(self.snapshot());
        }
        if !self.snapshot()["hosts"][0]["platforms"]
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
                self.status("failed", error["message"].as_str());
                return Err(error);
            }
        };
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
        let mut state = self.state.lock().unwrap();
        let refresh = kernel::apply_discovery(
            &state,
            kernel::RefreshInput {
                host_id: "local",
                listing: &listing,
                host_summaries: &before["hosts"],
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
                    .find(|device| device["hostId"] == "local" && device["id"] == id)
            })
            .cloned()
            .ok_or_else(|| not_found(id))
            .and_then(|device| serde_json::from_value(device).map_err(|_| not_found(id)))
    }
    fn active(&self, ready: &ReadyHost) -> bool {
        self.enabled.load(Ordering::Acquire)
            && self
                .owner
                .current()
                .is_some_and(|current| current.generation == ready.generation)
    }
    pub async fn open(&self, input: &Value) -> Result<Value, Value> {
        if input["hostId"].as_str().is_some_and(|id| id != "local") {
            return Err(host_changed());
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
                    json!({"_tag":"DevicePlatformUnavailableError","hostId":"local","platform":input["platform"],"reason":reason,"message":format!("{} devices are unavailable on host local: {reason}",input["platform"].as_str().unwrap_or("unknown"))}),
                );
            }
        }
        self.list().await?;
        let ready = self.owner.current().ok_or_else(host_changed)?;
        let mut device = self.device(id)?;
        if input["platform"] != device.platform.label() {
            return Err(not_found(id));
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
                    .find(|device| device["id"] == id && device["hostId"] == "local")
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
                "local",
                device.platform,
                id,
                &returned,
            )?)
            .map_err(|_| not_found(&returned))?;
        } else if device.booted {
            ready.hub.client.attach(&device).await?;
        }
        if !self.active(&ready) {
            return Err(host_changed());
        }
        let session = json!({"threadId":thread,"hostId":"local","deviceId":device.id,"platform":device.platform,"openedAt":crate::store::iso(crate::store::now_ms())});
        {
            let mut state = self.state.lock().unwrap();
            *state = kernel::record_session(&state, &session);
        }
        self.changed.send_modify(|version| *version += 1);
        Ok(session)
    }
    pub async fn close(&self, input: &Value) -> Result<Value, Value> {
        let thread = crate::p2::str_field(input, "threadId")?;
        if input["hostId"].as_str().is_some_and(|id| id != "local") {
            return Err(host_changed());
        }
        if input["shutdown"] == true {
            let id = crate::p2::str_field(input, "deviceId")?;
            let device = self.device(id)?;
            let ready = self.owner.current().ok_or_else(host_changed)?;
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
        *state = kernel::finish_boot(&state, "local", &self.id);
        drop(state);
        self.runtime.changed.send_modify(|version| *version += 1);
    }
}
fn not_found(id: &str) -> Value {
    json!({"_tag":"DeviceNotFoundError","hostId":"local","deviceId":id,"message":format!("Device {id} was not found on host local.")})
}
fn host_changed() -> Value {
    json!({"_tag":"DeviceHostUnavailableError","hostId":"local","hostLabel":"This machine","reason":"Host configuration changed. Retry the operation.","message":"This machine is unavailable: Host configuration changed. Retry the operation."})
}
